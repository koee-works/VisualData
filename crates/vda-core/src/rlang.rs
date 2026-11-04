//! Runs a custom R step in a child process (`Rscript`).
//!
//! The same lifecycle isolation as Python steps (ADR-0001): a separate
//! process started with `--vanilla` (no profile or saved workspace), a cleared
//! environment, a private working directory, a wall-clock timeout,
//! cooperative cancellation, and on Unix an address-space limit. It is not a
//! security sandbox.
//!
//! Contract with user code: the first input is the data frame `df`, every
//! input is in the named list `dfs` by id, parameters are in the list
//! `params`, and the code must assign a data frame to `result`.
//!
//! Data crosses the boundary as CSV with a file of exact column types, so
//! base R is enough (no arrow package needed). Numbers are written so they
//! read back exactly. Whole-number columns arrive as R integers when every
//! value fits (R integers stop at about 2.1 billion), otherwise as doubles.
//!
//! The user's own package libraries (what RStudio installs into) are found
//! once with their normal environment and passed to the step, so packages
//! such as dplyr work without the step seeing anything else.

use crate::table::{DataType, Value};
use crate::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Reading the step's inputs and writing its result, shared by the step
/// runner and the console (see `scratch.rs`).
pub(crate) const R_IO: &str = r#"
  finish <- function(code, msg = NULL) {
    if (!is.null(msg)) cat(msg, "\n", sep = "", file = stderr())
    flush(stdout()); flush(stderr())
    quit(save = "no", status = code, runLast = FALSE)
  }
  read_input <- function(id) {
    types <- readLines(paste0("in-", id, ".types"), encoding = "UTF-8")
    # Everything is read as text, then converted: quoted numbers are fine.
    d <- utils::read.csv(paste0("in-", id, ".csv"), colClasses = "character", na.strings = "\\N",
                         check.names = FALSE, stringsAsFactors = FALSE, encoding = "UTF-8")
    for (i in seq_along(types)) {
      t <- types[i]
      if (t == "BOOLEAN") {
        d[[i]] <- as.logical(d[[i]])
      } else if (t == "DOUBLE") {
        d[[i]] <- as.numeric(d[[i]])
      } else if (t == "BIGINT") {
        v <- as.numeric(d[[i]])
        d[[i]] <- v
        if (all(is.na(v) | abs(v) <= .Machine$integer.max)) d[[i]] <- as.integer(v)
      } else if (t == "DATE") {
        d[[i]] <- as.Date(d[[i]])
      } else if (t == "TIMESTAMP") {
        d[[i]] <- as.POSIXct(d[[i]], tz = "UTC", format = "%Y-%m-%d %H:%M:%OS")
      }
    }
    d
  }

  write_output <- function(d, out) {
    d <- as.data.frame(d, stringsAsFactors = FALSE, optional = TRUE)
    n <- ncol(d)
    types <- character(n)
    cols <- vector("list", n)
    for (i in seq_len(n)) {
      v <- d[[i]]
      if (is.factor(v)) v <- as.character(v)
      if (is.logical(v)) {
        t <- "BOOLEAN"; s <- ifelse(v, "true", "false")
      } else if (is.integer(v)) {
        t <- "BIGINT"; s <- as.character(v)
      } else if (inherits(v, "Date")) {
        t <- "DATE"; s <- format(v, "%Y-%m-%d")
      } else if (inherits(v, "POSIXt")) {
        t <- "TIMESTAMP"; s <- format(v, "%Y-%m-%d %H:%M:%OS6", tz = "UTC")
      } else if (is.numeric(v)) {
        t <- "DOUBLE"
        s <- formatC(v, digits = 15, format = "g")
        exact <- is.na(v) | is.infinite(v) | suppressWarnings(as.numeric(s)) == v
        s[!exact] <- formatC(v[!exact], digits = 17, format = "g")
        s[is.infinite(v)] <- ifelse(v[is.infinite(v)] > 0, "inf", "-inf")
        s[is.nan(v)] <- "nan"
      } else if (is.character(v)) {
        t <- "VARCHAR"; s <- v
      } else if (is.list(v)) {
        finish(3, sprintf("VDA_CONTRACT: column `%s` of `result` holds lists; make each value a single number, text or date", names(d)[i]))
      } else {
        t <- "VARCHAR"; s <- as.character(v)
      }
      s[is.na(v) & !is.nan(v)] <- NA
      types[i] <- t
      cols[[i]] <- s
    }
    names(cols) <- names(d)
    frame <- as.data.frame(cols, stringsAsFactors = FALSE, optional = TRUE)
    if (n > 0) names(frame) <- names(d)
    utils::write.table(frame, out, sep = ",", quote = TRUE, qmethod = "double", na = "\\N",
                       row.names = FALSE, col.names = names(d), fileEncoding = "UTF-8")
    writeLines(types, paste0(out, ".types"))
  }

