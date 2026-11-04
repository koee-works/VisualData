//! Editing a code step in the user's own tools: a Python step as a Jupyter
//! notebook, an R step as an .R file for RStudio. Each loads the step's real
//! inputs first, so the code runs there as it does in the app, and saving
//! there updates the step.
//!
//! Only files this app wrote, in the project's private work folder, are
//! watched, so an edit there is the user's own: it needs no review. (Code
//! that arrives in `analysis.json` from elsewhere still does.)

use crate::{fail, AnalysisView, App, AppError, Result};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use vda_core::json::Json;
use vda_core::model::StepKind;

const R_MARK: &str = "# ---- the step's code (below this line) ----";
const SETUP_TAG: &str = "vda-setup";

#[derive(Debug, Clone)]
pub(crate) struct External {
    pub path: PathBuf,
    /// The file as last written or read.
    pub seen: String,
    /// The step's code as last synced.
    pub code: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExternalView {
    pub step_id: String,
    pub path: String,
    /// "notebook" (.ipynb) or "R script".
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SyncView {
    /// Steps whose code changed in the other editor and were updated.
    pub updated: Vec<String>,
    pub view: Option<AnalysisView>,
}

fn py(s: &str) -> String {
    Json::str(s).to_string_compact()
}

/// The step's code from a notebook: its code cells, except the setup cell.
pub(crate) fn code_from_notebook(text: &str) -> Option<String> {
    let nb = Json::parse(text).ok()?;
    let cells = nb.get("cells")?.as_arr()?;
    let mut parts = Vec::new();
    for c in cells {
        if c.get("cell_type").and_then(Json::as_str) != Some("code") {
            continue;
        }
        let tags: Vec<&str> = c.get("metadata").and_then(|m| m.get("tags")).and_then(Json::as_arr).map(|t| t.iter().filter_map(Json::as_str).collect()).unwrap_or_default();
        if tags.contains(&SETUP_TAG) {
            continue;
        }
        let src = match c.get("source") {
            Some(Json::Arr(lines)) => lines.iter().filter_map(Json::as_str).collect::<String>(),
            Some(Json::Str(s)) => s.clone(),
            _ => String::new(),
        };
        if !src.trim().is_empty() {
            parts.push(src.trim_end().to_string());
        }
    }
    Some(parts.join("\n\n"))
}

/// The step's code from an R file: what follows the marker line.
pub(crate) fn code_from_r(text: &str) -> String {
    match text.find(R_MARK) {
        Some(i) => text[i + R_MARK.len()..].trim_start_matches(['\r', '\n']).trim_end().to_string(),
        None => text.trim_end().to_string(),
    }
}

fn lines(text: &str) -> Json {
    Json::Arr(text.split_inclusive('\n').map(Json::str).collect())
}

fn notebook(title: &str, setup: &str, code: &str) -> String {
    let cell = |kind: &str, src: &str, tags: &[&str]| {
        let mut meta = vec![];
        if !tags.is_empty() {
            meta.push(("tags", Json::Arr(tags.iter().map(|t| Json::str(*t)).collect())));
        }
        let mut c = vec![("cell_type", Json::str(kind)), ("metadata", Json::obj(meta)), ("source", lines(src))];
        if kind == "code" {
            c.push(("execution_count", Json::Null));
            c.push(("outputs", Json::Arr(vec![])));
        }
        Json::obj(c)
    };
    Json::obj(vec![
        (
            "cells",
            Json::Arr(vec![
                cell("markdown", &format!("### {title}\n\nSave this notebook to update the step in the app. The first code cell loads the step's inputs (`df`, `dfs`, `params`); it is not part of the step. Every other code cell is the step's code; it must assign a DataFrame to `result`."), &[]),
                cell("code", setup, &[SETUP_TAG]),
                cell("code", code, &[]),
            ]),
        ),
        (
            "metadata",
            Json::obj(vec![
                ("kernelspec", Json::obj(vec![("display_name", Json::str("Python 3")), ("language", Json::str("python")), ("name", Json::str("python3"))])),
                ("language_info", Json::obj(vec![("name", Json::str("python"))])),
            ]),
        ),
        ("nbformat", Json::Num(4.0)),
        ("nbformat_minor", Json::Num(5.0)),
    ])
    .to_string_pretty()
}

fn open_with_default_app(path: &Path) {
    let p = path.as_os_str();
    let _ = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(p).spawn()
    } else if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "start", ""]).arg(p).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(p).spawn()
    };
}

