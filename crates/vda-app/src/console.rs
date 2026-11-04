//! The console beside a Python or R step, through the app's commands.

use crate::{fail, to_serde, App, Result};
use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use vda_core::console::{Console, Language, Shown, Start};
use vda_core::model::StepKind;

/// Longest a single run in the console may take before it is interrupted.
const RUN_LIMIT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ObjectView {
    pub name: String,
    pub kind: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConsoleView {
    pub step_id: String,
    /// "python" or "r".
    pub language: String,
    pub version: String,
    pub objects: Vec<ObjectView>,
    /// Inputs whose results are from before a change upstream.
    pub outdated: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TablePreview {
    pub columns: Vec<(String, String)>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConsoleReply {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub table: Option<TablePreview>,
    pub text: Option<String>,
    pub error: Option<String>,
    /// Line of the error in the code that was run (1-based).
    pub line: Option<usize>,
    /// Plots as `data:image/png;base64,...` URLs.
    pub images: Vec<String>,
    pub objects: Vec<ObjectView>,
    pub elapsed_ms: u64,
}

fn objects(o: &[vda_core::console::Object]) -> Vec<ObjectView> {
    o.iter().map(|o| ObjectView { name: o.name.clone(), kind: o.kind.clone(), summary: o.summary.clone() }).collect()
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl App {
    /// Start a console for a Python or R step, with its inputs loaded. A
    /// console already open (for any step) is closed first.
    pub fn console_start(&mut self, step_id: &str) -> Result<ConsoleView> {
        self.console = None;
        let interpreter = self.interpreter.clone();
        let s = self.session()?;
        let step = s.analysis.step(step_id).ok_or_else(|| crate::AppError::from(format!("unknown step `{step_id}`")))?.clone();
        let language = match step.kind {
            StepKind::Python { .. } => Language::Python,
            StepKind::R { .. } => Language::R,
            _ => return fail("only Python and R steps have a console"),
        };
        let (inputs, outdated) = s.input_files(step_id)?;
        let params = s.analysis.parameters.clone();
        let work = s.project_dir().join(".work");
        let (program, libraries) = match language {
            Language::Python => (interpreter, vec![]),
            Language::R => (s.r_program().unwrap_or_else(vda_core::rlang::default_rscript), s.r_runner_libraries()),
        };
        let console = Console::start(
            Start { language, program: &program, inputs: &inputs, params: &params, libraries: &libraries, work_root: &work, memory_limit_mb: Some(4096) },
            Duration::from_secs(120),
        )?;
        let view = ConsoleView {
            step_id: step_id.to_string(),
            language: if language == Language::R { "r" } else { "python" }.into(),
            version: console.version.clone(),
            objects: objects(&console.objects),
            outdated,
        };
        self.console = Some((step_id.to_string(), console, Arc::new(AtomicBool::new(false))));
        Ok(view)
    }

    /// Set to interrupt the code running in the console (from another thread).
    pub fn console_interrupt_handle(&mut self) -> Option<Arc<AtomicBool>> {
        let (_, _, flag) = self.console.as_mut()?;
        let fresh = Arc::new(AtomicBool::new(false));
        *flag = fresh.clone();
        Some(fresh)
    }

    pub fn console_run(&mut self, code: &str) -> Result<ConsoleReply> {
        let Some((_, console, flag)) = self.console.as_mut() else { return fail("start the console first") };
        let flag = flag.clone();
        let r = match console.run(code, RUN_LIMIT, &flag) {
            Ok(r) => r,
            Err(e) => {
                // The process is gone (stopped, or did not stop when asked).
                self.console = None;
                return Err(e.into());
            }
        };
        let (table, text) = match r.value {
            Some(Shown::Table(t)) => (
                Some(TablePreview {
                    columns: t.columns,
                    rows: t.rows.iter().map(|row| row.iter().map(to_serde).collect()).collect(),
                    total: t.total,
                }),
                None,
            ),
            Some(Shown::Text(s)) => (None, Some(s)),
            None => (None, None),
        };
        Ok(ConsoleReply {
            ok: r.ok,
            stdout: r.stdout,
            stderr: r.stderr,
            table,
            text,
            line: r.error.as_ref().and_then(|e| e.1),
            error: r.error.map(|e| e.0),
            images: r.images.iter().filter_map(|p| std::fs::read(p).ok()).map(|b| format!("data:image/png;base64,{}", base64(&b))).collect(),
            objects: objects(&r.objects),
            elapsed_ms: r.elapsed.as_millis() as u64,
        })
    }

    pub fn console_stop(&mut self) {
        self.console = None;
    }

    /// After the step has run: does its result match `result` in its
    /// console? `None` when there is nothing to compare (no console, no
    /// `result` there, or the step has no current result).
    pub fn console_compare(&mut self, step_id: &str) -> Result<Option<String>> {
        let Some((id, _, _)) = &self.console else { return Ok(None) };
        if id != step_id {
            return Ok(None);
        }
        let s = self.session()?;
        if s.state(step_id)? != vda_core::session::StepState::Fresh {
            return Ok(None);
        }
        let Some(step_file) = s.output(step_id).and_then(vda_core::session::Output::as_dataset).map(|d| d.path.clone()) else { return Ok(None) };
        let out = s.project_dir().join(".work").join(format!("console-result-{}.parquet", std::process::id()));
        let (_, console, _) = self.console.as_mut().expect("checked");
        if console.save_result(&out, Duration::from_secs(60)).is_err() {
            return Ok(None);
        }
        let differs = vda_core::duck::compare_tables(&step_file, &out);
        let _ = std::fs::remove_file(&out);
        Ok(differs?.map(|d| {
            format!(
                "The saved step's result differs from `result` in the console ({}). The console may hold something the code no longer makes, such as a variable from a line since changed. Restart the console to start clean.",
                d.replace("the first", "the step").replace("the second", "the console")
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
}
