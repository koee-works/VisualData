//! Runs a custom Python step in a child process.
//!
//! Isolation provided (see ADR-0001): separate process, isolated mode
//! (`python -I`), cleared environment, private working directory, wall-clock
//! timeout, cooperative cancellation, and on Unix an address-space limit.
//! This is lifecycle isolation, NOT a security sandbox.
//!
//! Contract with user code: the first input is available as `df` and every
//! input in the dict `dfs` by its id (pandas DataFrames); the code must
//! assign a DataFrame to `result`.
//!
//! Data crosses the boundary as Parquet: the child reads the upstream result
//! file in place and writes its own. pandas loads the whole input into memory,
//! so the memory cap bounds how large a Python step's input can be. Python
//! reads with nullable dtypes, so an integer column with missing values stays
//! integer.

use crate::duck;
use crate::table::Table;
use crate::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

// Every exit goes through os._exit after flushing stderr. Normal interpreter
// shutdown can deadlock in pyarrow's thread pool after a MemoryError, which
// left out-of-memory steps hanging until their timeout.
const WRAPPER: &str = r#"
import os, sys

def finish(code, message=None, show_traceback=False):
    try:
        if show_traceback:
            import traceback
            traceback.print_exc()
        if message:
            sys.stderr.write(message + "\n")
        # os._exit skips the interpreter's own flushing, so printed output
        # would be lost without this.
        sys.stdout.flush()
        sys.stderr.flush()
    except BaseException:
        pass
    os._exit(code)

try:
    out, code_path = sys.argv[1], sys.argv[2]
    pairs = sys.argv[3:]
    import pandas as pd
    # Nullable dtypes keep integers with missing values as integers (Int64)
    # instead of silently turning them into floats.
    dfs = {pairs[i]: pd.read_parquet(pairs[i + 1], dtype_backend="numpy_nullable") for i in range(0, len(pairs), 2)}
    df = dfs[pairs[0]]
    with open(code_path, encoding="utf-8") as f:
        code = f.read()
    import json, datetime
    def param(kind, v):
        return int(v) if kind == "int" else datetime.date.fromisoformat(v) if kind == "date" else v
    with open("params.json", encoding="utf-8") as f:
        params = {name: param(kind, v) for name, kind, v in json.load(f)}
    ns = {"df": df, "dfs": dfs, "pd": pd, "params": params, "__name__": "__vda_step__"}
except BaseException:
    finish(4, "VDA_SETUP: could not prepare the Python step", show_traceback=True)

try:
    exec(compile(code, "<python step>", "exec"), ns)
except MemoryError:
    finish(1, "MemoryError")
except BaseException:
    finish(1, show_traceback=True)

result = ns.get("result")
if not isinstance(result, pd.DataFrame):
    finish(3, "VDA_CONTRACT: the step must assign a pandas DataFrame to `result` (got %s)" % type(result).__name__)
try:
    result.to_parquet(out, index=False)
except BaseException as e:
    finish(3, "VDA_CONTRACT: `result` could not be saved as a table: %s: %s" % (type(e).__name__, e))
finish(0)
"#;

#[derive(Debug, Clone)]
pub struct PythonRunner {
    pub interpreter: PathBuf,
    /// Address-space cap in MiB (Unix only). `None` disables it.
    pub memory_limit_mb: Option<u64>,
    pub work_root: PathBuf,
}

static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// Interpreter to use when the project does not name one: `$VDA_PYTHON`, or
/// `python3`. (On Windows the launcher is usually `python`.)
pub fn default_interpreter() -> PathBuf {
    std::env::var_os("VDA_PYTHON").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("python3"))
}

/// Packages the app looks for, and what each is for.
pub const PACKAGES: &[(&str, &str, bool)] = &[
    ("pandas", "Python steps (required)", true),
    ("pyarrow", "Python steps (required)", true),
    ("numpy", "numerical work", false),
    ("scipy", "statistics", false),
    ("statsmodels", "statistical models", false),
    ("sklearn", "machine learning (scikit-learn)", false),
    ("matplotlib", "plots", false),
    ("duckdb", "rerunning exported Jupyter notebooks", false),
];

/// What an interpreter is and has.
#[derive(Debug, Clone, PartialEq)]
pub struct Environment {
    pub interpreter: PathBuf,
    /// The executable it resolves to (`sys.executable`).
    pub executable: Option<String>,
    pub version: Option<String>,
    /// Package name, what it is for, whether required, installed version.
    pub packages: Vec<(String, String, bool, Option<String>)>,
    /// Why the interpreter cannot run Python steps, if it cannot.
    pub problem: Option<String>,
}