impl App {
    /// Write the step as a file for another editor (with its inputs) and,
    /// when `open`, open it with the computer's app for that kind of file.
    pub fn edit_externally(&mut self, step_id: &str, open: bool) -> Result<ExternalView> {
        let s = self.session()?;
        let step = s.analysis.step(step_id).ok_or_else(|| AppError::from(format!("unknown step `{step_id}`")))?.clone();
        let (is_r, code) = match &step.kind {
            StepKind::Python { code, .. } => (false, code.clone()),
            StepKind::R { code, .. } => (true, code.clone()),
            _ => return fail("only Python and R steps can be edited as a file"),
        };
        let (inputs, _) = s.input_files(step_id)?;
        let params = s.analysis.parameters.clone();
        let analysis_name = s.analysis.name.clone();
        let dir = s.project_dir().join(".work").join("edit").join(step_id);
        fs::create_dir_all(&dir)?;
        let dir = fs::canonicalize(&dir)?;
        let title = format!("{} — {}", step.label.clone().unwrap_or_else(|| step_id.to_string()), analysis_name);
        let (path, text) = if is_r {
            let mut ids = Vec::new();
            for (n, (_, p)) in inputs.iter().enumerate() {
                let types = vda_core::duck::parquet_to_csv(p, &dir.join(format!("in-{n}.csv")))?;
                let words: Vec<&str> = types.iter().map(|(_, t)| vda_core::rlang::type_word(*t)).collect();
                fs::write(dir.join(format!("in-{n}.types")), words.join("\n") + "\n")?;
                ids.push(format!("\"{n}\""));
            }
            let names: Vec<String> = inputs.iter().map(|(id, _)| vda_core::rlang::r_string(id)).collect();
            fs::write(dir.join("params.R"), format!("{}\ninput_names <- c({})\n", vda_core::rlang::params_r(&params), names.join(", ")))?;
            let setup = dir.join("setup.R");
            fs::write(
                &setup,
                format!(
                    "# Loads the step's inputs as the app gives them (df, dfs, params) into your session.\nlocal({{\n  owd <- setwd({dir}); on.exit(setwd(owd))\n{io}\n  ids <- c({ids})\n  dfs <- lapply(ids, read_input)\n  source(\"params.R\", local = TRUE)\n  names(dfs) <- input_names\n  assign(\"dfs\", dfs, envir = globalenv())\n  assign(\"df\", dfs[[1]], envir = globalenv())\n  assign(\"params\", params, envir = globalenv())\n}})\n",
                    dir = vda_core::rlang::r_string(&dir.to_string_lossy()),
                    io = vda_core::rlang::r_io(),
                    ids = ids.join(", ")
                ),
            )?;
            let text = format!(
                "# {title}\n# Saving this file updates the step in the app. The next line loads the step's\n# inputs (df, dfs, params); it is not part of the step. Assign a data frame to `result`.\nsource({})\n{R_MARK}\n{code}\n",
                vda_core::rlang::r_string(&setup.to_string_lossy())
            );
            (dir.join(format!("{step_id}.R")), text)
        } else {
            let mut loads = Vec::new();
            for (n, (id, p)) in inputs.iter().enumerate() {
                let copy = dir.join(format!("in-{n}.parquet"));
                fs::copy(p, &copy)?;
                loads.push(format!("    {}: pd.read_parquet({}, dtype_backend=\"numpy_nullable\"),", py(id), py(&copy.to_string_lossy())));
            }
            let setup = format!(
                "# The step's inputs, as the app gives them to it. Not part of the step.\nimport datetime, json\nimport pandas as pd\n\ndfs = {{\n{}\n}}\ndf = dfs[{}]\n_kinds = {{\"int\": int, \"date\": datetime.date.fromisoformat}}\nparams = {{name: _kinds.get(kind, lambda v: v)(v) for name, kind, v in json.loads({})}}",
                loads.join("\n"),
                py(&inputs[0].0),
                py(&vda_core::params::python_json(&params))
            );
            (dir.join(format!("{step_id}.ipynb")), notebook(&title, &setup, &code))
        };
        fs::write(&path, &text)?;
        self.external.insert(step_id.to_string(), crate::external::External { path: path.clone(), seen: text, code });
        if open {
            open_with_default_app(&path);
        }
        Ok(ExternalView { step_id: step_id.to_string(), path: path.to_string_lossy().into_owned(), kind: if is_r { "R script" } else { "notebook" }.into() })
    }

