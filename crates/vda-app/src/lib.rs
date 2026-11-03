//! Commands the desktop UI can call, as plain Rust functions over `vda-core`
//! (ADR-0003). The Tauri shell only forwards to [`App`]; everything here is
//! tested with `cargo test` and no window.
//!
//! Wire format:
//! - Steps travel as the same JSON used in `analysis.json` (`persist`), so
//!   there is one step format everywhere.
//! - Table data travels a page at a time as typed [`Cell`]s.

use serde::{Deserialize, Serialize};
use serde_json::Value as J;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use vda_core::compile;
use vda_core::duck::{DuckDbEngine, EngineOptions};
use vda_core::json::Json;
use vda_core::model::{Analysis, Cell as NoteCell, Source, Step, StepKind, SOURCE_ID};
use vda_core::persist;
use vda_core::python::{default_interpreter, PythonRunner};
use vda_core::review::{self, Approvals, ReviewPolicy};
use vda_core::session::{Output, RunStatus, Session, StepRun, StepState};
use vda_core::table::{Table, Value};

/// The most rows one page may carry over IPC.
pub const MAX_PAGE_ROWS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppError {
    pub message: String,
}

impl<E: std::fmt::Display> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError { message: e.to_string() }
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(AppError { message: message.into() })
}

/// A table value with its type, so the UI formats it like the core does.
/// Integers are strings to keep 64-bit precision in JavaScript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "lowercase")]
pub enum Cell {
    Null,
    Bool(bool),
    Int(String),
    Float(f64),
    Text(String),
    Date(String),
    Timestamp(String),
}

