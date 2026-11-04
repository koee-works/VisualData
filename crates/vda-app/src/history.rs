//! Undo, redo and the saved history of an analysis.
//!
//! Every saved change goes through `App::commit`, which records it here:
//! - **Undo and redo** for this session, in memory (the last
//!   [`MAX_UNDO`] changes). Undo and redo are themselves saved changes.
//! - **History** in the project's `history/` folder: one file per saved
//!   version (the last [`MAX_VERSIONS`]), each with what changed and when,
//!   so earlier versions can be restored after the app is closed, and by
//!   anyone the project is shared with. Restoring is a change like any
//!   other, so it can be undone.
//!
//! Approvals are kept by code fingerprint, so going back to code the user
//! has not approved asks for review again, and going forward to code they
//! wrote stays approved.

use crate::{fail, App, AnalysisView, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use vda_core::json::Json;
use vda_core::model::{Analysis, Step};
use vda_core::persist;

pub const MAX_UNDO: usize = 100;
pub const MAX_VERSIONS: usize = 500;

#[derive(Debug, Clone)]
pub struct Edit {
    /// The analysis on the other side of the change.
    pub analysis: Analysis,
    pub what: String,
}

#[derive(Debug, Clone, Default)]
pub struct Undo {
    pub undo: Vec<Edit>,
    pub redo: Vec<Edit>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VersionView {
    pub version: u64,
    pub time_unix_ms: u64,
    pub what: String,
    /// The version the project is at now.
    pub current: bool,
}

fn title(s: &Step) -> String {
    vda_core::describe::title(s)
}

/// What changed between two versions, in a few words.
pub fn describe(before: &Analysis, after: &Analysis) -> String {
    let mut parts: Vec<String> = Vec::new();
    if before.name != after.name {
        parts.push(format!("Renamed the analysis to \"{}\"", after.name));
    }
    let ids = |a: &Analysis| a.sources.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
    let (bs, as_) = (ids(before), ids(after));
    let added: Vec<&String> = as_.iter().filter(|i| !bs.contains(i)).collect();
    let removed: Vec<&String> = bs.iter().filter(|i| !as_.contains(i)).collect();
    if added.len() == 1 && removed.len() == 1 && bs.len() == as_.len() {
        parts.push(format!("Renamed source `{}` to `{}`", removed[0], added[0]));
    } else {
        for i in added {
            let path = after.source(i).map(|s| s.path.clone()).unwrap_or_default();
            parts.push(format!("Added source {path}"));
        }
        for i in removed {
            let path = before.source(i).map(|s| s.path.clone()).unwrap_or_default();
            parts.push(format!("Removed source {path}"));
        }
    }
    for s in &after.sources {
        if let Some(b) = before.source(&s.id) {
            if b.query != s.query {
                parts.push(match &s.query {
                    Some(_) => format!("Changed the query reading {}", s.path),
                    None => format!("Read table {} of {}", s.sheet.as_deref().unwrap_or("(first)"), s.path),
                });
            } else if b.sheet != s.sheet {
                let what = if s.format.is_database() { "table" } else { "sheet" };
                parts.push(format!("Chose {what} {} of {}", s.sheet.as_deref().unwrap_or("1"), s.path));
            }
        }
    }
    let mut renamed_source = false;
    if parts.iter().any(|p| p.starts_with("Renamed source")) {
        renamed_source = true;
    }
    for s in &after.steps {
        match before.step(&s.id) {
            None => parts.push(format!("Added {}", title(s))),
            Some(b) if b != s && !renamed_source => parts.push(format!("Edited {}", title(s))),
            _ => {}
        }
    }
    for s in &before.steps {
        if after.step(&s.id).is_none() {
            parts.push(format!("Deleted {}", title(s)));
        }
    }
    if before.parameters != after.parameters {
        let names = |a: &Analysis| a.parameters.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        let (bn, an) = (names(before), names(after));
        if bn.len() == an.len() && bn != an && !renamed_source {
            parts.push("Renamed a parameter".into());
        } else {
            for p in &after.parameters {
                match before.parameters.iter().find(|q| q.name == p.name) {
                    None => parts.push(format!("Added parameter {}", p.name)),
                    Some(q) if q.value != p.value => parts.push(format!("Set {} to {}", p.name, p.value)),
                    _ => {}
                }
            }
            for q in &before.parameters {
                if !an.contains(&q.name) {
                    parts.push(format!("Removed parameter {}", q.name));
                }
            }
        }
    }
    // Adding or deleting a step changes the cells too; that is part of it.
    let steps_changed = before.steps.len() != after.steps.len();
    if before.cells != after.cells && !steps_changed {
        parts.push("Edited the notebook".into());
    }
    match parts.len() {
        0 => "Saved".into(),
        1..=2 => parts.join("; "),
        n => format!("{}; and {} more changes", parts[..2].join("; "), n - 2),
    }
}

/// The source renamed between two versions, if exactly one was.
fn renamed_source(before: &Analysis, after: &Analysis) -> Option<(String, String)> {
    let gone: Vec<&str> = before.sources.iter().map(|s| s.id.as_str()).filter(|i| !after.is_source(i)).collect();
    let new: Vec<&str> = after.sources.iter().map(|s| s.id.as_str()).filter(|i| !before.is_source(i)).collect();
    match (&gone[..], &new[..]) {
        ([old], [new]) if before.sources.len() == after.sources.len() => Some((old.to_string(), new.to_string())),
        _ => None,
    }
}

fn history_dir(project: &Path) -> PathBuf {
    project.join("history")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Saved versions, oldest first: (version, path).
fn versions(project: &Path) -> Vec<(u64, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(history_dir(project)) else { return vec![] };
    let mut v: Vec<(u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name.strip_suffix(".json")?.parse::<u64>().ok()?;
            Some((n, e.path()))
        })
        .collect();
    v.sort();
    v
}

fn write_version(project: &Path, n: u64, what: &str, a: &Analysis) -> Result<()> {
    let dir = history_dir(project);
    std::fs::create_dir_all(&dir)?;
    let doc = Json::obj(vec![
        ("version", Json::Num(n as f64)),
        ("time_unix_ms", Json::Num(now_ms() as f64)),
        ("what", Json::str(what)),
        ("analysis", persist::analysis_to_json(a)),
    ]);
    let path = dir.join(format!("{n:06}.json"));
    let tmp = dir.join(format!("{n:06}.json.tmp"));
    std::fs::write(&tmp, doc.to_string_pretty())?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn read_version(path: &Path) -> Result<(u64, u64, String, Json)> {
    let j = Json::parse(&std::fs::read_to_string(path)?)?;
    let num = |k: &str| j.get(k).and_then(Json::as_f64).unwrap_or(0.0) as u64;
    let what = j.get("what").and_then(Json::as_str).unwrap_or("").to_string();
    let a = j.get("analysis").cloned().unwrap_or(Json::Null);
    Ok((num("version"), num("time_unix_ms"), what, a))
}

/// Record a saved change in the project's history. The first change of a
/// project with no history also records how it was before.
pub fn record(project: &Path, before: &Analysis, after: &Analysis, what: &str) -> Result<()> {
    let existing = versions(project);
    let mut next = existing.last().map(|v| v.0 + 1).unwrap_or(1);
    if existing.is_empty() {
        write_version(project, next, "As first opened", before)?;
        next += 1;
    }
    write_version(project, next, what, after)?;
    let all = versions(project);
    if all.len() > MAX_VERSIONS {
        for (_, p) in &all[..all.len() - MAX_VERSIONS] {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(())
}

impl App {
    /// After a change is saved: describe it, remember it for undo, and add it
    /// to the history. `undoing` is set for undo and redo themselves.
    pub(crate) fn after_commit(&mut self, before: Analysis, after: &Analysis, undoing: Option<bool>) -> Result<()> {
        // A restore is named as one, with what it changed.
        let what = match self.restoring.take() {
            Some(v) => format!("Restored version {v} ({})", describe(&before, after)),
            None => describe(&before, after),
        };
        // A renamed source's comments follow it, also when a rename is undone.
        if let Some((old, new)) = renamed_source(&before, after) {
            self.retarget_comments(&old, &new)?;
        }
        let project = self.session()?.project_dir().to_path_buf();
        match undoing {
            None => {
                self.edits.undo.push(Edit { analysis: before, what: what.clone() });
                if self.edits.undo.len() > MAX_UNDO {
                    self.edits.undo.remove(0);
                }
                self.edits.redo.clear();
                // History is a convenience: the change itself is already saved.
                let _ = record(&project, &self.edits.undo.last().unwrap().analysis, after, &what);
            }
            Some(true) => {
                let what = format!("Undid: {}", self.edits.redo.last().map(|e| e.what.as_str()).unwrap_or("a change"));
                let _ = record(&project, &before, after, &what);
            }
            Some(false) => {
                let what = format!("Redid: {}", self.edits.undo.last().map(|e| e.what.as_str()).unwrap_or("a change"));
                let _ = record(&project, &before, after, &what);
            }
        }
        Ok(())
    }

    /// Go back one change. The steps it touched become outdated (or reuse
    /// the results saved for that version).
    pub fn undo(&mut self) -> Result<AnalysisView> {
        let Some(edit) = self.edits.undo.pop() else { return fail("there is nothing to undo") };
        let current = self.session()?.analysis.clone();
        self.edits.redo.push(Edit { analysis: current, what: edit.what.clone() });
        self.commit_as(edit.analysis, Some(true))
    }

    pub fn redo(&mut self) -> Result<AnalysisView> {
        let Some(edit) = self.edits.redo.pop() else { return fail("there is nothing to redo") };
        let current = self.session()?.analysis.clone();
        self.edits.undo.push(Edit { analysis: current, what: edit.what.clone() });
        self.commit_as(edit.analysis, Some(false))
    }

    /// Saved versions, newest first.
    pub fn history(&mut self, limit: usize) -> Result<Vec<VersionView>> {
        let project = self.session()?.project_dir().to_path_buf();
        let current = persist::analysis_to_json(&self.session()?.analysis);
        let mut out = Vec::new();
        let all = versions(&project);
        let mut marked = false;
        for (_, path) in all.iter().rev().take(limit) {
            let Ok((version, time_unix_ms, what, a)) = read_version(path) else { continue };
            let is_current = !marked && a == current;
            marked |= is_current;
            out.push(VersionView { version, time_unix_ms, what, current: is_current });
        }
        Ok(out)
    }

    /// Make an earlier version current again (a change that can be undone).
    pub fn restore_version(&mut self, version: u64) -> Result<AnalysisView> {
        let project = self.session()?.project_dir().to_path_buf();
        let Some((_, path)) = versions(&project).into_iter().find(|v| v.0 == version) else {
            return fail(format!("there is no saved version {version}"));
        };
        let (_, _, _, doc) = read_version(&path)?;
        let a = persist::analysis_from_json(&doc)?;
        self.restoring = Some(version);
        // Sources must still be in the folder: history keeps definitions, not data.
        for src in &a.sources {
            let p = if Path::new(&src.path).is_absolute() { PathBuf::from(&src.path) } else { project.join(&src.path) };
            if !p.exists() {
                return fail(format!("version {version} reads {}, which is no longer in the project folder", src.path));
            }
        }
        let r = self.commit(a);
        self.restoring = None;
        r
    }
}