"#;

const R_MAIN: &str = r#"
  args <- commandArgs(trailingOnly = TRUE)
  out <- args[1]; code_path <- args[2]; ids <- args[-(1:2)]

  setup <- tryCatch({
    dfs <- lapply(ids, read_input)
    source("params.R", local = TRUE)
    names(dfs) <- input_names
    exprs <- parse(code_path, keep.source = TRUE, encoding = "UTF-8")
    list(dfs = dfs, params = params, exprs = exprs)
  }, error = function(e) e)
  if (inherits(setup, "error")) {
    m <- conditionMessage(setup)
    # A syntax error in the step: "<file>:2:7: unexpected symbol".
    place <- regmatches(m, regexec(":([0-9]+):([0-9]+): ", m))[[1]]
    if (length(place) == 3) {
      what <- sub("^[^:]*:[0-9]+:[0-9]+: ", "", strsplit(m, "\n")[[1]][1])
      finish(1, sprintf("Error: syntax error: %s (line %s)", what, place[2]))
    }
    finish(4, paste0("VDA_SETUP: could not prepare the R step: ", m))
  }

  env <- new.env(parent = globalenv())
  env$dfs <- setup$dfs
  env$df <- setup$dfs[[1]]
  env$params <- setup$params
  refs <- attr(setup$exprs, "srcref")
  for (i in seq_along(setup$exprs)) {
    line <- if (!is.null(refs)) refs[[i]][1] else i
    ok <- tryCatch({
      withCallingHandlers(
        eval(setup$exprs[[i]], env),
        # library()'s banner ("Attaching package ...") is not the step's output.
        packageStartupMessage = function(m) invokeRestart("muffleMessage"),
        warning = function(w) {
          cat(sprintf("Warning (line %d): %s\n", line, conditionMessage(w)), file = stderr())
          invokeRestart("muffleWarning")
        }
      )
      TRUE
    }, error = function(e) {
      cat(sprintf("Error: %s (line %d)\n", conditionMessage(e), line), file = stderr())
      FALSE
    })
    if (!ok) finish(1)
  }
  result <- get0("result", envir = env, inherits = FALSE)
  if (!is.data.frame(result)) {
    got <- if (is.null(result)) "nothing" else class(result)[1]
    finish(3, sprintf("VDA_CONTRACT: the step must assign a data frame to `result` (got %s)", got))
  }
  saved <- tryCatch({ write_output(result, out); TRUE }, error = function(e) {
    cat("VDA_CONTRACT: `result` could not be saved as a table: ", conditionMessage(e), "\n", sep = "", file = stderr())
    FALSE
  })
  finish(if (saved) 0 else 3)
"#;

/// The script that runs an R step (also embedded in exported notebooks).
pub(crate) fn wrapper() -> String {
    format!("local({{\n{R_IO}\n{R_MAIN}\n}})\n")
}

/// `Rscript` to use when nothing else is chosen: `$VDA_R`, or `Rscript`.
pub fn default_rscript() -> PathBuf {
    std::env::var_os("VDA_R").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("Rscript"))
}

/// What an R installation is and has.
#[derive(Debug, Clone, PartialEq)]
pub struct REnvironment {
    pub rscript: PathBuf,
    pub version: Option<String>,
    /// The package libraries the step will see.
    pub libraries: Vec<String>,
    /// Package, what it is for, installed version.
    pub packages: Vec<(String, String, Option<String>)>,
    pub problem: Option<String>,
}

/// Packages worth reporting, and what each is for. None is required.
pub const R_PACKAGES: &[(&str, &str)] = &[
    ("dplyr", "data manipulation"),
    ("tidyr", "reshaping"),
    ("ggplot2", "plots"),
    ("arrow", "faster data exchange (not used yet)"),
    ("IRkernel", "running exported notebooks in Jupyter"),
];

