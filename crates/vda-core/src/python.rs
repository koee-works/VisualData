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
    ns = {"df": df, "dfs": dfs, "pd": pd, "__name__": "__vda_step__"}
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

impl PythonRunner {
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
        if inputs.is_empty() {
            return PythonRun { result: Err(Error::Python("a Python step needs at least one input".into())), output: String::new() };
        }
        let seq = RUN_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = self.work_root.join(format!("py-{}-{seq}", std::process::id()));
        let dir = match fs::create_dir_all(&dir).and_then(|_| fs::canonicalize(&dir)) {
            Ok(d) => d,
            Err(e) => return PythonRun { result: Err(e.into()), output: String::new() },
        };
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

fn step_output(stdout: &[u8], stderr: &str) -> String {
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
