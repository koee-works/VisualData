//! A working session on one analysis: runs steps, caches results by key,
//! reports fresh/stale state, writes run records, and replays them.

use crate::artifacts::ArtifactStore;
use crate::chart;
use crate::compile;
use crate::engine::Engine;
use crate::hash::{sha256_hex, Sha256};
use crate::json::Json;
use crate::model::{Analysis, StepKind, SOURCE_ID};
use crate::plan::{self, Fingerprints, Plan};
use crate::python::PythonRunner;
use crate::table::Table;
use crate::Error;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Table(Table),
    Chart(Json),
}

impl Output {
    pub fn content_hash(&self) -> String {
        match self {
            Output::Table(t) => t.content_hash(),
            Output::Chart(spec) => sha256_hex(spec.to_string_compact().as_bytes()),
        }
    }
    pub fn rows(&self) -> Option<usize> {
        match self {
            Output::Table(t) => Some(t.rows.len()),
            Output::Chart(_) => None,
        }
    }
    pub fn as_table(&self) -> Option<&Table> {
        match self {
            Output::Table(t) => Some(t),
            Output::Chart(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
struct CachedResult {
    key: String,
    output: Output,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunStatus {
    Succeeded { cached: bool },
    Failed { error: String },
    Cancelled,
    /// Not attempted because an upstream step failed or the run was cancelled.
    Skipped { reason: String },
}

impl RunStatus {
    pub fn name(&self) -> &'static str {
        match self {
            RunStatus::Succeeded { .. } => "succeeded",
            RunStatus::Failed { .. } => "failed",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Skipped { .. } => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepRun {
    pub step_id: String,
    pub cache_key: String,
    pub status: RunStatus,
    pub rows: Option<usize>,
    pub content_hash: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub run_id: String,
    pub started_unix_ms: u128,
    pub engine: String,
    pub python: Option<String>,
    pub source_path: String,
    pub source_hash: String,
    pub steps: Vec<StepRun>,
    /// Problems that did not fail a step, e.g. a result that could not be saved.
    pub warnings: Vec<String>,
}

impl RunRecord {
    pub fn step(&self, id: &str) -> Option<&StepRun> {
        self.steps.iter().find(|s| s.step_id == id)
    }

    pub fn succeeded(&self) -> bool {
        self.steps.iter().all(|s| matches!(s.status, RunStatus::Succeeded { .. }))
    }

    pub fn to_json(&self) -> Json {
        let opt = |v: &Option<String>| v.clone().map(Json::Str).unwrap_or(Json::Null);
        Json::obj(vec![
            ("run_id", Json::str(self.run_id.clone())),
            ("started_unix_ms", Json::Num(self.started_unix_ms as f64)),
            ("environment", Json::obj(vec![("engine", Json::str(self.engine.clone())), ("python", opt(&self.python))])),
            (
                "source",
                Json::obj(vec![
                    ("path", Json::str(self.source_path.clone())),
                    ("sha256", Json::str(self.source_hash.clone())),
                ]),
            ),
            (
                "steps",
                Json::Arr(
                    self.steps
                        .iter()
                        .map(|s| {
                            let (detail, cached) = match &s.status {
                                RunStatus::Failed { error } => (Some(error.clone()), Json::Null),
                                RunStatus::Skipped { reason } => (Some(reason.clone()), Json::Null),
                                RunStatus::Succeeded { cached } => (None, Json::Bool(*cached)),
                                RunStatus::Cancelled => (None, Json::Null),
                            };
                            Json::obj(vec![
                                ("step", Json::str(s.step_id.clone())),
                                ("cache_key", Json::str(s.cache_key.clone())),
                                ("status", Json::str(s.status.name())),
                                ("cached", cached),
                                ("detail", opt(&detail)),
                                ("rows", s.rows.map(|r| Json::Num(r as f64)).unwrap_or(Json::Null)),
                                ("content_sha256", opt(&s.content_hash)),
                                ("duration_ms", Json::Num(s.duration_ms as f64)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("warnings", Json::Arr(self.warnings.iter().map(|w| Json::str(w.clone())).collect())),
        ])
    }

    pub fn from_json(j: &Json) -> Result<RunRecord, Error> {
        let env = j.get("environment").ok_or_else(|| Error::Format("missing environment".into()))?;
        let src = j.get("source").ok_or_else(|| Error::Format("missing source".into()))?;
        let opt = |o: &Json, k: &str| o.get(k).and_then(Json::as_str).map(str::to_string);
        Ok(RunRecord {
            run_id: j.req_str("run_id")?.to_string(),
            started_unix_ms: j.get("started_unix_ms").and_then(Json::as_f64).unwrap_or(0.0) as u128,
            engine: env.req_str("engine")?.to_string(),
            python: opt(env, "python"),
            source_path: src.req_str("path")?.to_string(),
            source_hash: src.req_str("sha256")?.to_string(),
            steps: j
                .req_arr("steps")?
                .iter()
                .map(|s| {
                    let detail = opt(s, "detail").unwrap_or_default();
                    let status = match s.req_str("status")? {
                        "succeeded" => RunStatus::Succeeded { cached: s.get("cached") == Some(&Json::Bool(true)) },
                        "failed" => RunStatus::Failed { error: detail },
                        "cancelled" => RunStatus::Cancelled,
                        "skipped" => RunStatus::Skipped { reason: detail },
                        other => return Err(Error::Format(format!("unknown status `{other}`"))),
                    };
                    Ok(StepRun {
                        step_id: s.req_str("step")?.to_string(),
                        cache_key: s.req_str("cache_key")?.to_string(),
                        status,
                        rows: s.get("rows").and_then(Json::as_f64).map(|r| r as usize),
                        content_hash: opt(s, "content_sha256"),
                        duration_ms: s.get("duration_ms").and_then(Json::as_f64).unwrap_or(0.0) as u64,
                    })
                })
                .collect::<Result<_, Error>>()?,
            warnings: j
                .get("warnings")
                .and_then(Json::as_arr)
                .map(|w| w.iter().filter_map(Json::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
        })
    }
}

/// What the UI shows for a step right now.
#[derive(Debug, Clone, PartialEq)]
pub enum StepState {
    NotRun,
    /// The held result matches the current definition and inputs.
    Fresh,
    /// A result is held but was produced from an older definition or input.
    /// It may be displayed only if clearly labelled as outdated.
    Stale,
    Failed(String),
    Cancelled,
    /// Not attempted in the last run because something upstream failed.
    Blocked(String),
}

impl StepState {
    pub fn label(&self) -> &'static str {
        match self {
            StepState::NotRun => "not run",
            StepState::Fresh => "fresh",
            StepState::Stale => "stale",
            StepState::Failed(_) => "failed",
            StepState::Cancelled => "cancelled",
            StepState::Blocked(_) => "blocked",
        }
    }
}

pub struct Session {
    pub analysis: Analysis,
    project_dir: PathBuf,
    engine: Box<dyn Engine>,
    python: Option<PythonRunner>,
    python_fp: Option<String>,
    source_cache: Option<(String, Table)>,
    results: HashMap<String, CachedResult>,
    pub last_run: Option<RunRecord>,
    cancel: Arc<AtomicBool>,
    artifacts: Option<ArtifactStore>,
}

impl Session {
    pub fn new(analysis: Analysis, project_dir: impl Into<PathBuf>, engine: Box<dyn Engine>, python: Option<PythonRunner>) -> Self {
        let python_fp = python.as_ref().and_then(|p| p.fingerprint().ok());
        let project_dir: PathBuf = project_dir.into();
        Session {
            analysis,
            artifacts: Some(ArtifactStore::new(project_dir.join("artifacts"))),
            project_dir,
            engine,
            python,
            python_fp,
            source_cache: None,
            results: HashMap::new(),
            last_run: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Open a saved project: load `analysis.json` and any saved results that
    /// are still current, so the analysis is viewable without rerunning.
    pub fn open(project_dir: impl Into<PathBuf>, engine: Box<dyn Engine>, python: Option<PythonRunner>) -> Result<Self, Error> {
        let project_dir = project_dir.into();
        let analysis = crate::persist::load_analysis(&project_dir)?;
        let mut session = Session::new(analysis, project_dir, engine, python);
        session.hydrate()?;
        Ok(session)
    }

    /// Run without reading or writing saved results (used for clean replay).
    pub fn without_saved_results(mut self) -> Self {
        self.artifacts = None;
        self
    }

    /// Load saved results whose key matches the current definition. Returns
    /// how many steps were loaded. Results already in memory are kept.
    pub fn hydrate(&mut self) -> Result<usize, Error> {
        let Some(store) = self.artifacts.clone() else { return Ok(0) };
        let plan = self.plan()?;
        let mut loaded = 0;
        for id in &plan.order {
            let key = &plan.keys[id];
            if self.results.get(id).map(|r| &r.key) == Some(key) {
                continue;
            }
            let chart = !self.analysis.step(id).unwrap().kind.produces_table();
            if let Some(output) = store.load(key, chart) {
                self.results.insert(id.clone(), CachedResult { key: key.clone(), output });
                loaded += 1;
            }
        }
        Ok(loaded)
    }

    /// Delete saved results that no step in the current definition uses.
    pub fn prune_saved_results(&self) -> Result<usize, Error> {
        let Some(store) = &self.artifacts else { return Ok(0) };
        let keep: HashSet<String> = self.plan()?.keys.into_values().collect();
        store.prune(&keep)
    }

    /// Handle the UI can use to cancel a running analysis from another thread.
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub fn python_fingerprint(&self) -> Option<&str> {
        self.python_fp.as_deref()
    }

    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    fn source_path(&self) -> PathBuf {
        let p = Path::new(&self.analysis.source.path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.project_dir.join(p)
        }
    }

    pub fn source_hash(&self) -> Result<String, Error> {
        let path = self.source_path();
        let bytes = fs::read(&path).map_err(|e| Error::Data(format!("cannot read source {}: {e}", path.display())))?;
        Ok(sha256_hex(&bytes))
    }

    pub fn plan(&self) -> Result<Plan, Error> {
        let source = self.source_hash()?;
        let engine = self.engine.fingerprint();
        plan::plan(
            &self.analysis,
            &Fingerprints { source: &source, engine: &engine, python: self.python_fp.as_deref() },
        )
    }

    /// Source table, imported once per source content hash.
    pub fn source_table(&mut self) -> Result<&Table, Error> {
        let hash = self.source_hash()?;
        if self.source_cache.as_ref().map(|(h, _)| h != &hash).unwrap_or(true) {
            let t = self.engine.import_csv(&self.source_path())?;
            self.source_cache = Some((hash, t));
        }
        Ok(&self.source_cache.as_ref().unwrap().1)
    }

    pub fn output(&self, step_id: &str) -> Option<&Output> {
        self.results.get(step_id).map(|r| &r.output)
    }

    pub fn state(&self, step_id: &str) -> Result<StepState, Error> {
        let plan = self.plan()?;
        let key = plan.keys.get(step_id).ok_or_else(|| Error::Validation(format!("unknown step `{step_id}`")))?;
        if self.results.get(step_id).map(|r| &r.key) == Some(key) {
            return Ok(StepState::Fresh);
        }
        if let Some(run) = self.last_run.as_ref().and_then(|r| r.step(step_id)) {
            if &run.cache_key == key {
                match &run.status {
                    RunStatus::Failed { error } => return Ok(StepState::Failed(error.clone())),
                    RunStatus::Cancelled => return Ok(StepState::Cancelled),
                    RunStatus::Skipped { reason } => return Ok(StepState::Blocked(reason.clone())),
                    RunStatus::Succeeded { .. } => {}
                }
            }
        }
        Ok(if self.results.contains_key(step_id) { StepState::Stale } else { StepState::NotRun })
    }

    /// Run every step in dependency order, reusing results whose key is
    /// unchanged. Step failures are recorded, not returned as errors.
    pub fn run_all(&mut self) -> Result<RunRecord, Error> {
        self.cancel.store(false, Ordering::SeqCst);
        let plan = self.plan()?;
        let source_hash = self.source_hash()?;
        let started = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
        let mut record = RunRecord {
            run_id: format!("run-{started}"),
            started_unix_ms: started,
            engine: self.engine.fingerprint(),
            python: self.python_fp.clone(),
            source_path: self.analysis.source.path.clone(),
            source_hash,
            steps: Vec::new(),
            warnings: Vec::new(),
        };
        let mut halted: Option<String> = None;

        for id in &plan.order {
            let key = plan.keys[id].clone();
            let step = self.analysis.step(id).unwrap().clone();
            let t0 = Instant::now();

            let upstream_ok = step.input == SOURCE_ID
                || record.step(&step.input).is_some_and(|r| matches!(r.status, RunStatus::Succeeded { .. }));
            let status: RunStatus = if let Some(reason) = &halted {
                RunStatus::Skipped { reason: reason.clone() }
            } else if !upstream_ok {
                RunStatus::Skipped { reason: format!("upstream step `{}` did not succeed", step.input) }
            } else if self.cancel.load(Ordering::SeqCst) {
                halted = Some("run was cancelled".into());
                RunStatus::Cancelled
            } else if self.results.get(id).map(|r| &r.key) == Some(&key) {
                RunStatus::Succeeded { cached: true }
            } else if let Some(output) =
                self.artifacts.as_ref().and_then(|a| a.load(&key, !step.kind.produces_table()))
            {
                self.results.insert(id.clone(), CachedResult { key: key.clone(), output });
                RunStatus::Succeeded { cached: true }
            } else {
                match self.execute(&step.id) {
                    Ok(output) => {
                        if let Some(store) = &self.artifacts {
                            if let Err(e) = store.save(&key, &output) {
                                record.warnings.push(format!("step `{id}`: result not saved to disk: {e}"));
                            }
                        }
                        self.results.insert(id.clone(), CachedResult { key: key.clone(), output });
                        RunStatus::Succeeded { cached: false }
                    }
                    Err(Error::Cancelled) => {
                        halted = Some("run was cancelled".into());
                        RunStatus::Cancelled
                    }
                    // The previous (now stale) result is kept so the UI can
                    // still show it, labelled as outdated.
                    Err(e) => RunStatus::Failed { error: e.to_string() },
                }
            };
            let output = matches!(status, RunStatus::Succeeded { .. }).then(|| &self.results[id].output);
            record.steps.push(StepRun {
                step_id: id.clone(),
                cache_key: key,
                rows: output.and_then(Output::rows),
                content_hash: output.map(Output::content_hash),
                status,
                duration_ms: t0.elapsed().as_millis() as u64,
            });
        }
        self.last_run = Some(record.clone());
        Ok(record)
    }

    fn input_table(&mut self, input: &str) -> Result<Table, Error> {
        if input == SOURCE_ID {
            return Ok(self.source_table()?.clone());
        }
        self.results
            .get(input)
            .and_then(|r| r.output.as_table().cloned())
            .ok_or_else(|| Error::Validation(format!("no result for upstream step `{input}`")))
    }

    fn execute(&mut self, step_id: &str) -> Result<Output, Error> {
        let step = self.analysis.step(step_id).unwrap().clone();
        let input = self.input_table(&step.input)?;
        match &step.kind {
            StepKind::Filter { .. } | StepKind::Aggregate { .. } | StepKind::Sql { .. } => {
                Ok(Output::Table(self.engine.run_step(&step, &input)?))
            }
            StepKind::Python { code, timeout_ms } => {
                let runner = self.python.as_ref().ok_or_else(|| {
                    Error::Python("no Python interpreter is configured for this project".into())
                })?;
                if self.python_fp.is_none() {
                    return Err(Error::Python(format!(
                        "interpreter {:?} is not usable (it needs pandas)",
                        runner.interpreter
                    )));
                }
                Ok(Output::Table(runner.run(code, &input, Duration::from_millis(*timeout_ms), &self.cancel)?))
            }
            StepKind::Chart { mark, x, y, title } => {
                compile::output_schema(&step, &input.schema)?;
                Ok(Output::Chart(chart::vega_lite(&input, *mark, x, y, title.as_deref())?))
            }
        }
    }

    pub fn save_run(&self, record: &RunRecord) -> Result<PathBuf, Error> {
        let dir = self.project_dir.join("runs");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", record.run_id));
        fs::write(&path, record.to_json().to_string_pretty())?;
        Ok(path)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReplayOutcome {
    /// Same definition and inputs, identical result.
    Reproduced,
    /// Same definition and inputs, different result: nondeterminism.
    Diverged { recorded: String, replayed: String },
    /// The definition, source, or environment changed since the recording,
    /// so the result is not expected to match.
    InputsChanged,
    /// The step did not succeed in the recording or in the replay.
    NotComparable { detail: String },
}

#[derive(Debug, Clone)]
pub struct ReplayReport {
    pub recorded_run: String,
    pub replay_run: RunRecord,
    pub environment_changes: Vec<String>,
    pub steps: Vec<(String, ReplayOutcome)>,
}

impl ReplayReport {
    pub fn fully_reproduced(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|(_, o)| *o == ReplayOutcome::Reproduced)
    }
}

/// Replay a recorded run in a clean session (no cached results) and compare
/// every step's output with the recording.
pub fn replay(
    project_dir: &Path,
    recorded: &RunRecord,
    engine: Box<dyn Engine>,
    python: Option<PythonRunner>,
) -> Result<ReplayReport, Error> {
    let analysis = crate::persist::load_analysis(project_dir)?;
    // Clean runtime: nothing cached in memory, saved results neither read nor written.
    let mut session = Session::new(analysis, project_dir, engine, python).without_saved_results();
    let run = session.run_all()?;

    let mut env = Vec::new();
    if run.engine != recorded.engine {
        env.push(format!("engine: {} -> {}", recorded.engine, run.engine));
    }
    if run.python != recorded.python {
        env.push(format!("python: {:?} -> {:?}", recorded.python, run.python));
    }
    if run.source_hash != recorded.source_hash {
        env.push("source file content changed".into());
    }

    let mut steps = Vec::new();
    for now in &run.steps {
        let outcome = match recorded.step(&now.step_id) {
            None => ReplayOutcome::NotComparable { detail: "step did not exist in the recorded run".into() },
            Some(then) if then.cache_key != now.cache_key => ReplayOutcome::InputsChanged,
            Some(then) => match (&then.content_hash, &now.content_hash) {
                (Some(a), Some(b)) if a == b => ReplayOutcome::Reproduced,
                (Some(a), Some(b)) => ReplayOutcome::Diverged { recorded: a.clone(), replayed: b.clone() },
                _ => ReplayOutcome::NotComparable {
                    detail: format!("recorded: {}, replay: {}", then.status.name(), now.status.name()),
                },
            },
        };
        steps.push((now.step_id.clone(), outcome));
    }
    Ok(ReplayReport { recorded_run: recorded.run_id.clone(), replay_run: run, environment_changes: env, steps })
}

/// Fingerprint of an analysis definition (excludes labels and notebook text).
pub fn definition_hash(a: &Analysis) -> String {
    let mut h = Sha256::new();
    for s in &a.steps {
        h.update(s.id.as_bytes());
        h.update(s.input.as_bytes());
        h.update(crate::persist::step_kind_to_json(&s.kind).to_string_compact().as_bytes());
    }
    h.hex_digest()
}
