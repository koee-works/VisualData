//! A console beside a Python or R step: a child process that keeps the
//! step's inputs loaded, so trying a line takes as long as the line itself.
//!
//! It is for exploring. What counts is still the step, which always runs
//! clean in its own process (see `python.rs`, `rlang.rs`); [`Console::save_result`]
//! lets the app compare the two, so state that only exists in the console
//! (a variable set by a line since deleted) is noticed.
//!
//! The same isolation as steps: own process and directory, cleared
//! environment, memory limit on Unix. Requests are framed on stdin; replies
//! are lines on stdout marked with `\x1eVDA ` (see console.py and console.R).

use crate::json::Json;
use crate::Error;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const PYTHON: &str = include_str!("console.py");
const R: &str = include_str!("console.R");
const MARK: &str = "\x1eVDA ";

static SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Python,
    R,
}

/// What a console holds, for its Environment pane.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub name: String,
    pub kind: String,
    pub summary: String,
}

/// A table shown in the console: the first rows, as JSON values.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    /// Name and the language's own type name.
    pub columns: Vec<(String, String)>,
    pub rows: Vec<Vec<Json>>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Table(Preview),
    Text(String),
}

/// What running some code gave.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    /// The value of the last expression, as a notebook or the R console shows it.
    pub value: Option<Shown>,
    /// The error and its line in the code that was run.
    pub error: Option<(String, Option<usize>)>,
    /// Plots drawn, as PNG files (removed with the console).
    pub images: Vec<PathBuf>,
    pub objects: Vec<Object>,
    pub elapsed: Duration,
}

pub struct Console {
    pub language: Language,
    /// "3.12.4" or "4.4.1".
    pub version: String,
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<String>,
    dir: PathBuf,
    next: u64,
    /// What the console held when it started.
    pub objects: Vec<Object>,
}

/// How to start a console.
pub struct Start<'a> {
    pub language: Language,
    /// The interpreter (`python3`) or `Rscript`.
    pub program: &'a Path,
    /// The inputs by id, as Parquet files; the first is `df`.
    pub inputs: &'a [(String, PathBuf)],
    pub params: &'a [crate::params::Parameter],
    /// R package libraries to use (R only).
    pub libraries: &'a [String],
    pub work_root: &'a Path,
    pub memory_limit_mb: Option<u64>,
}