impl Environment {
    pub fn usable(&self) -> bool {
        self.problem.is_none()
    }
}

/// The interpreter's path: as given when it names a file, else found on the PATH.
pub fn resolve(interpreter: &Path) -> PathBuf {
    if interpreter.components().count() > 1 || interpreter.is_absolute() {
        return interpreter.to_path_buf();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(interpreter);
        if candidate.is_file() {
            return candidate;
        }
        if cfg!(windows) {
            let exe = candidate.with_extension("exe");
            if exe.is_file() {
                return exe;
            }
        }
    }
    interpreter.to_path_buf()
}

/// Interpreters worth offering: the project's own virtual environment, the
/// active one, conda and pyenv environments, and `python3` on the PATH.
pub fn discover(project: Option<&Path>) -> Vec<PathBuf> {
    let bin = |dir: PathBuf| -> PathBuf {
        if cfg!(windows) { dir.join("Scripts").join("python.exe") } else { dir.join("bin").join("python") }
    };
    let mut found: Vec<PathBuf> = Vec::new();
    if let Some(p) = project {
        for name in [".venv", "venv", "env"] {
            found.push(bin(p.join(name)));
        }
    }
    if let Some(v) = std::env::var_os("VIRTUAL_ENV") {
        found.push(bin(PathBuf::from(v)));
    }
    if let Some(c) = std::env::var_os("CONDA_PREFIX") {
        found.push(if cfg!(windows) { PathBuf::from(&c).join("python.exe") } else { PathBuf::from(&c).join("bin").join("python") });
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from) {
        for root in ["miniconda3/envs", "anaconda3/envs", "miniforge3/envs", "mambaforge/envs", ".pyenv/versions"] {
            if let Ok(entries) = std::fs::read_dir(home.join(root)) {
                let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
                dirs.sort();
                found.extend(dirs.into_iter().map(bin));
            }
        }
        for base in ["miniconda3", "anaconda3", "miniforge3"] {
            found.push(bin(home.join(base)));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in ["python3", "python"] {
                let exe = if cfg!(windows) { dir.join(format!("{name}.exe")) } else { dir.join(name) };
                found.push(exe);
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    found
        .into_iter()
        .filter(|p| p.is_file())
        // The same interpreter reached through several links counts once.
        .filter(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())))
        .collect()
}

impl PythonRunner {
    /// The interpreter's version, location and packages. Never fails: a
    /// broken interpreter is described, with what is wrong.
    pub fn environment(&self) -> Environment {
        let names: Vec<String> = PACKAGES.iter().map(|p| format!("'{}'", p.0)).collect();
        let script = format!(
            "import sys, importlib, json\nout = {{'exe': sys.executable, 'version': sys.version.split()[0], 'packages': {{}}}}\n\
             for n in [{}]:\n    try:\n        m = importlib.import_module(n)\n        out['packages'][n] = str(getattr(m, '__version__', 'installed'))\n\
             \x20   except Exception:\n        out['packages'][n] = None\nprint(json.dumps(out))",
            names.join(", ")
        );
        let mut env = Environment { interpreter: self.interpreter.clone(), executable: None, version: None, packages: vec![], problem: None };
        // A bare name is found on the PATH here: with the environment
        // cleared, Python cannot tell where it was started from.
        let exe = resolve(&self.interpreter);
        let out = Command::new(&exe).args(["-I", "-c", &script]).env_clear().output();
        let fill = |env: &mut Environment, got: &crate::json::Json| {
            for (name, why, required) in PACKAGES {
                let v = got.get("packages").and_then(|p| p.get(name)).and_then(|v| v.as_str()).map(str::to_string);
                env.packages.push((name.to_string(), why.to_string(), *required, v));
            }
        };
        match out {
            Err(e) => env.problem = Some(format!("cannot start {}: {e}", self.interpreter.display())),
            Ok(o) if !o.status.success() => {
                env.problem = Some(format!("{} did not run: {}", self.interpreter.display(), String::from_utf8_lossy(&o.stderr).trim().lines().last().unwrap_or("")))
            }
            Ok(o) => match crate::json::Json::parse(String::from_utf8_lossy(&o.stdout).trim()) {
                Err(_) => env.problem = Some("the interpreter's answer could not be read".into()),
                Ok(got) => {
                    env.executable = got
                        .get("exe")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .or_else(|| exe.is_absolute().then(|| exe.to_string_lossy().into_owned()));
                    env.version = got.get("version").and_then(|v| v.as_str()).map(str::to_string);
                    fill(&mut env, &got);
                    let missing: Vec<&str> = env.packages.iter().filter(|p| p.2 && p.3.is_none()).map(|p| p.0.as_str()).collect();
                    if !missing.is_empty() {
                        env.problem = Some(format!(
                            "it has no {}; install with: {} -m pip install {}",
                            missing.join(" or "),
                            self.interpreter.display(),
                            missing.join(" ")
                        ));
                    }
                }
            },
        }
        env
    }

    pub fn new(interpreter: impl Into<PathBuf>, work_root: impl Into<PathBuf>) -> Self {
        PythonRunner { interpreter: interpreter.into(), memory_limit_mb: Some(2048), work_root: work_root.into() }
    }

    /// Interpreter and library versions; part of every Python step's cache key
    /// and recorded in run records for replay.
    pub fn fingerprint(&self) -> Result<String, Error> {
        let out = Command::new(&self.interpreter)
            .args(["-I", "-c", "import sys, pandas, pyarrow; print('python', sys.version.split()[0], '| pandas', pandas.__version__, '| pyarrow', pyarrow.__version__)"])
            .env_clear()
            .output()
            .map_err(|e| Error::Python(format!("cannot start interpreter {:?}: {e}", self.interpreter)))?;
        if !out.status.success() {
            return Err(Error::Python(format!(
                "interpreter check failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Run `code` with the Parquet file `input` as `df`, writing `result` to
    /// `output`. The input is read in place; nothing is copied. Paths must be
    /// absolute (the child runs in its own working directory).
    pub fn run_files(&self, code: &str, input: &Path, output: &Path, timeout: Duration, cancel: &AtomicBool) -> Result<(), Error> {
        self.run_inputs(code, &[("input".to_string(), input.to_path_buf())], output, timeout, cancel).result
    }

    /// Run `code` with several Parquet inputs: the first is `df`, and all are
    /// in `dfs` under their names.
    pub fn run_inputs(
        &self,
        code: &str,
        inputs: &[(String, PathBuf)],
        output: &Path,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> PythonRun {
        self.run_with_params(code, inputs, "[]", output, timeout, cancel)
    }

    /// [`Self::run_inputs`] with the analysis parameters as `params` (JSON
    /// from [`crate::params::python_json`]).
    pub fn run_with_params(
        &self,
        code: &str,
        inputs: &[(String, PathBuf)],
        params: &str,
        output: &Path,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> PythonRun {
        if inputs.is_empty() {
            return PythonRun { result: Err(Error::Python("a Python step needs at least one input".into())), output: String::new() };
        }
        let seq = RUN_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = self.work_root.join(format!("py-{}-{seq}", std::process::id()));
        let dir = match fs::create_dir_all(&dir).and_then(|_| fs::canonicalize(&dir)) {
            Ok(d) => d,
            Err(e) => return PythonRun { result: Err(e.into()), output: String::new() },
        };
        if let Err(e) = fs::write(dir.join("params.json"), params) {
            return PythonRun { result: Err(e.into()), output: String::new() };
        }
        let result = self.run_in(&dir, code, inputs, output, timeout, cancel);
        let _ = fs::remove_dir_all(&dir);
        result
    }

    /// Convenience for small in-memory tables (tests, tools).
    pub fn run(&self, code: &str, input: &Table, timeout: Duration, cancel: &AtomicBool) -> Result<Table, Error> {
        fs::create_dir_all(&self.work_root)?;
        let root = fs::canonicalize(&self.work_root)?;
        let seq = RUN_SEQ.fetch_add(1, Ordering::SeqCst);
        let (inp, out) = (
            root.join(format!("table-in-{}-{seq}.parquet", std::process::id())),
            root.join(format!("table-out-{}-{seq}.parquet", std::process::id())),
        );
        duck::write_parquet(input, &inp)?;
        let result = self.run_files(code, &inp, &out, timeout, cancel).and_then(|_| duck::read_parquet(&out));
        let _ = fs::remove_file(&inp);
        let _ = fs::remove_file(&out);
        result
    }

    fn run_in(
        &self,
        dir: &Path,
        code: &str,
        inputs: &[(String, PathBuf)],
        out: &Path,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> PythonRun {
        let mut output = String::new();
        let result = self.run_child(dir, code, inputs, out, timeout, cancel, &mut output);
        PythonRun { result, output }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_child(
        &self,
        dir: &Path,
        code: &str,
        inputs: &[(String, PathBuf)],
        out: &Path,
        timeout: Duration,
        cancel: &AtomicBool,
        output: &mut String,
    ) -> Result<(), Error> {
        let (code_path, wrapper, err_path) = (dir.join("step.py"), dir.join("wrapper.py"), dir.join("stderr.txt"));
        let out_path = dir.join("stdout.txt");
        fs::write(&code_path, code)?;
        fs::write(&wrapper, WRAPPER)?;
        let stderr_file = fs::File::create(&err_path)?;
        let stdout_file = fs::File::create(&out_path)?;

        let mut cmd = if let (true, Some(mb)) = (cfg!(unix), self.memory_limit_mb) {
            let kb = mb * 1024;
            let mut c = Command::new("/bin/sh");
            c.arg("-c")
                .arg(format!("ulimit -v {kb} && exec \"$0\" -I \"$@\""))
                .arg(&self.interpreter);
            c
        } else {
            let mut c = Command::new(&self.interpreter);
            c.arg("-I");
            c
        };
        cmd.arg(&wrapper).arg(out).arg(&code_path);
        for (name, path) in inputs {
            cmd.arg(name).arg(path);
        }
        cmd
            .current_dir(dir)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", dir)
            .stdin(Stdio::null())
            .stdout(stdout_file)
            .stderr(stderr_file);

        let start = Instant::now();
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Python(format!("cannot start interpreter {:?}: {e}", self.interpreter)))?;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if cancel.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Cancelled);
            }
            if start.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Python(format!(
                    "timed out after {:.1}s and was stopped",
                    timeout.as_secs_f64()
                )));
            }
            std::thread::sleep(Duration::from_millis(15));
        };

        let stderr = fs::read_to_string(&err_path).unwrap_or_default();
        *output = step_output(&fs::read(&out_path).unwrap_or_default(), &stderr);
        if !status.success() {
            return Err(Error::Python(summarize_error(&stderr, status.code())));
        }
        Ok(())
    }
}

/// Most of a step's printed output kept with a run, per stream.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// What a Python step ran to: its result and anything it printed.
pub struct PythonRun {
    pub result: Result<(), Error>,
    /// stdout, then stderr (warnings, tracebacks), each capped; the
    /// wrapper's own messages are left out.
    pub output: String,
}

fn capped(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_OUTPUT_BYTES)]).into_owned();
    if bytes.len() > MAX_OUTPUT_BYTES {
        format!("{text}\n… output cut at {} KB", MAX_OUTPUT_BYTES / 1024)
    } else {
        text
    }
}

pub(crate) fn step_output(stdout: &[u8], stderr: &str) -> String {
    let err: String = stderr
        .lines()
        .filter(|l| !l.starts_with("VDA_CONTRACT: ") && !l.starts_with("VDA_SETUP: "))
        .collect::<Vec<_>>()
        .join("\n");
    let (out, err) = (capped(stdout), capped(err.as_bytes()));
    match (out.trim().is_empty(), err.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => out,
        (true, false) => err,
        (false, false) => format!("{}\n{err}", out.trim_end()),
    }
}

/// Keep the useful part of a traceback: the last frame in user code plus the
/// exception line.
fn summarize_error(stderr: &str, code: Option<i32>) -> String {
    if let Some(line) = stderr.lines().find(|l| l.starts_with("VDA_CONTRACT: ")) {
        return line.trim_start_matches("VDA_CONTRACT: ").to_string();
    }
    if stderr.lines().any(|l| l.starts_with("VDA_SETUP: ")) {
        let cause = stderr.lines().rev().find(|l| !l.trim().is_empty() && !l.starts_with("VDA_SETUP"));
        return format!("could not prepare the Python step: {}", cause.unwrap_or("unknown error").trim());
    }
    if stderr.contains("MemoryError") {
        return "ran out of memory (limit reached)".into();
    }
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let user_frame = lines
        .iter()
        .rposition(|l| l.contains("File \"<python step>\""))
        .map(|i| lines[i].trim().replace("File \"<python step>\", ", ""));
    let last = lines.last().map(|s| s.trim().to_string()).unwrap_or_else(|| format!("exited with code {code:?}"));
    match user_frame {
        Some(f) => format!("{last} ({f})"),
        None => last,
    }
}