#[derive(Debug, Clone)]
pub struct RRunner {
    pub rscript: PathBuf,
    pub memory_limit_mb: Option<u64>,
    pub work_root: PathBuf,
    /// Found once from the user's environment (see the module notes).
    libraries: Option<Vec<String>>,
}

static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// What an R step ran to.
pub struct RRun {
    pub result: Result<(), Error>,
    /// stdout, then warnings and errors.
    pub output: String,
}

impl RRunner {
    pub fn new(rscript: impl Into<PathBuf>, work_root: impl Into<PathBuf>) -> Self {
        RRunner { rscript: rscript.into(), memory_limit_mb: Some(4096), work_root: work_root.into(), libraries: None }
    }

    /// The installation's version, libraries and packages. Never fails.
    pub fn environment(&mut self) -> REnvironment {
        let names: Vec<String> = R_PACKAGES.iter().map(|p| format!("'{}'", p.0)).collect();
        let script = format!(
            "cat('version', paste(R.version$major, R.version$minor, sep = '.'), '\\n', sep = '\\t'); \
             for (l in .libPaths()) cat('lib', l, '\\n', sep = '\\t'); \
             for (p in c({})) cat('pkg', p, tryCatch(as.character(utils::packageVersion(p)), error = function(e) ''), '\\n', sep = '\\t')",
            names.join(", ")
        );
        let mut env = REnvironment { rscript: self.rscript.clone(), version: None, libraries: vec![], packages: vec![], problem: None };
        // The user's own environment here (not cleared): that is where
        // R_LIBS_USER and ~/.Renviron say their packages are.
        match Command::new(crate::python::resolve(&self.rscript)).args(["-e", &script]).stdin(Stdio::null()).output() {
            Err(e) => env.problem = Some(format!("cannot start {}: {e}", self.rscript.display())),
            Ok(o) if !o.status.success() => {
                env.problem = Some(format!("{} did not run: {}", self.rscript.display(), String::from_utf8_lossy(&o.stderr).trim().lines().last().unwrap_or("")))
            }
            Ok(o) => {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    let parts: Vec<&str> = line.split('\t').collect();
                    match parts.as_slice() {
                        ["version", v, ..] => env.version = Some(v.to_string()),
                        ["lib", l, ..] => env.libraries.push(l.to_string()),
                        ["pkg", p, v, ..] => {
                            let why = R_PACKAGES.iter().find(|x| x.0 == *p).map(|x| x.1).unwrap_or("");
                            env.packages.push((p.to_string(), why.to_string(), Some(v.to_string()).filter(|v| !v.is_empty())));
                        }
                        _ => {}
                    }
                }
                if env.version.is_none() {
                    env.problem = Some("the R installation's answer could not be read".into());
                }
            }
        }
        self.libraries = Some(env.libraries.clone());
        env
    }

    /// The package libraries R steps see (found once).
    pub fn libraries(&mut self) -> Vec<String> {
        if self.libraries.is_none() {
            self.environment();
        }
        self.libraries.clone().unwrap_or_default()
    }

    /// R's version and the versions of the reported packages; part of every
    /// R step's cache key. `None` when R cannot run.
    pub fn fingerprint(&mut self) -> Option<String> {
        let env = self.environment();
        let version = env.version.filter(|_| env.problem.is_none())?;
        let pkgs: Vec<String> = env.packages.iter().filter_map(|(p, _, v)| v.as_ref().map(|v| format!("{p} {v}"))).collect();
        Some(format!("R {version}{}{}", if pkgs.is_empty() { "" } else { " | " }, pkgs.join(", ")))
    }

    /// Run `code` with Parquet inputs (the first is `df`), writing `result`
    /// to the Parquet file `output`. `params` is R source from [`params_r`].
    pub fn run(&mut self, code: &str, inputs: &[(String, PathBuf)], params: &str, output: &Path, timeout: Duration, cancel: &AtomicBool) -> RRun {
        let fail = |e: Error| RRun { result: Err(e), output: String::new() };
        if inputs.is_empty() {
            return fail(Error::R("an R step needs at least one input".into()));
        }
        if self.libraries.is_none() {
            self.environment();
        }
        let seq = RUN_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = self.work_root.join(format!("r-{}-{seq}", std::process::id()));
        let dir = match fs::create_dir_all(&dir).and_then(|_| fs::canonicalize(&dir)) {
            Ok(d) => d,
            Err(e) => return fail(e.into()),
        };
        let mut output_text = String::new();
        let result = self.run_in(&dir, code, inputs, params, output, timeout, cancel, &mut output_text);
        let _ = fs::remove_dir_all(&dir);
        RRun { result, output: output_text }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_in(
        &self,
        dir: &Path,
        code: &str,
        inputs: &[(String, PathBuf)],
        params: &str,
        output: &Path,
        timeout: Duration,
        cancel: &AtomicBool,
        printed: &mut String,
    ) -> Result<(), Error> {
        let mut ids = Vec::new();
        for (n, (id, path)) in inputs.iter().enumerate() {
            // File names from a counter: ids are the user's words.
            let file_id = format!("{n}");
            let types = crate::duck::parquet_to_csv(path, &dir.join(format!("in-{file_id}.csv")))?;
            fs::write(dir.join(format!("in-{file_id}.types")), types.iter().map(|(_, t)| type_word(*t)).collect::<Vec<_>>().join("\n") + "\n")?;
            ids.push((file_id, id.clone()));
        }
        // The list `dfs` is named by the inputs' ids.
        let names: Vec<String> = ids.iter().map(|(_, id)| r_string(id)).collect();
        fs::write(dir.join("params.R"), format!("{params}\ninput_names <- c({})\n", names.join(", ")))?;
        let code_path = dir.join("step.R");
        fs::write(&code_path, code)?;
        let wrapper = dir.join("wrapper.R");
        fs::write(&wrapper, self::wrapper())?;
        let out_csv = dir.join("result.csv");
        let (err_path, out_path) = (dir.join("stderr.txt"), dir.join("stdout.txt"));

        let mut cmd = if let (true, Some(mb)) = (cfg!(unix), self.memory_limit_mb) {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg(format!("ulimit -v {} && exec \"$0\" --vanilla \"$@\"", mb * 1024)).arg(crate::python::resolve(&self.rscript));
            c
        } else {
            let mut c = Command::new(crate::python::resolve(&self.rscript));
            c.arg("--vanilla");
            c
        };
        cmd.arg(&wrapper).arg(&out_csv).arg(&code_path);
        for (file_id, _) in &ids {
            cmd.arg(file_id);
        }
        let libs = self.libraries.clone().unwrap_or_default();
        cmd.current_dir(dir)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", dir)
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("R_LIBS", std::env::join_paths(&libs).unwrap_or_default())
            .stdin(Stdio::null())
            .stdout(fs::File::create(&out_path)?)
            .stderr(fs::File::create(&err_path)?);

        let start = Instant::now();
        let mut child = cmd.spawn().map_err(|e| Error::R(format!("cannot start R ({}): {e}", self.rscript.display())))?;
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
                return Err(Error::R(format!("timed out after {:.1}s and was stopped", timeout.as_secs_f64())));
            }
            std::thread::sleep(Duration::from_millis(15));
        };
        let stderr = fs::read_to_string(&err_path).unwrap_or_default();
        *printed = crate::python::step_output(&fs::read(&out_path).unwrap_or_default(), &stderr);
        if !status.success() {
            return Err(Error::R(summarize(&stderr, status.code())));
        }
        read_r_table(&out_csv, output)
    }
}