impl Console {
    /// Start the process and load the inputs. Waits until they are loaded.
    pub fn start(s: Start, timeout: Duration) -> Result<Console, Error> {
        if s.inputs.is_empty() {
            return Err(Error::Validation("the console needs the step's inputs".into()));
        }
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = s.work_root.join(format!("console-{}-{seq}", std::process::id()));
        fs::create_dir_all(&dir)?;
        let dir = fs::canonicalize(&dir)?;
        let program = crate::python::resolve(s.program);
        let (script, mut args): (PathBuf, Vec<String>) = match s.language {
            Language::Python => {
                fs::write(dir.join("params.json"), crate::params::python_json(s.params))?;
                let path = dir.join("console.py");
                fs::write(&path, PYTHON)?;
                let mut a = vec!["-I".to_string(), path.to_string_lossy().into_owned()];
                for (id, p) in s.inputs {
                    a.push(id.clone());
                    a.push(p.to_string_lossy().into_owned());
                }
                (path, a)
            }
            Language::R => {
                let mut ids = Vec::new();
                for (n, (_, p)) in s.inputs.iter().enumerate() {
                    let types = crate::duck::parquet_to_csv(p, &dir.join(format!("in-{n}.csv")))?;
                    let words: Vec<&str> = types.iter().map(|(_, t)| crate::rlang::type_word(*t)).collect();
                    fs::write(dir.join(format!("in-{n}.types")), words.join("\n") + "\n")?;
                    ids.push(n.to_string());
                }
                let names: Vec<String> = s.inputs.iter().map(|(id, _)| crate::rlang::r_string(id)).collect();
                fs::write(dir.join("params.R"), format!("{}\ninput_names <- c({})\n", crate::rlang::params_r(s.params), names.join(", ")))?;
                let path = dir.join("console.R");
                fs::write(&path, format!("local({{\n{}\n{R}\n}})\n", crate::rlang::R_IO))?;
                let mut a = vec!["--vanilla".to_string(), path.to_string_lossy().into_owned()];
                a.extend(ids);
                (path, a)
            }
        };
        let _ = script;
        let mut cmd = if let (true, Some(mb)) = (cfg!(unix), s.memory_limit_mb) {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg(format!("ulimit -v {} && exec \"$0\" \"$@\"", mb * 1024)).arg(&program);
            c
        } else {
            Command::new(&program)
        };
        cmd.args(args.drain(..))
            .current_dir(&dir)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", &dir)
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("MPLBACKEND", "Agg")
            .env("R_LIBS", std::env::join_paths(s.libraries).unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(fs::File::create(dir.join("stderr.txt"))?);
        let mut child = cmd.spawn().map_err(|e| Error::Unsupported(format!("cannot start {}: {e}", program.display())))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let (tx, replies) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(rest) = line.strip_prefix(MARK) {
                    if tx.send(rest.to_string()).is_err() {
                        break;
                    }
                }
            }
        });
        let mut c = Console { language: s.language, version: String::new(), child, stdin, replies, dir, next: 1, objects: vec![] };
        let ready = c.wait(0, timeout, &AtomicBool::new(false))?;
        if ready.get("ok").and_then(Json::as_bool) != Some(true) {
            return Err(Error::Validation(error_of(&ready).map(|e| e.0).unwrap_or_else(|| "the console did not start".into())));
        }
        c.version = ready.get("version").and_then(Json::as_str).unwrap_or("").to_string();
        c.objects = objects(&ready);
        Ok(c)
    }

    fn send(&mut self, cmd: &str, body: &str) -> Result<u64, Error> {
        let id = self.next;
        self.next += 1;
        write!(self.stdin, "{cmd} {id} {}\n{body}", body.len()).and_then(|_| self.stdin.flush()).map_err(|_| self.gone())?;
        Ok(id)
    }

    /// The reply to request `id`; interrupts the code when `cancel` is set
    /// or the time is up, and gives up if it does not stop.
    fn wait(&mut self, id: u64, timeout: Duration, cancel: &AtomicBool) -> Result<Json, Error> {
        let started = Instant::now();
        let mut interrupted: Option<Instant> = None;
        loop {
            match self.replies.recv_timeout(Duration::from_millis(20)) {
                Ok(line) => {
                    let j = Json::parse(&line).map_err(|_| Error::Validation("the console's reply could not be read".into()))?;
                    if j.get("id").and_then(Json::as_f64) == Some(id as f64) {
                        return Ok(j);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Err(self.gone()),
                Err(RecvTimeoutError::Timeout) => {}
            }
            let over = started.elapsed() >= timeout;
            if (cancel.load(Ordering::SeqCst) || over) && interrupted.is_none() {
                self.interrupt();
                interrupted = Some(Instant::now());
            }
            if interrupted.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
                let _ = self.child.kill();
                return Err(Error::Validation(if over {
                    format!("stopped after {:.0}s; the console was closed, so start it again", timeout.as_secs_f64())
                } else {
                    "the code did not stop when interrupted; the console was closed, so start it again".into()
                }));
            }
        }
    }

    /// Ask the running code to stop (Ctrl+C), as in RStudio or Jupyter.
    pub fn interrupt(&self) {
        #[cfg(unix)]
        {
            let _ = Command::new("kill").args(["-INT", &self.child.id().to_string()]).status();
        }
    }

    fn gone(&mut self) -> Error {
        let err = fs::read_to_string(self.dir.join("stderr.txt")).unwrap_or_default();
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        Error::Validation(if last.is_empty() { "the console stopped; start it again".into() } else { format!("the console stopped ({last}); start it again") })
    }

    /// Run code in the console.
    pub fn run(&mut self, code: &str, timeout: Duration, cancel: &AtomicBool) -> Result<Reply, Error> {
        let started = Instant::now();
        let id = self.send("RUN", code)?;
        let j = self.wait(id, timeout, cancel)?;
        let text = |k: &str| j.get(k).and_then(Json::as_str).unwrap_or("").to_string();
        let value = j.get("value").and_then(|v| match v {
            Json::Null => None,
            v => match v.get("table") {
                Some(t) => Some(Shown::Table(preview(t))),
                None => v.get("text").and_then(Json::as_str).map(|s| Shown::Text(s.to_string())),
            },
        });
        let images = j
            .get("images")
            .and_then(Json::as_arr)
            .map(|a| a.iter().filter_map(Json::as_str).map(PathBuf::from).filter(|p| p.starts_with(&self.dir) && p.is_file()).collect())
            .unwrap_or_default();
        self.objects = objects(&j);
        Ok(Reply {
            ok: j.get("ok").and_then(Json::as_bool) == Some(true),
            stdout: text("stdout"),
            stderr: text("stderr"),
            value,
            error: error_of(&j),
            images,
            objects: self.objects.clone(),
            elapsed: started.elapsed(),
        })
    }

    /// Write the console's `result` as a table (Parquet), to compare with the
    /// step's own result.
    pub fn save_result(&mut self, out: &Path, timeout: Duration) -> Result<(), Error> {
        let tmp = match self.language {
            Language::Python => out.to_path_buf(),
            Language::R => self.dir.join("result.csv"),
        };
        let id = self.send("SAVE", &tmp.to_string_lossy())?;
        let j = self.wait(id, timeout, &AtomicBool::new(false))?;
        if let Some((m, _)) = error_of(&j) {
            return Err(Error::Validation(m));
        }
        if self.language == Language::R {
            crate::rlang::read_r_table(&tmp, out)?;
        }
        Ok(())
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "EXIT 0 0");
        let _ = self.stdin.flush();
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline && matches!(self.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn error_of(j: &Json) -> Option<(String, Option<usize>)> {
    let e = j.get("error")?;
    let m = e.get("message").and_then(Json::as_str)?.to_string();
    Some((m, e.get("line").and_then(Json::as_f64).map(|l| l as usize)))
}

fn objects(j: &Json) -> Vec<Object> {
    j.get("env")
        .and_then(Json::as_arr)
        .map(|a| {
            a.iter()
                .map(|o| {
                    let s = |k: &str| o.get(k).and_then(Json::as_str).unwrap_or("").to_string();
                    Object { name: s("name"), kind: s("kind"), summary: s("summary") }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn preview(t: &Json) -> Preview {
    let columns = t
        .get("columns")
        .and_then(Json::as_arr)
        .map(|a| {
            a.iter()
                .map(|c| {
                    let p = c.as_arr().cloned().unwrap_or_default();
                    let s = |i: usize| p.get(i).and_then(Json::as_str).unwrap_or("").to_string();
                    (s(0), s(1))
                })
                .collect()
        })
        .unwrap_or_default();
    let rows = t.get("rows").and_then(Json::as_arr).map(|a| a.iter().map(|r| r.as_arr().cloned().unwrap_or_default()).collect()).unwrap_or_default();
    Preview { columns, rows, total: t.get("total").and_then(Json::as_f64).unwrap_or(0.0) as u64 }
}
