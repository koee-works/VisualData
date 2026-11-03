//! A working session on one analysis: runs steps, caches results by key,
//! reports fresh/stale state, writes run records, and replays them.
//!
//! Results live on disk as Parquet datasets in the project's artifact store.
//! The session holds only handles to them, so memory use does not grow with
//! data size; previews are read a page at a time.

use crate::artifacts::ArtifactStore;
use crate::chart;
use crate::compile::{self, CSV_OPTIONS, JSON_OPTIONS};
use crate::dataset::Dataset;
use crate::engine::Engine;
use crate::hash::{sha256_file, sha256_hex, Sha256};
use crate::json::Json;
use crate::model::{Analysis, SourceFormat, StepKind, SOURCE_ID};
use crate::table::Schema;
use crate::plan::{self, Fingerprints, Plan};
use crate::python::PythonRunner;
use crate::review::ReviewPolicy;
use crate::table::{ColumnProfile, Table};
use crate::Error;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// A table result, held on disk.
    Table(Dataset),
    Chart(Json),
}

impl Output {
    pub fn content_hash(&self) -> String {
        match self {
            Output::Table(ds) => ds.content_hash.clone(),
            Output::Chart(spec) => sha256_hex(spec.to_string_compact().as_bytes()),
        }
    }
    pub fn rows(&self) -> Option<usize> {
        match self {
            Output::Table(ds) => Some(ds.rows as usize),
            Output::Chart(_) => None,
        }
    }
    pub fn as_dataset(&self) -> Option<&Dataset> {
        match self {
            Output::Table(ds) => Some(ds),
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
    /// Not run: the step's code is waiting for the user's review.
    NeedsReview,
    /// Not attempted because an upstream step failed or the run was cancelled.
    Skipped { reason: String },
}

impl RunStatus {
    pub fn name(&self) -> &'static str {
        match self {
            RunStatus::Succeeded { .. } => "succeeded",
            RunStatus::Failed { .. } => "failed",
            RunStatus::Cancelled => "cancelled",
            RunStatus::NeedsReview => "needs review",
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
    /// What the step printed (Python stdout and stderr), if anything.
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub run_id: String,
    pub started_unix_ms: u128,
    pub engine: String,
    pub python: Option<String>,
    /// Every source as it was read: id, path and content hash.
    pub sources: Vec<SourceRun>,
    pub steps: Vec<StepRun>,
    /// Problems that did not fail a step, e.g. a result that could not be saved.
    pub warnings: Vec<String>,
    /// What started the run when it was not the app: "scheduled" for `vda-run`.
    pub trigger: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceRun {
    pub id: String,
    pub path: String,
    pub sha256: String,
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
            ("trigger", opt(&self.trigger)),
            (
                "sources",
                Json::Arr(
                    self.sources
                        .iter()
                        .map(|s| {
                            Json::obj(vec![
                                ("id", Json::str(s.id.clone())),
                                ("path", Json::str(s.path.clone())),
                                ("sha256", Json::str(s.sha256.clone())),
                            ])
                        })
                        .collect(),
                ),
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
                                RunStatus::Cancelled | RunStatus::NeedsReview => (None, Json::Null),
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
                                ("output", opt(&s.output)),
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
        let opt = |o: &Json, k: &str| o.get(k).and_then(Json::as_str).map(str::to_string);
        let read_source = |o: &Json, id: String| -> Result<SourceRun, Error> {
            Ok(SourceRun { id, path: o.req_str("path")?.to_string(), sha256: o.req_str("sha256")?.to_string() })
        };
        // Records from before multi-input analyses have one `source`.
        let sources = match (j.get("sources").and_then(Json::as_arr), j.get("source")) {
            (Some(list), _) => list
                .iter()
                .map(|o| read_source(o, o.req_str("id")?.to_string()))
                .collect::<Result<Vec<_>, Error>>()?,
            (None, Some(o)) => vec![read_source(o, SOURCE_ID.into())?],
            (None, None) => return Err(Error::Format("missing sources".into())),
        };
        Ok(RunRecord {
            run_id: j.req_str("run_id")?.to_string(),
            started_unix_ms: j.get("started_unix_ms").and_then(Json::as_f64).unwrap_or(0.0) as u128,
            engine: env.req_str("engine")?.to_string(),
            python: opt(env, "python"),
            trigger: opt(j, "trigger"),
            sources,
            steps: j
                .req_arr("steps")?
                .iter()
                .map(|s| {
                    let detail = opt(s, "detail").unwrap_or_default();
                    let status = match s.req_str("status")? {
                        "succeeded" => RunStatus::Succeeded { cached: s.get("cached") == Some(&Json::Bool(true)) },
                        "failed" => RunStatus::Failed { error: detail },
                        "cancelled" => RunStatus::Cancelled,
                        "needs review" => RunStatus::NeedsReview,
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
                        output: opt(s, "output"),
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
    /// The step's code must be reviewed before it can run.
    NeedsReview,
    /// An earlier step's code must be reviewed before this one can run.
    WaitingOnReview,
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
            StepState::NeedsReview => "needs review",
            StepState::WaitingOnReview => "waiting on review",
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
    /// (size, modified time, hash) of each source file by id, so sources are
    /// not rehashed on every state query.
    source_fp: RefCell<HashMap<String, (u64, SystemTime, String)>>,
    /// Sources converted to Parquet (by id), keyed like step results.
    source_ds: HashMap<String, (String, Dataset)>,
    results: HashMap<String, CachedResult>,
    pub last_run: Option<RunRecord>,
    cancel: Arc<AtomicBool>,
    artifacts: ArtifactStore,
    review: ReviewPolicy,
    /// Output of the step being executed, picked up by the run record.
    step_output: Option<String>,
    /// Column details by dataset content hash.
    details: HashMap<String, Vec<crate::profile::ColumnDetail>>,
}

impl Session {
    pub fn new(
        analysis: Analysis,
        project_dir: impl Into<PathBuf>,
        engine: Box<dyn Engine>,
        python: Option<PythonRunner>,
    ) -> Result<Self, Error> {
        let python_fp = python.as_ref().and_then(|p| p.fingerprint().ok());
        let project_dir: PathBuf = project_dir.into();
        Ok(Session {
            analysis,
            artifacts: ArtifactStore::new(project_dir.join("artifacts"))?,
            project_dir,
            engine,
            python,
            python_fp,
            source_fp: RefCell::new(HashMap::new()),
            source_ds: HashMap::new(),
            results: HashMap::new(),
            last_run: None,
            cancel: Arc::new(AtomicBool::new(false)),
            review: ReviewPolicy::default(),
            step_output: None,
            details: HashMap::new(),
        })
    }

    /// Open a saved project: load `analysis.json` and any saved results that
    /// are still current, so the analysis is viewable without rerunning.
    pub fn open(project_dir: impl Into<PathBuf>, engine: Box<dyn Engine>, python: Option<PythonRunner>) -> Result<Self, Error> {
        let project_dir = project_dir.into();
        let analysis = crate::persist::load_analysis(&project_dir)?;
        let mut session = Session::new(analysis, project_dir, engine, python)?;
        session.hydrate()?;
        Ok(session)
    }

    /// Use a throwaway result store instead of the project's, so nothing
    /// saved earlier is reused (clean replay). It is deleted with the session.
    pub fn without_saved_results(mut self) -> Result<Self, Error> {
        self.artifacts = ArtifactStore::ephemeral()?;
        self.results.clear();
        self.source_ds.clear();
        Ok(self)
    }

    /// Load handles to saved results whose key matches the current
    /// definition. Returns how many steps were loaded. No data is read.
    pub fn hydrate(&mut self) -> Result<usize, Error> {
        let plan = self.plan()?;
        let mut loaded = 0;
        for id in &plan.order {
            let key = &plan.keys[id];
            if self.results.get(id).map(|r| &r.key) == Some(key) {
                continue;
            }
            let chart = !self.analysis.step(id).unwrap().kind.produces_table();
            if let Some(output) = self.artifacts.load(key, chart) {
                self.results.insert(id.clone(), CachedResult { key: key.clone(), output });
                loaded += 1;
            }
        }
        Ok(loaded)
    }

    /// Delete saved results that the current definition does not use.
    pub fn prune_saved_results(&self) -> Result<usize, Error> {
        let mut keep: HashSet<String> = self.plan()?.keys.into_values().collect();
        for src in &self.analysis.sources {
            keep.insert(self.source_key(&src.id)?);
        }
        self.artifacts.prune(&keep)
    }

    /// Require approval before Python code runs (the desktop app always does).
    pub fn set_review_policy(&mut self, policy: ReviewPolicy) {
        self.review = policy;
    }

    pub fn review_policy(&self) -> &ReviewPolicy {
        &self.review
    }

    pub fn review_policy_mut(&mut self) -> &mut ReviewPolicy {
        &mut self.review
    }

    /// Steps whose code is waiting for review, in execution order.
    pub fn pending_review(&self) -> Vec<String> {
        let order = plan::validate(&self.analysis).unwrap_or_default();
        order.into_iter().filter(|id| self.review.pending(&self.analysis.step(id).unwrap().kind)).collect()
    }

    /// Approve the step's current code (it then runs on the next run).
    pub fn approve_step(&mut self, step_id: &str) -> Result<(), Error> {
        let kind = self
            .analysis
            .step(step_id)
            .ok_or_else(|| Error::Validation(format!("unknown step `{step_id}`")))?
            .kind
            .clone();
        self.review.approve(&kind)
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

    /// Where a source's file is on disk.
    pub fn source_path(&self, id: &str) -> Result<PathBuf, Error> {
        let src = self.analysis.source(id).ok_or_else(|| Error::Validation(format!("unknown source `{id}`")))?;
        let p = Path::new(&src.path);
        Ok(if p.is_absolute() { p.to_path_buf() } else { self.project_dir.join(p) })
    }

    /// SHA-256 of the source file. Streamed, and recomputed only when the
    /// file's size or modification time changes. The fingerprint is also kept
    /// in the artifact store, so reopening a project does not rehash a large
    /// source. (A content change that keeps both size and modification time
    /// would go unnoticed; editors and copies update the modification time.)
    pub fn source_hash(&self, id: &str) -> Result<String, Error> {
        let path = self.source_path(id)?;
        let meta = fs::metadata(&path).map_err(|e| Error::Data(format!("cannot read source {}: {e}", path.display())))?;
        let stamp = (meta.len(), meta.modified()?);
        if let Some((len, modified, hash)) = self.source_fp.borrow().get(id) {
            if (*len, *modified) == stamp {
                return Ok(hash.clone());
            }
        }
        let nanos = stamp.1.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos().to_string();
        // The first source keeps the file name it had before multi-source analyses.
        let name = if id == SOURCE_ID { "source-fingerprint.json".to_string() } else { format!("source-fingerprint-{id}.json") };
        let saved = self.artifacts.dir().join(&name);
        let remembered = fs::read_to_string(&saved).ok().and_then(|t| Json::parse(&t).ok()).and_then(|j| {
            let matches = j.get("path").and_then(Json::as_str) == Some(&*path.to_string_lossy())
                && j.get("bytes").and_then(Json::as_str) == Some(&*stamp.0.to_string())
                && j.get("modified_ns").and_then(Json::as_str) == Some(&*nanos);
            matches.then(|| j.get("sha256").and_then(Json::as_str).map(str::to_string)).flatten()
        });
        let hash = match remembered {
            Some(h) => h,
            None => {
                let h = sha256_file(&path)
                    .map_err(|e| Error::Data(format!("cannot read source {}: {e}", path.display())))?;
                let record = Json::obj(vec![
                    ("path", Json::str(path.to_string_lossy())),
                    ("bytes", Json::str(stamp.0.to_string())),
                    ("modified_ns", Json::str(nanos)),
                    ("sha256", Json::str(h.clone())),
                ]);
                // Best effort: losing it only costs a rehash next time.
                let tmp = self.artifacts.dir().join(format!(".{name}.tmp"));
                if fs::write(&tmp, record.to_string_pretty()).is_ok() {
                    let _ = fs::rename(&tmp, &saved);
                }
                h
            }
        };
        self.source_fp.borrow_mut().insert(id.to_string(), (stamp.0, stamp.1, hash.clone()));
        Ok(hash)
    }

    /// Content hash of every source, by id.
    pub fn source_hashes(&self) -> Result<HashMap<String, String>, Error> {
        self.analysis.sources.iter().map(|s| Ok((s.id.clone(), self.source_hash(&s.id)?))).collect()
    }

    /// What a step reading a source depends on: its content, and for
    /// formats other than CSV also how it is read (format, sheet). CSV keeps
    /// the plain content hash, so keys from earlier versions still match.
    fn source_fingerprints(&self) -> Result<HashMap<String, String>, Error> {
        let mut out = self.source_hashes()?;
        for src in &self.analysis.sources {
            if src.format != SourceFormat::Csv {
                let hash = out.get_mut(&src.id).unwrap();
                let mut text = format!("{hash}\n{}\n{}", src.format.name(), src.sheet.as_deref().unwrap_or(""));
                if let Some(q) = &src.query {
                    text.push_str(&format!("\nquery:{q}"));
                }
                *hash = sha256_hex(text.as_bytes());
            }
        }
        Ok(out)
    }

    pub fn plan(&self) -> Result<Plan, Error> {
        let sources = self.source_fingerprints()?;
        let engine = self.engine.fingerprint();
        plan::plan(
            &self.analysis.bound(),
            &Fingerprints { sources: &sources, engine: &engine, python: self.python_fp.as_deref() },
        )
    }

    fn source_key(&self, id: &str) -> Result<String, Error> {
        let src = self.analysis.source(id).ok_or_else(|| Error::Validation(format!("unknown source `{id}`")))?;
        let mut h = Sha256::new();
        // CSV keys are unchanged from the first format, so converted sources are reused.
        let how = match src.format {
            SourceFormat::Csv => CSV_OPTIONS.to_string(),
            SourceFormat::Json => format!("json\n{JSON_OPTIONS}"),
            f => {
                let mut how = format!("{}\n{}", f.name(), src.sheet.as_deref().unwrap_or(""));
                // A database read by a query is a different copy than its table.
                if let Some(q) = &src.query {
                    how.push_str(&format!("\nquery:{q}"));
                }
                how
            }
        };
        for part in ["vda-source-v1", &self.source_hash(id)?, &self.engine.fingerprint(), &how] {
            h.update(part.as_bytes());
            h.update(b"\n");
        }
        Ok(format!("source-{}", h.hex_digest()))
    }

    /// The source as a Parquet dataset: converted once per source content
    /// and engine version, then reused from the artifact store.
    pub fn source_dataset(&mut self, id: &str) -> Result<Dataset, Error> {
        let key = self.source_key(id)?;
        if let Some((k, ds)) = self.source_ds.get(id) {
            if *k == key {
                return Ok(ds.clone());
            }
        }
        let ds = match self.artifacts.load(&key, false) {
            Some(Output::Table(ds)) => ds,
            _ => {
                let tmp = self.artifacts.temp_path(&key);
                let src = self.analysis.source(id).unwrap().clone();
                let path = self.source_path(id)?;
                let ds = if src.format.is_database() {
                    // Copied out of the database once per version of the file.
                    let raw = self.artifacts.temp_path(&format!("{key}-db"));
                    let what = match &src.query {
                        Some(q) => crate::database::Read::Query(q),
                        None => crate::database::Read::Table(src.sheet.as_deref()),
                    };
                    let copied = crate::database::copy_to_parquet(&path, src.format, what, &raw)
                        .map_err(|e| match e {
                            Error::Validation(m) => Error::Validation(format!("source `{id}`: {m}")),
                            e => e,
                        })
                        .and_then(|_| self.engine.adopt_parquet(&raw, &tmp));
                    let _ = fs::remove_file(&raw);
                    copied
                } else {
                    self.engine.import_source(&path, src.format, src.sheet.as_deref(), &tmp)
                }
                .inspect_err(|_| {
                    let _ = fs::remove_file(&tmp);
                })?;
                self.artifacts.commit_table(&key, ds)?
            }
        };
        self.source_ds.insert(id.to_string(), (key, ds.clone()));
        Ok(ds)
    }

    /// Column types, missing values, distinct counts, min and max of a source.
    pub fn source_profile(&mut self, id: &str) -> Result<Vec<ColumnProfile>, Error> {
        let ds = self.source_dataset(id)?;
        self.engine.profile(&ds)
    }

    /// The report of a check step's current result (or of its last run, if
    /// it stopped there).
    pub fn check_report(&self, step_id: &str) -> Option<crate::check::CheckReport> {
        let key = self.plan().ok()?.keys.get(step_id)?.clone();
        crate::check::CheckReport::from_json(&self.artifacts.load_check(&key)?).ok()
    }

    pub fn output(&self, step_id: &str) -> Option<&Output> {
        self.results.get(step_id).map(|r| &r.output)
    }

    /// A page of a source, as converted to Parquet.
    pub fn source_preview(&mut self, id: &str, offset: u64, limit: u64) -> Result<Table, Error> {
        let ds = self.source_dataset(id)?;
        self.engine.preview(&ds, offset, limit)
    }

    /// A page of a step's held result (fresh or stale; check [`Self::state`]).
    pub fn preview(&mut self, step_id: &str, offset: u64, limit: u64) -> Result<Table, Error> {
        let ds = self
            .output(step_id)
            .and_then(Output::as_dataset)
            .cloned()
            .ok_or_else(|| Error::Validation(format!("step `{step_id}` has no table result")))?;
        self.engine.preview(&ds, offset, limit)
    }

    /// The whole held result in memory. Only for results known to be small.
    pub fn table(&mut self, step_id: &str) -> Result<Table, Error> {
        self.preview(step_id, 0, u64::MAX >> 1)
    }

    pub fn state(&self, step_id: &str) -> Result<StepState, Error> {
        let plan = self.plan()?;
        let key = plan.keys.get(step_id).ok_or_else(|| Error::Validation(format!("unknown step `{step_id}`")))?;
        if self.results.get(step_id).map(|r| &r.key) == Some(key) {
            return Ok(StepState::Fresh);
        }
        // Review state is derived live, so approving takes effect at once.
        if self.review.pending(&self.analysis.step(step_id).unwrap().kind) {
            return Ok(StepState::NeedsReview);
        }
        if self.upstream_needs_review(step_id) {
            return Ok(StepState::WaitingOnReview);
        }
        if let Some(run) = self.last_run.as_ref().and_then(|r| r.step(step_id)) {
            if &run.cache_key == key {
                match &run.status {
                    RunStatus::Failed { error } => return Ok(StepState::Failed(error.clone())),
                    RunStatus::Cancelled => return Ok(StepState::Cancelled),
                    // Approved since; nothing to report until it runs.
                    RunStatus::NeedsReview => {}
                    RunStatus::Skipped { reason } if reason.contains("waiting for your review") => {}
                    RunStatus::Skipped { reason } => return Ok(StepState::Blocked(reason.clone())),
                    RunStatus::Succeeded { .. } => {}
                }
            }
        }
        Ok(if self.results.contains_key(step_id) { StepState::Stale } else { StepState::NotRun })
    }

    fn upstream_needs_review(&self, step_id: &str) -> bool {
        let Some(step) = self.analysis.step(step_id) else { return false };
        step.inputs.iter().any(|id| match self.analysis.step(id) {
            Some(up) => self.review.pending(&up.kind) || self.upstream_needs_review(id),
            None => false,
        })
    }

    /// The dataset behind a source or a step's held table result.
    pub fn dataset_of(&mut self, id: &str) -> Option<Dataset> {
        if self.analysis.is_source(id) {
            return self.source_dataset(id).ok();
        }
        self.output(id).and_then(Output::as_dataset).cloned()
    }

    /// Per-column profile of a dataset, from the engine.
    /// The basic profile and the details of a source's or step's current
    /// columns (computed once per content).
    pub fn column_details(&mut self, id: &str) -> Result<(Vec<ColumnProfile>, Vec<crate::profile::ColumnDetail>), Error> {
        let ds = self.dataset_of(id).ok_or_else(|| Error::Validation(format!("`{id}` has no table result yet; run the analysis")))?;
        let profile = self.engine.profile(&ds)?;
        if let Some(d) = self.details.get(&ds.content_hash) {
            return Ok((profile, d.clone()));
        }
        let artifacts = &self.artifacts;
        let mut n = 0;
        let mut scratch = || {
            n += 1;
            artifacts.temp_path(&format!("details-{}-{n}", &ds.content_hash[..16.min(ds.content_hash.len())]))
        };
        let d = crate::profile::details(self.engine.as_mut(), &ds, &profile, &mut scratch)?;
        self.details.insert(ds.content_hash.clone(), d.clone());
        Ok((profile, d))
    }

    pub fn profile_dataset(&mut self, ds: &Dataset) -> Result<Vec<ColumnProfile>, Error> {
        self.engine.profile(ds)
    }

    pub fn python_runner(&self) -> Option<&PythonRunner> {
        self.python.as_ref()
    }

    /// Schema of a source or of a step's held result, if known without
    /// reading data (sources are converted on first use).
    pub fn schema_of(&mut self, id: &str) -> Option<Schema> {
        if self.analysis.is_source(id) {
            return self.source_dataset(id).ok().map(|d| d.schema);
        }
        self.output(id).and_then(Output::as_dataset).map(|d| d.schema.clone())
    }

    /// Inspectable SQL for a step, including joins whose inputs have results.
    pub fn pipeline_sql(&mut self, step_id: &str) -> Result<String, Error> {
        let mut schemas = HashMap::new();
        let ids: Vec<String> = self
            .analysis
            .sources
            .iter()
            .map(|s| s.id.clone())
            .chain(self.analysis.steps.iter().map(|s| s.id.clone()))
            .collect();
        for id in ids {
            if let Some(schema) = self.schema_of(&id) {
                schemas.insert(id, schema);
            }
        }
        compile::pipeline_sql_with(&self.analysis.bound(), step_id, &|id| schemas.get(id).cloned())
    }

    /// Run every step in dependency order, reusing results whose key is
    /// unchanged. Step failures are recorded, not returned as errors.
    pub fn run_all(&mut self) -> Result<RunRecord, Error> {
        self.run_all_with(&mut |_| {})
    }

    /// [`Self::run_all`], calling `on_step` as each step finishes (for progress).
    pub fn run_all_with(&mut self, on_step: &mut dyn FnMut(&StepRun)) -> Result<RunRecord, Error> {
        self.cancel.store(false, Ordering::SeqCst);
        let plan = self.plan()?;
        let hashes = self.source_hashes()?;
        let sources = self
            .analysis
            .sources
            .iter()
            .map(|s| SourceRun { id: s.id.clone(), path: s.path.clone(), sha256: hashes[&s.id].clone() })
            .collect();
        let started = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
        let mut record = RunRecord {
            run_id: format!("run-{started}"),
            started_unix_ms: started,
            engine: self.engine.fingerprint(),
            python: self.python_fp.clone(),
            sources,
            steps: Vec::new(),
            warnings: Vec::new(),
            trigger: None,
        };
        let mut halted: Option<String> = None;

        for id in &plan.order {
            let key = plan.keys[id].clone();
            let step = self.analysis.step(id).unwrap().clone();
            let t0 = Instant::now();

            // The first input that did not succeed, if any.
            let blocked = step.inputs.iter().filter(|i| !self.analysis.is_source(i)).find_map(|i| {
                match record.step(i).map(|r| r.status.clone()) {
                    Some(RunStatus::Succeeded { .. }) => None,
                    other => Some((i.clone(), other)),
                }
            });
            let status: RunStatus = if let Some(reason) = &halted {
                RunStatus::Skipped { reason: reason.clone() }
            } else if let Some((input, upstream)) = blocked {
                let reason = match upstream {
                    Some(RunStatus::NeedsReview) => format!("step `{input}` is waiting for your review"),
                    Some(RunStatus::Skipped { reason }) if reason.contains("waiting for your review") => reason,
                    _ => format!("upstream step `{input}` did not succeed"),
                };
                RunStatus::Skipped { reason }
            } else if self.cancel.load(Ordering::SeqCst) {
                halted = Some("run was cancelled".into());
                RunStatus::Cancelled
            } else if self.results.get(id).map(|r| &r.key) == Some(&key) {
                RunStatus::Succeeded { cached: true }
            } else if let Some(output) = self.artifacts.load(&key, !step.kind.produces_table()) {
                // A saved result is data, not code: showing it runs nothing.
                self.results.insert(id.clone(), CachedResult { key: key.clone(), output });
                RunStatus::Succeeded { cached: true }
            } else if self.review.pending(&step.kind) {
                RunStatus::NeedsReview
            } else {
                match self.execute(&step.id, &key) {
                    Ok(output) => {
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
                output: self.step_output.take().filter(|o| !o.trim().is_empty()),
            });
            on_step(record.steps.last().unwrap());
        }
        self.last_run = Some(record.clone());
        Ok(record)
    }

    fn input_dataset(&mut self, input: &str) -> Result<Dataset, Error> {
        if self.analysis.is_source(input) {
            return self.source_dataset(input);
        }
        self.results
            .get(input)
            .and_then(|r| r.output.as_dataset().cloned())
            .ok_or_else(|| Error::Validation(format!("no result for upstream step `{input}`")))
    }

    /// Execute one step and save its result under `key`.
    fn execute(&mut self, step_id: &str, key: &str) -> Result<Output, Error> {
        let written = self.analysis.step(step_id).unwrap().clone();
        let step = crate::params::bind_step(&written, &self.analysis.parameters);
        let inputs = step.inputs.iter().map(|i| self.input_dataset(i)).collect::<Result<Vec<_>, _>>()?;
        let input = inputs[0].clone();
        let tmp = self.artifacts.temp_path(key);
        let result = (|| -> Result<Output, Error> {
            match &step.kind {
                StepKind::Filter { .. }
                | StepKind::Aggregate { .. }
                | StepKind::Join { .. }
                | StepKind::Append { .. }
                | StepKind::Derive { .. }
                | StepKind::Select { .. }
                | StepKind::Sort { .. }
                | StepKind::Window { .. }
                | StepKind::Dedupe { .. }
                | StepKind::Pivot { .. }
                | StepKind::Unpivot { .. }
                | StepKind::Sql { .. } => {
                    let ds = self.engine.run_step(&step, &inputs, &tmp)?;
                    Ok(Output::Table(self.artifacts.commit_table(key, ds)?))
                }
                StepKind::Check { stop, .. } => {
                    let artifacts = &self.artifacts;
                    let mut n = 0;
                    let mut scratch = || {
                        n += 1;
                        artifacts.temp_path(&format!("{key}-check{n}"))
                    };
                    let report = crate::check::evaluate(self.engine.as_mut(), &step, &written, &input, &mut scratch)?;
                    self.step_output = Some(report.text());
                    self.artifacts.save_check(key, &report.to_json())?;
                    if *stop && report.failed().next().is_some() {
                        return Err(Error::Data(report.failure_summary()));
                    }
                    let ds = self.engine.run_step(&step, &inputs, &tmp)?;
                    Ok(Output::Table(self.artifacts.commit_table(key, ds)?))
                }
                StepKind::Test { .. } => {
                    let artifacts = &self.artifacts;
                    let mut n = 0;
                    let mut scratch = || {
                        n += 1;
                        artifacts.temp_path(&format!("{key}-test{n}"))
                    };
                    let table = crate::stat_test::evaluate(self.engine.as_mut(), &step, &input, &mut scratch)?;
                    if let Some(crate::table::Value::Text(reading)) = table.rows.first().and_then(|r| r.last()) {
                        self.step_output = Some(reading.clone());
                    }
                    let raw = self.artifacts.temp_path(&format!("{key}-raw"));
                    crate::duck::write_parquet(&table, &raw)?;
                    let adopted = self.engine.adopt_parquet(&raw, &tmp);
                    let _ = fs::remove_file(&raw);
                    Ok(Output::Table(self.artifacts.commit_table(key, adopted?)?))
                }
                StepKind::Python { code, timeout_ms } => {
                    let runner = self.python.as_ref().ok_or_else(|| {
                        Error::Python("no Python interpreter is configured for this project".into())
                    })?;
                    if self.python_fp.is_none() {
                        return Err(Error::Python(format!(
                            "interpreter {:?} is not usable (it needs pandas and pyarrow)",
                            runner.interpreter
                        )));
                    }
                    let raw = self.artifacts.temp_path(key);
                    let named: Vec<(String, PathBuf)> =
                        step.inputs.iter().cloned().zip(inputs.iter().map(|d| d.path.clone())).collect();
                    let params = crate::params::python_json(&self.analysis.parameters);
                    let run = runner.run_with_params(code, &named, &params, &raw, Duration::from_millis(*timeout_ms), &self.cancel);
                    self.step_output = Some(run.output);
                    let ran = run.result;
                    let adopted = ran.and_then(|_| self.engine.adopt_parquet(&raw, &tmp));
                    let _ = fs::remove_file(&raw);
                    Ok(Output::Table(self.artifacts.commit_table(key, adopted?)?))
                }
                StepKind::Chart { .. } => {
                    compile::output_schema(&step, std::slice::from_ref(&input.schema))?;
                    let artifacts = &self.artifacts;
                    let mut n = 0;
                    let mut scratch = || {
                        n += 1;
                        artifacts.temp_path(&format!("{key}-chart{n}"))
                    };
                    let data = chart::data(self.engine.as_mut(), &step, &input, &mut scratch)?;
                    let spec = chart::vega_lite(&step.kind, &data)?;
                    self.artifacts.save_chart(key, &spec)?;
                    Ok(Output::Chart(spec))
                }
            }
        })();
        let _ = fs::remove_file(&tmp);
        result
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
    let mut session = Session::new(analysis, project_dir, engine, python)?.without_saved_results()?;
    let run = session.run_all()?;

    let mut env = Vec::new();
    if run.engine != recorded.engine {
        env.push(format!("engine: {} -> {}", recorded.engine, run.engine));
    }
    if run.python != recorded.python {
        env.push(format!("python: {:?} -> {:?}", recorded.python, run.python));
    }
    for now in &run.sources {
        match recorded.sources.iter().find(|s| s.id == now.id) {
            Some(then) if then.sha256 != now.sha256 => env.push(format!("source `{}` content changed", now.id)),
            None => env.push(format!("source `{}` was added", now.id)),
            _ => {}
        }
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
        for input in &s.inputs {
            h.update(input.as_bytes());
        }
        h.update(crate::persist::step_kind_to_json(&s.kind).to_string_compact().as_bytes());
    }
    h.hex_digest()
}
