//! Runs a custom Python step in a child process.
//!
//! Isolation provided (see ADR-0001): separate process, isolated mode
//! (`python -I`), cleared environment, private working directory, wall-clock
//! timeout, cooperative cancellation, and on Unix an address-space limit.
//! This is lifecycle isolation, NOT a security sandbox.
//!
//! Contract with user code: the upstream result is available as `df`
//! (a pandas DataFrame); the code must assign a DataFrame to `result`.
//!
//! Data crosses the boundary as Parquet. Python reads it with pandas'
//! nullable dtypes, so an integer column with missing values stays integer.

use crate::duck;
use crate::table::Table;
use crate::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const WRAPPER: &str = r#"
import sys
inp, out, code_path = sys.argv[1], sys.argv[2], sys.argv[3]
import pandas as pd
# Nullable dtypes keep integers with missing values as integers (Int64)
# instead of silently turning them into floats.
df = pd.read_parquet(inp, dtype_backend="numpy_nullable")
with open(code_path, encoding="utf-8") as f:
    code = f.read()
ns = {"df": df, "pd": pd, "__name__": "__vda_step__"}
exec(compile(code, "<python step>", "exec"), ns)
result = ns.get("result")
if not isinstance(result, pd.DataFrame):
    sys.stderr.write("VDA_CONTRACT: the step must assign a pandas DataFrame to `result` (got %s)\n" % type(result).__name__)
    sys.exit(3)
try:
    result.to_parquet(out, index=False)
except Exception as e:
    sys.stderr.write("VDA_CONTRACT: `result` could not be saved as a table: %s: %s\n" % (type(e).__name__, e))
    sys.exit(3)
"#;

#[derive(Debug, Clone)]
pub struct PythonRunner {
    pub interpreter: PathBuf,
    /// Address-space cap in MiB (Unix only). `None` disables it.
    pub memory_limit_mb: Option<u64>,
    pub work_root: PathBuf,
}

static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

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

    pub fn run(&self, code: &str, input: &Table, timeout: Duration, cancel: &AtomicBool) -> Result<Table, Error> {
        let seq = RUN_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = self.work_root.join(format!("py-{}-{seq}", std::process::id()));
        fs::create_dir_all(&dir)?;
        // The child runs inside `dir`, so every path handed to it must be absolute.
        let dir = fs::canonicalize(&dir)?;
        let result = self.run_in(&dir, code, input, timeout, cancel);
        let _ = fs::remove_dir_all(&dir);
        result
    }

    fn run_in(&self, dir: &Path, code: &str, input: &Table, timeout: Duration, cancel: &AtomicBool) -> Result<Table, Error> {
        let (inp, out, code_path, wrapper, err_path) = (
            dir.join("input.parquet"),
            dir.join("output.parquet"),
            dir.join("step.py"),
            dir.join("wrapper.py"),
            dir.join("stderr.txt"),
        );
        duck::write_parquet(input, &inp)?;
        fs::write(&code_path, code)?;
        fs::write(&wrapper, WRAPPER)?;
        let stderr_file = fs::File::create(&err_path)?;

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
        cmd.arg(&wrapper)
            .arg(&inp)
            .arg(&out)
            .arg(&code_path)
            .current_dir(dir)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
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
        if !status.success() {
            return Err(Error::Python(summarize_error(&stderr, status.code())));
        }
        duck::read_parquet(&out)
    }
}

/// Keep the useful part of a traceback: the last frame in user code plus the
/// exception line.
fn summarize_error(stderr: &str, code: Option<i32>) -> String {
    if let Some(line) = stderr.lines().find(|l| l.starts_with("VDA_CONTRACT: ")) {
        return line.trim_start_matches("VDA_CONTRACT: ").to_string();
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