    /// Bring edits made in the other editors into their steps (and edits
    /// made in the app out to the files). Cheap when nothing changed.
    pub fn sync_external(&mut self) -> Result<SyncView> {
        let mut updated = Vec::new();
        let ids: Vec<String> = self.external.keys().cloned().collect();
        for id in ids {
            let ext = self.external[&id].clone();
            let Some(step) = self.session()?.analysis.step(&id).cloned() else {
                self.external.remove(&id);
                continue;
            };
            let (is_r, current, timeout) = match &step.kind {
                StepKind::Python { code, timeout_ms } => (false, code.clone(), *timeout_ms),
                StepKind::R { code, timeout_ms } => (true, code.clone(), *timeout_ms),
                _ => {
                    self.external.remove(&id);
                    continue;
                }
            };
            let Ok(text) = fs::read_to_string(&ext.path) else { continue };
            if text != ext.seen {
                let from_file = if is_r { Some(code_from_r(&text)) } else { code_from_notebook(&text) };
                // A notebook mid-save may not parse; try again next time.
                let Some(from_file) = from_file else { continue };
                if from_file != current {
                    let op = if is_r { serde_json::json!({"type": "r", "code": from_file, "timeout_ms": timeout}) } else { serde_json::json!({"type": "python", "code": from_file, "timeout_ms": timeout}) };
                    self.update_step(&id, step.label.clone(), &op, None)?;
                    updated.push(id.clone());
                }
                self.external.insert(id.clone(), External { path: ext.path.clone(), seen: text, code: from_file });
            } else if current != ext.code {
                // Changed in the app: write it out, so the file stays the step.
                let text = if is_r {
                    match text.find(R_MARK) {
                        Some(i) => format!("{}{R_MARK}\n{current}\n", &text[..i]),
                        None => format!("{current}\n"),
                    }
                } else {
                    let setup = Json::parse(&text)
                        .ok()
                        .and_then(|nb| {
                            nb.get("cells")?.as_arr()?.iter().find(|c| c.get("metadata").and_then(|m| m.get("tags")).and_then(Json::as_arr).is_some_and(|t| t.iter().any(|x| x.as_str() == Some(SETUP_TAG)))).cloned()
                        })
                        .and_then(|c| match c.get("source") {
                            Some(Json::Arr(l)) => Some(l.iter().filter_map(Json::as_str).collect::<String>()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    notebook(&step.label.clone().unwrap_or_else(|| id.clone()), &setup, &current)
                };
                fs::write(&ext.path, &text)?;
                self.external.insert(id.clone(), External { path: ext.path.clone(), seen: text, code: current });
            }
        }
        let view = if updated.is_empty() { None } else { Some(self.view()?) };
        Ok(SyncView { updated, view })
    }

    /// Stop watching a step's file (the file stays).
    pub fn stop_external(&mut self, step_id: &str) {
        self.external.remove(step_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_read_back_from_both_kinds_of_file() {
        let nb = notebook("t", "import x", "result = df\n");
        assert_eq!(code_from_notebook(&nb).as_deref(), Some("result = df"));
        // JupyterLab writes `source` as a list of lines; the setup cell is skipped.
        let edited = nb.replace("\"result = df\\n\"", "\"a = 1\\n\", \"result = df.head(a)\"");
        assert_eq!(code_from_notebook(&edited).as_deref(), Some("a = 1\nresult = df.head(a)"));
        assert_eq!(code_from_notebook("{not json"), None);
        let r = format!("# header\nsource(\"x\")\n{R_MARK}\nresult <- df\n\n");
        assert_eq!(code_from_r(&r), "result <- df");
        assert_eq!(code_from_r("result <- df\n"), "result <- df");
    }
}