/// A table R wrote (CSV beside a `.types` file) as Parquet.
pub(crate) fn read_r_table(csv: &Path, output: &Path) -> Result<(), Error> {
    let mut types_path = csv.as_os_str().to_owned();
    types_path.push(".types");
    let words = fs::read_to_string(PathBuf::from(types_path)).unwrap_or_default();
    let header = read_header(csv)?;
    let types: Vec<DataType> = words.lines().map(parse_type).collect::<Option<_>>().ok_or_else(|| Error::R("the result's types could not be read".into()))?;
    if types.len() != header.len() {
        return Err(Error::R("the result's columns could not be read".into()));
    }
    crate::duck::csv_to_parquet(csv, &header.into_iter().zip(types).collect::<Vec<_>>(), output)
}

pub(crate) fn type_word(t: DataType) -> &'static str {
    match t {
        DataType::Bool => "BOOLEAN",
        DataType::Int => "BIGINT",
        DataType::Float => "DOUBLE",
        DataType::Text => "VARCHAR",
        DataType::Date => "DATE",
        DataType::Timestamp => "TIMESTAMP",
    }
}

fn parse_type(w: &str) -> Option<DataType> {
    Some(match w.trim() {
        "BOOLEAN" => DataType::Bool,
        "BIGINT" => DataType::Int,
        "DOUBLE" => DataType::Float,
        "VARCHAR" => DataType::Text,
        "DATE" => DataType::Date,
        "TIMESTAMP" => DataType::Timestamp,
        _ => return None,
    })
}