impl From<&Value> for Cell {
    fn from(v: &Value) -> Self {
        match v {
            Value::Null => Cell::Null,
            Value::Bool(b) => Cell::Bool(*b),
            Value::Int(i) => Cell::Int(i.to_string()),
            // JSON has no NaN or infinity; send those as text.
            Value::Float(f) if f.is_finite() => Cell::Float(*f),
            Value::Float(f) => Cell::Text(f.to_string()),
            Value::Text(s) => Cell::Text(s.clone()),
            Value::Date(_) => Cell::Date(v.to_string()),
            Value::Timestamp(_) => Cell::Timestamp(v.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub dtype: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page {
    pub offset: u64,
    pub total_rows: u64,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    /// True when the result was produced from an older definition or input.
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileRow {
    pub name: String,
    #[serde(rename = "type")]
    pub dtype: String,
    pub rows: u64,
    pub missing: u64,
    pub distinct: u64,
    pub min: Cell,
    pub max: Cell,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepView {
    pub id: String,
    pub input: String,
    pub label: Option<String>,
    /// The step definition in `analysis.json` format.
    pub op: J,
    /// `visual`, `sql`, `python` or `chart`. Custom SQL and Python are never
    /// edited through visual controls.
    pub mode: String,
    /// `fresh`, `stale`, `not run`, `failed`, `cancelled`, `blocked` or
    /// `needs review` (Python code that has not been approved to run) or
    /// `waiting on review` (an earlier step needs review).
    pub state: String,
    pub detail: Option<String>,
    pub rows: Option<u64>,
    pub columns: Option<Vec<Column>>,
    /// Inspectable SQL back to the source or nearest Python step.
    pub sql: Option<String>,
    /// Steps that read this one's result.
    pub dependents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceView {
    pub path: String,
    pub rows: u64,
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnalysisView {
    pub project_dir: String,
    pub name: String,
    pub source: SourceView,
    pub steps: Vec<StepView>,
    /// Notebook cells in document order (`analysis.json` format).
    pub cells: J,
    pub python: Option<String>,
    /// Python steps waiting for the user's approval, in run order.
    pub review: Vec<ReviewItem>,
    /// Set when the source file lives outside the project folder (custom
    /// SQL may read it; nothing else outside the folder).
    pub external_source: Option<String>,
}

/// Code the user is asked to read before it runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewItem {
    pub step_id: String,
    pub label: Option<String>,
    pub input: String,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepEvent {
    pub step_id: String,
    pub status: String,
    pub cached: bool,
    pub detail: Option<String>,
    pub rows: Option<u64>,
    pub duration_ms: u64,
}

impl From<&StepRun> for StepEvent {
    fn from(r: &StepRun) -> Self {
        let (cached, detail) = match &r.status {
            RunStatus::Succeeded { cached } => (*cached, None),
            RunStatus::Failed { error } => (false, Some(error.clone())),
            RunStatus::Skipped { reason } => (false, Some(reason.clone())),
            RunStatus::Cancelled | RunStatus::NeedsReview => (false, None),
        };
        StepEvent {
            step_id: r.step_id.clone(),
            status: r.status.name().into(),
            cached,
            detail,
            rows: r.rows.map(|n| n as u64),
            duration_ms: r.duration_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub succeeded: bool,
    pub steps: Vec<StepEvent>,
}

fn to_serde(j: &Json) -> J {
    serde_json::from_str(&j.to_string_compact()).expect("core JSON is valid JSON")
}

fn from_serde(v: &J) -> Result<Json> {
    Ok(Json::parse(&serde_json::to_string(v)?)?)
}

fn columns(schema: &vda_core::table::Schema) -> Vec<Column> {
    schema.fields.iter().map(|f| Column { name: f.name.clone(), dtype: f.dtype.name().into() }).collect()
}

fn page_of(t: &Table, offset: u64, total_rows: u64, stale: bool) -> Page {
    Page {
        offset,
        total_rows,
        columns: columns(&t.schema),
        rows: t.rows.iter().map(|r| r.iter().map(Cell::from).collect()).collect(),
        stale,
    }
}

fn mode(kind: &StepKind) -> &'static str {
    match kind {
        StepKind::Sql { .. } => "sql",
        StepKind::Python { .. } => "python",
        StepKind::Chart { .. } => "chart",
        _ => "visual",
    }
}

/// Application state: at most one open project.
///
/// Every project is opened with the safeguards of ADR-0004: Python code runs
/// only after the user has approved it, and SQL can read only the project
/// folder (and the source file, if it is kept elsewhere).
pub struct App {
    session: Option<Session>,
    engine_options: EngineOptions,
    interpreter: PathBuf,
    /// Where approvals are kept; `None` keeps them in memory for this App.
    approvals_path: Option<PathBuf>,
    memory_approvals: Approvals,
}

impl Default for App {
    fn default() -> Self {
        Self::new(EngineOptions::default(), default_interpreter()).with_approvals_file(review::default_approvals_path())
    }
}

impl App {
    /// Approvals are kept in memory until `with_approvals_file` is used.
    pub fn new(engine_options: EngineOptions, interpreter: PathBuf) -> Self {
        App { session: None, engine_options, interpreter, approvals_path: None, memory_approvals: Approvals::in_memory() }
    }

    /// Keep approvals in this file (outside any project, so a project can
    /// never approve its own code).
    pub fn with_approvals_file(mut self, path: PathBuf) -> Self {
        self.approvals_path = Some(path);
        self
    }

    fn start(&mut self, analysis: Option<Analysis>, dir: &Path) -> Result<()> {
        let source = match &analysis {
            Some(a) => a.source.path.clone(),
            None => persist::load_analysis(dir)?.source.path,
        };
        let mut allowed = vec![dir.to_path_buf()];
        let source = Path::new(&source);
        if source.is_absolute() {
            allowed.push(source.to_path_buf());
        }
        let options = EngineOptions { allowed_paths: Some(allowed), ..self.engine_options.clone() };
        let engine = Box::new(DuckDbEngine::with_options(options)?);
        let python = Some(PythonRunner::new(self.interpreter.clone(), dir.join(".work")));
        let mut session = match analysis {
            Some(a) => Session::new(a, dir, engine, python)?,
            None => Session::open(dir, engine, python)?,
        };
        let approvals = match &self.approvals_path {
            Some(p) => Approvals::load(p.clone())?,
            None => self.memory_approvals.clone(),
        };
        session.set_review_policy(ReviewPolicy::RequireApproval(approvals));
        self.session = Some(session);
        Ok(())
    }

    /// Approve the current code of these steps so they can run.
    pub fn approve(&mut self, step_ids: &[String]) -> Result<AnalysisView> {
        let s = self.session()?;
        for id in step_ids {
            s.approve_step(id)?;
        }
        self.remember_approvals();
        self.view()
    }

    /// In-memory approvals outlive the session (close and reopen keeps them).
    fn remember_approvals(&mut self) {
        if self.approvals_path.is_none() {
            if let Some(ReviewPolicy::RequireApproval(a)) = self.session.as_ref().map(Session::review_policy) {
                self.memory_approvals = a.clone();
            }
        }
    }

    fn session(&mut self) -> Result<&mut Session> {
        match self.session.as_mut() {
            Some(s) => Ok(s),
            None => fail("no project is open"),
        }
    }

    /// Create a project folder around a CSV file (copied in, so the project
    /// is self-contained) and open it.
    pub fn create_project(&mut self, dir: &Path, csv: &Path, name: &str) -> Result<AnalysisView> {
        if dir.join("analysis.json").exists() {
            return fail(format!("{} already contains a project", dir.display()));
        }
        std::fs::create_dir_all(dir)?;
        let file = csv.file_name().ok_or_else(|| AppError::from("the CSV path has no file name"))?;
        std::fs::copy(csv, dir.join(file))?;
        let analysis = Analysis {
            name: name.into(),
            source: Source { path: file.to_string_lossy().into_owned() },
            steps: vec![],
            cells: vec![],
        };
        persist::save_analysis(&analysis, dir)?;
        self.start(Some(analysis), dir)?;
        self.view()
    }

    pub fn open_project(&mut self, dir: &Path) -> Result<AnalysisView> {
        self.start(None, dir)?;
        self.view()
    }

    pub fn close_project(&mut self) {
        self.session = None;
    }

    pub fn view(&mut self) -> Result<AnalysisView> {
        let s = self.session()?;
        let source = s.source_dataset()?;
        let a = s.analysis.clone();
        let mut steps = Vec::new();
        for step in &a.steps {
            let state = s.state(&step.id)?;
            let detail = match &state {
                StepState::Failed(e) | StepState::Blocked(e) => Some(e.clone()),
                _ => None,
            };
            let (rows, cols) = match s.output(&step.id) {
                Some(Output::Table(ds)) => (Some(ds.rows), Some(columns(&ds.schema))),
                _ => (None, None),
            };
            steps.push(StepView {
                id: step.id.clone(),
                input: step.input.clone(),
                label: step.label.clone(),
                op: to_serde(&persist::step_kind_to_json(&step.kind)),
                mode: mode(&step.kind).into(),
                state: state.label().into(),
                detail,
                rows,
                columns: cols,
                sql: compile::pipeline_sql(&a, &step.id).ok(),
                dependents: a.steps.iter().filter(|o| o.input == step.id).map(|o| o.id.clone()).collect(),
            });
        }
        let cells = to_serde(persist::analysis_to_json(&a).get("cells").expect("cells are always written"));
        Ok(AnalysisView {
            project_dir: s.project_dir().to_string_lossy().into_owned(),
            name: a.name.clone(),
            source: SourceView { path: a.source.path.clone(), rows: source.rows, columns: columns(&source.schema) },
            steps,
            cells,
            python: s.python_fingerprint().map(str::to_string),
            review: s
                .pending_review()
                .iter()
                .filter_map(|id| a.step(id))
                .filter_map(|st| match &st.kind {
                    StepKind::Python { code, .. } => Some(ReviewItem {
                        step_id: st.id.clone(),
                        label: st.label.clone(),
                        input: st.input.clone(),
                        code: code.clone(),
                    }),
                    _ => None,
                })
                .collect(),
            external_source: Path::new(&a.source.path).is_absolute().then(|| a.source.path.clone()),
        })
    }

    pub fn source_profile(&mut self) -> Result<Vec<ProfileRow>> {
        let profile = self.session()?.source_profile()?;
        Ok(profile
            .into_iter()
            .map(|p| ProfileRow {
                name: p.name,
                dtype: p.dtype.name().into(),
                rows: p.rows as u64,
                missing: p.nulls as u64,
                distinct: p.distinct as u64,
                min: p.min.as_ref().map(Cell::from).unwrap_or(Cell::Null),
                max: p.max.as_ref().map(Cell::from).unwrap_or(Cell::Null),
            })
            .collect())
    }

    pub fn source_page(&mut self, offset: u64, limit: u64) -> Result<Page> {
        let s = self.session()?;
        let total = s.source_dataset()?.rows;
        let t = s.source_preview(offset, limit.min(MAX_PAGE_ROWS))?;
        Ok(page_of(&t, offset, total, false))
    }

    /// One page of a step's held result, fresh or stale (flagged).
    pub fn page(&mut self, step_id: &str, offset: u64, limit: u64) -> Result<Page> {
        let s = self.session()?;
        let total = match s.output(step_id) {
            Some(Output::Table(ds)) => ds.rows,
            Some(Output::Chart(_)) => return fail(format!("step `{step_id}` is a chart")),
            None => return fail(format!("step `{step_id}` has no result yet")),
        };
        let stale = s.state(step_id)? != StepState::Fresh;
        let t = s.preview(step_id, offset, limit.min(MAX_PAGE_ROWS))?;
        Ok(page_of(&t, offset, total, stale))
    }

    pub fn chart(&mut self, step_id: &str) -> Result<J> {
        match self.session()?.output(step_id) {
            Some(Output::Chart(spec)) => Ok(to_serde(spec)),
            _ => fail(format!("step `{step_id}` has no chart yet")),
        }
    }

    fn next_id(a: &Analysis, prefix: &str) -> String {
        (1..).map(|n| format!("{prefix}_{n}")).find(|id| a.step(id).is_none()).unwrap()
    }

    /// Reject a visual step that cannot work on its input's current columns,
    /// when that input's columns are known. Custom code is checked when it runs.
    fn check_against_input(s: &mut Session, step: &Step) -> Result<()> {
        let schema = if step.input == SOURCE_ID {
            Some(s.source_dataset()?.schema)
        } else {
            s.output(&step.input).and_then(Output::as_dataset).map(|d| d.schema.clone())
        };
        if let Some(schema) = schema {
            compile::output_schema(step, &schema)?;
        }
        Ok(())
    }

    fn commit(&mut self, analysis: Analysis) -> Result<AnalysisView> {
        vda_core::plan::validate(&analysis)?;
        let s = self.session()?;
        persist::save_analysis(&analysis, s.project_dir())?;
        s.analysis = analysis;
        self.view()
    }

    /// Add a step reading `input`, and a notebook cell showing it.
    pub fn add_step(&mut self, input: &str, label: Option<String>, op: &J) -> Result<AnalysisView> {
        let kind = persist::step_kind_from_json(&from_serde(op)?)?;
        let s = self.session()?;
        let mut a = s.analysis.clone();
        let id = Self::next_id(&a, kind.type_name());
        let step = Step { id: id.clone(), input: input.into(), label, kind };
        Self::check_against_input(s, &step)?;
        a.steps.push(step);
        a.cells.push(NoteCell::Step { step_id: id.clone() });
        self.commit_with_approval(a, &id, true)
    }

    /// Save, and approve the step's code when the user has just written it
    /// in the app. Saving a label or other setting of code the user has not
    /// reviewed leaves it waiting for review.
    fn commit_with_approval(&mut self, analysis: Analysis, id: &str, code_written: bool) -> Result<AnalysisView> {
        self.commit(analysis)?;
        if code_written {
            self.session()?.approve_step(id)?;
            self.remember_approvals();
        }
        self.view()
    }

    pub fn update_step(&mut self, id: &str, label: Option<String>, op: &J) -> Result<AnalysisView> {
        let kind = persist::step_kind_from_json(&from_serde(op)?)?;
        let s = self.session()?;
        let mut a = s.analysis.clone();
        let Some(step) = a.step_mut(id) else { return fail(format!("unknown step `{id}`")) };
        if mode(&step.kind) != mode(&kind) {
            return fail("a step cannot change between visual, SQL, Python and chart; add a new step instead");
        }
        let code_written = review::fingerprint(&step.kind) != review::fingerprint(&kind);
        step.kind = kind;
        step.label = label;
        let step = step.clone();
        Self::check_against_input(s, &step)?;
        self.commit_with_approval(a, id, code_written)
    }

    pub fn remove_step(&mut self, id: &str) -> Result<AnalysisView> {
        let mut a = self.session()?.analysis.clone();
        if let Some(dep) = a.steps.iter().find(|s| s.input == id) {
            return fail(format!("step `{}` reads `{id}`; remove or repoint it first", dep.id));
        }
        a.steps.retain(|s| s.id != id);
        a.cells.retain(|c| !matches!(c, NoteCell::Step { step_id } if step_id == id));
        self.commit(a)
    }

    /// Replace the notebook cells (reordering, markdown edits).
    pub fn set_cells(&mut self, cells: &J) -> Result<AnalysisView> {
        let mut a = self.session()?.analysis.clone();
        let mut doc = persist::analysis_to_json(&a);
        if let Json::Obj(m) = &mut doc {
            m.insert("cells".into(), from_serde(cells)?);
        }
        a.cells = persist::analysis_from_json(&doc)?.cells;
        self.commit(a)
    }

    pub fn rename(&mut self, name: &str) -> Result<AnalysisView> {
        let mut a = self.session()?.analysis.clone();
        a.name = name.into();
        self.commit(a)
    }

    /// Run the analysis, reporting each step as it finishes, and save the run record.
    pub fn run(&mut self, on_step: &mut dyn FnMut(StepEvent)) -> Result<RunSummary> {
        let s = self.session()?;
        let record = s.run_all_with(&mut |r| on_step(StepEvent::from(r)))?;
        s.save_run(&record)?;
        Ok(RunSummary {
            run_id: record.run_id.clone(),
            succeeded: record.succeeded(),
            steps: record.steps.iter().map(StepEvent::from).collect(),
        })
    }

    /// Flag the UI sets to cancel a run in progress (taken before running, so
    /// cancelling does not need access to the busy session).
    pub fn cancel_handle(&mut self) -> Result<Arc<AtomicBool>> {
        Ok(self.session()?.cancel_handle())
    }
}