/// Column names from the first line of the CSV R wrote (every name quoted).
fn read_header(csv: &Path) -> Result<Vec<String>, Error> {
    use std::io::BufRead;
    let mut first = String::new();
    std::io::BufReader::new(fs::File::open(csv)?).read_line(&mut first)?;
    let line = first.trim_end_matches(['\n', '\r']);
    let mut names = Vec::new();
    let mut chars = line.chars().peekable();
    while chars.peek().is_some() {
        let mut name = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        name.push('"');
                    } else {
                        break;
                    }
                } else {
                    name.push(c);
                }
            }
        }
        for c in chars.by_ref() {
            if c == ',' {
                break;
            }
            name.push(c);
        }
        names.push(name);
    }
    Ok(names)
}

/// An R string literal.
pub(crate) fn r_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The analysis parameters as R source defining the list `params`.
pub fn params_r(params: &[crate::params::Parameter]) -> String {
    let items: Vec<String> = params
        .iter()
        .map(|p| {
            let v = match &p.value {
                Value::Int(i) if i.unsigned_abs() <= i32::MAX as u64 => format!("{i}L"),
                Value::Int(i) => format!("{i}"),
                Value::Float(f) if f.is_nan() => "NaN".into(),
                Value::Float(f) if f.is_infinite() => if *f > 0.0 { "Inf".into() } else { "-Inf".into() },
                Value::Float(f) => format!("{f:?}"),
                Value::Bool(b) => if *b { "TRUE".into() } else { "FALSE".into() },
                Value::Date(_) => format!("as.Date({})", r_string(&p.value.to_string())),
                Value::Null => "NA".into(),
                v => r_string(&v.to_string()),
            };
            format!("`{}` = {v}", p.name.replace('`', ""))
        })
        .collect();
    format!("params <- list({})", items.join(", "))
}

/// Keep the useful part of what R printed when the step failed.
fn summarize(stderr: &str, code: Option<i32>) -> String {
    if let Some(line) = stderr.lines().find(|l| l.starts_with("VDA_CONTRACT: ")) {
        return line.trim_start_matches("VDA_CONTRACT: ").to_string();
    }
    if let Some(line) = stderr.lines().find(|l| l.starts_with("VDA_SETUP: ")) {
        return line.trim_start_matches("VDA_SETUP: ").to_string();
    }
    if let Some(line) = stderr.lines().rev().find(|l| l.starts_with("Error: ")) {
        return line.trim_start_matches("Error: ").to_string();
    }
    if stderr.contains("cannot allocate") {
        return "ran out of memory (limit reached)".into();
    }
    stderr.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).unwrap_or_else(|| format!("exited with code {code:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_and_literals() {
        let d = std::env::temp_dir().join(format!("vda-r-header-{}", std::process::id()));
        fs::write(&d, "\"a\",\"b,c\",\"say \"\"hi\"\"\"\n1,2,3\n").unwrap();
        assert_eq!(read_header(&d).unwrap(), ["a", "b,c", "say \"hi\""]);
        let _ = fs::remove_file(&d);
        assert_eq!(r_string("a\"b\\c\n"), r#""a\"b\\c\n""#);
        let p = |name: &str, value: Value| crate::params::Parameter { name: name.into(), value };
        assert_eq!(
            params_r(&[p("n", Value::Int(3)), p("big", Value::Int(5_000_000_000)), p("goal", Value::Float(0.5)), p("on", Value::Bool(true))]),
            "params <- list(`n` = 3L, `big` = 5000000000, `goal` = 0.5, `on` = TRUE)"
        );
    }
}
