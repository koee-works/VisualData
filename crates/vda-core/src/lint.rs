//! Checks an analysis without running it, and points out results from the
//! last run that are probably not what was meant (ADR-0007).
//!
//! Static checks work on the columns each step will receive: sources'
//! columns, carried through visual steps by their compiler, and the held
//! result of custom SQL and Python steps when it is current. So a column
//! that disappears upstream is reported at every step that uses it, before
//! anything runs. Code is parsed, never executed: SQL by DuckDB's parser,
//! Python by `ast.parse` in an isolated interpreter (parsing runs no user
//! code, so code waiting for review is checked too).
//!
//! Warnings from data say "on the last run" and come from the held
//! results; they are computed once per result (by content hash).

use crate::compile;
use crate::dataset::Dataset;
use crate::formula;
use crate::json::Json;
use crate::model::{Analysis, JoinKind, Step, StepKind};
use crate::session::{Output, Session, StepState};
use crate::table::{ColumnProfile, Schema};
use crate::Error;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The step cannot run as it is.
    Error,
    /// It runs, but probably not as meant.
    Warning,
    /// Worth knowing.
    Info,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable name of the check, used to dismiss it: `formula`, `join-multiplies`, ...
    pub code: &'static str,
    /// The step or source it is about; `None` for the whole analysis.
    pub step: Option<String>,
    /// The part of the step: `columns.1.formula`, `sql`, `code`, `on.0`.
    pub field: Option<String>,
    /// Where in that text: 1-based character (formulas) or line (code).
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub message: String,
    /// Comes from the held results of the last run.
    pub from_last_run: bool,
}

impl Diagnostic {
    fn new(severity: Severity, code: &'static str, step: Option<&str>, message: impl Into<String>) -> Self {
        Diagnostic {
            severity,
            code,
            step: step.map(str::to_string),
            field: None,
            line: None,
            column: None,
            message: message.into(),
            from_last_run: false,
        }
    }
    fn at(mut self, field: impl Into<String>, line: Option<usize>, column: Option<usize>) -> Self {
        self.field = Some(field.into());
        self.line = line;
        self.column = column;
        self
    }
    fn last_run(mut self) -> Self {
        self.from_last_run = true;
        self
    }
}

/// A Python step's code as `ast.parse` sees it.
#[derive(Debug, Clone, PartialEq)]
struct PythonFacts {
    error: Option<(String, Option<usize>, Option<usize>)>,
    assigns_result: bool,
    row_loops: Vec<usize>,
}

const PY_CHECK: &str = r#"
import ast, json, sys
src = sys.stdin.read()
out = {"error": None, "assigns_result": False, "row_loops": []}
try:
    tree = ast.parse(src, "<python step>")
except SyntaxError as e:
    out["error"] = [e.msg, e.lineno, e.offset]
else:
    for n in ast.walk(tree):
        if isinstance(n, ast.Name) and n.id == "result" and isinstance(n.ctx, ast.Store):
            out["assigns_result"] = True
        if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute):
            if n.func.attr in ("iterrows", "itertuples"):
                out["row_loops"].append(n.lineno)
            if n.func.attr == "apply" and any(k.arg == "axis" and isinstance(k.value, ast.Constant) and k.value.value in (1, "columns") for k in n.keywords):
                out["row_loops"].append(n.lineno)
sys.stdout.write(json.dumps(out))
"#;

/// Bar charts with more bars than this are hard to read.
pub const MAX_BARS: usize = 50;
/// Results larger than this are not profiled for warnings.
pub const MAX_PROFILED_ROWS: u64 = 5_000_000;

/// Runs checks, remembering what is expensive to recompute.
pub struct Linter {
    conn: duckdb::Connection,
    python: HashMap<String, Option<PythonFacts>>,
    profiles: HashMap<String, Vec<ColumnProfile>>,
}

impl Linter {
    pub fn new() -> Result<Self, Error> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| Error::Data(format!("cannot start the checker: {e}")))?;
        Ok(Linter { conn, python: HashMap::new(), profiles: HashMap::new() })
    }

    /// Everything worth reporting about the analysis, errors first.
    pub fn lint(&mut self, s: &mut Session) -> Vec<Diagnostic> {
        let a = s.analysis.clone();
        let mut out = Vec::new();
        for src in &a.sources {
            if let Ok(path) = s.source_path(&src.id) {
                if !path.exists() {
                    out.push(Diagnostic::new(
                        Severity::Error,
                        "source-missing",
                        Some(&src.id),
                        format!("the file {} is missing", src.path),
                    ));
                }
            }
        }
        self.schemas(s, &a, &mut out);
        let order = crate::plan::validate(&a).unwrap_or_else(|_| a.steps.iter().map(|x| x.id.clone()).collect());
        let pending: HashSet<String> = s.pending_review().into_iter().collect();
        let mut outdated = 0;
        for id in &order {
            let step = a.step(id).unwrap().clone();
            self.code_checks(s, &a, &step, &mut out);
            let state = s.state(id).ok();
            match &state {
                Some(StepState::Failed(e)) => {
                    out.push(Diagnostic::new(Severity::Error, "failed", Some(id), format!("failed on the last run: {e}")).last_run())
                }
                Some(StepState::Stale) => outdated += 1,
                _ => {}
            }
            if pending.contains(id) {
                out.push(Diagnostic::new(Severity::Info, "needs-review", Some(id), "its Python code is waiting for your review"));
            }
            if state == Some(StepState::Fresh) {
                self.data_checks(s, &step, &mut out);
            }
            let read = a.readers(id).next().is_some();
            let shown = a.cells.iter().any(|c| matches!(c, crate::model::Cell::Step { step_id } if step_id == id));
            if !read && !shown && a.steps.len() > 1 {
                out.push(Diagnostic::new(
                    Severity::Info,
                    "unused",
                    Some(id),
                    "no step reads it and the notebook does not show it",
                ));
            }
        }
        if outdated > 0 {
            out.push(Diagnostic::new(
                Severity::Info,
                "outdated",
                None,
                format!("{outdated} result{} outdated; run the analysis to update", if outdated == 1 { " is" } else { "s are" }),
            ));
        }
        out.sort_by_key(|d| d.severity);
        out
    }

    /// Checks for a step being edited (not yet saved): its definition
    /// against the columns its inputs will have. No data checks.
    pub fn check_draft(&mut self, s: &mut Session, draft: &Step) -> Vec<Diagnostic> {
        let mut a = s.analysis.clone();
        match a.step_mut(&draft.id) {
            Some(st) => *st = draft.clone(),
            None => a.steps.push(draft.clone()),
        }
        let mut out = Vec::new();
        if let Err(e) = crate::plan::validate(&a) {
            out.push(Diagnostic::new(Severity::Error, "invalid-step", Some(&draft.id), plain(&e)));
            return out;
        }
        let mut all = Vec::new();
        self.schemas(s, &a, &mut all);
        out.extend(all.into_iter().filter(|d| d.step.as_deref() == Some(&draft.id)));
        self.code_checks(s, &a, draft, &mut out);
        out.sort_by_key(|d| d.severity);
        out
    }

    /// Output columns of every source and step that can be known without
    /// running, reporting steps that cannot work on their inputs.
    fn schemas(&mut self, s: &mut Session, a: &Analysis, out: &mut Vec<Diagnostic>) -> HashMap<String, Schema> {
        let mut known: HashMap<String, Schema> = HashMap::new();
        for src in &a.sources {
            if let Some(ds) = s.dataset_of(&src.id) {
                known.insert(src.id.clone(), ds.schema);
            }
        }
        let Ok(order) = crate::plan::validate(a) else { return known };
        for id in order {
            let step = a.step(&id).unwrap();
            let inputs: Option<Vec<Schema>> = step.inputs.iter().map(|i| known.get(i).cloned()).collect();
            match (&step.kind, inputs) {
                (StepKind::Sql { .. } | StepKind::Python { .. }, _) => {
                    // Known only from a current result.
                    if s.state(&id).ok() == Some(StepState::Fresh) {
                        if let Some(Output::Table(ds)) = s.output(&id) {
                            known.insert(id.clone(), ds.schema.clone());
                        }
                    }
                }
                (StepKind::Chart { .. }, Some(inputs)) => {
                    if let Err(e) = compile::output_schema(step, &inputs) {
                        out.push(Diagnostic::new(Severity::Error, "invalid-step", Some(&id), plain(&e)));
                    }
                }
                (StepKind::Derive { columns }, Some(inputs)) => {
                    // Per column, so each formula's problem is shown at that formula.
                    let mut schema = inputs[0].clone();
                    let mut ok = true;
                    for (i, c) in columns.iter().enumerate() {
                        let field = format!("columns.{i}.formula");
                        if c.name.trim().is_empty() {
                            out.push(Diagnostic::new(Severity::Error, "formula", Some(&id), "name this column").at(format!("columns.{i}.name"), None, None));
                            ok = false;
                            continue;
                        }
                        match formula::parse(&c.formula).and_then(|e| formula::result_type(&e, &schema)) {
                            Ok(t) => match schema.fields.iter_mut().find(|f| f.name == c.name) {
                                Some(f) => f.dtype = t,
                                None => schema.fields.push(crate::table::Field { name: c.name.clone(), dtype: t }),
                            },
                            Err(m) => {
                                let at = character_of(&m);
                                out.push(Diagnostic::new(Severity::Error, "formula", Some(&id), m).at(field, None, at));
                                ok = false;
                            }
                        }
                    }
                    if ok {
                        known.insert(id.clone(), schema);
                    }
                }
                (_, Some(inputs)) => match compile::output_schema(step, &inputs) {
                    Ok(Some(schema)) => {
                        known.insert(id.clone(), schema);
                    }
                    Ok(None) => {}
                    Err(e) => out.push(Diagnostic::new(Severity::Error, "invalid-step", Some(&id), plain(&e))),
                },
                (_, None) => {}
            }
        }
        known
    }

    fn code_checks(&mut self, s: &Session, a: &Analysis, step: &Step, out: &mut Vec<Diagnostic>) {
        match &step.kind {
            StepKind::Sql { sql } => {
                let sql = sql.trim().trim_end_matches(';');
                if let Err(m) = crate::duck::check_single_select(&self.conn, sql) {
                    out.push(Diagnostic::new(Severity::Error, "sql", Some(&step.id), m).at("sql", None, None));
                    return;
                }
                for t in self.sql_tables(sql) {
                    let known = t.eq_ignore_ascii_case("input") || step.inputs.iter().any(|i| i.eq_ignore_ascii_case(&t));
                    if !known {
                        let hint = if a.is_source(&t) || a.step(&t).is_some() {
                            format!("add `{t}` as an input of this step")
                        } else {
                            "use `input`, or a table this step reads".to_string()
                        };
                        out.push(
                            Diagnostic::new(
                                Severity::Error,
                                "sql-table",
                                Some(&step.id),
                                format!("reads `{t}`, which is not one of this step's inputs; {hint}"),
                            )
                            .at("sql", None, None),
                        );
                    }
                }
            }
            StepKind::Python { code, .. } => {
                let Some(facts) = self.python_facts(s, code) else { return };
                if let Some((msg, line, col)) = facts.error {
                    out.push(
                        Diagnostic::new(Severity::Error, "python-syntax", Some(&step.id), format!("syntax error: {msg}"))
                            .at("code", line, col),
                    );
                    return;
                }
                if !facts.assigns_result {
                    out.push(
                        Diagnostic::new(Severity::Error, "python-result", Some(&step.id), "never assigns a DataFrame to `result`")
                            .at("code", None, None),
                    );
                }
                for line in facts.row_loops {
                    out.push(
                        Diagnostic::new(
                            Severity::Warning,
                            "row-loop",
                            Some(&step.id),
                            "works through rows one at a time; operations on whole columns are usually far faster on large inputs",
                        )
                        .at("code", Some(line), None),
                    );
                }
            }
            _ => {}
        }
    }

    /// Base tables a query names (not table functions, not its own CTEs).
    fn sql_tables(&self, sql: &str) -> Vec<String> {
        let Ok(raw) = self.conn.query_row("SELECT json_serialize_sql(?)", [sql], |r| r.get::<_, String>(0)) else {
            return vec![];
        };
        let Ok(tree) = Json::parse(&raw) else { return vec![] };
        let (mut tables, mut ctes) = (Vec::new(), Vec::new());
        fn walk(n: &Json, tables: &mut Vec<String>, ctes: &mut Vec<String>) {
            match n {
                Json::Obj(m) => {
                    if m.get("type").and_then(Json::as_str) == Some("BASE_TABLE")
                        && m.get("schema_name").and_then(Json::as_str).unwrap_or("").is_empty()
                    {
                        if let Some(t) = m.get("table_name").and_then(Json::as_str) {
                            tables.push(t.to_string());
                        }
                    }
                    if let Some(Json::Arr(entries)) = m.get("cte_map").and_then(|c| c.get("map")) {
                        ctes.extend(entries.iter().filter_map(|e| e.get("key").and_then(Json::as_str).map(str::to_string)));
                    }
                    m.values().for_each(|v| walk(v, tables, ctes));
                }
                Json::Arr(items) => items.iter().for_each(|v| walk(v, tables, ctes)),
                _ => {}
            }
        }
        walk(&tree, &mut tables, &mut ctes);
        let mut seen = HashSet::new();
        tables
            .into_iter()
            .filter(|t| !ctes.iter().any(|c| c.eq_ignore_ascii_case(t)))
            .filter(|t| seen.insert(t.to_ascii_lowercase()))
            .collect()
    }

    fn python_facts(&mut self, s: &Session, code: &str) -> Option<PythonFacts> {
        let key = crate::hash::sha256_hex(code.as_bytes());
        if let Some(f) = self.python.get(&key) {
            return f.clone();
        }
        let facts = s.python_runner().and_then(|r| parse_python(&r.interpreter, code));
        self.python.insert(key, facts.clone());
        facts
    }

    fn profile(&mut self, s: &mut Session, ds: &Dataset) -> Option<Vec<ColumnProfile>> {
        if ds.rows > MAX_PROFILED_ROWS {
            return None;
        }
        if let Some(p) = self.profiles.get(&ds.content_hash) {
            return Some(p.clone());
        }
        let p = s.profile_dataset(ds).ok()?;
        self.profiles.insert(ds.content_hash.clone(), p.clone());
        Some(p)
    }

    fn data_checks(&mut self, s: &mut Session, step: &Step, out: &mut Vec<Diagnostic>) {
        let id = step.id.as_str();
        let result = s.dataset_of(id);
        let first = s.dataset_of(step.input());
        match (&step.kind, &result, &first) {
            (StepKind::Join { how, .. }, Some(r), Some(l)) => {
                if r.rows == 0 && l.rows > 0 {
                    out.push(Diagnostic::new(Severity::Warning, "no-rows", Some(id), "no rows matched on the last run; check the conditions and the columns' values").last_run());
                } else if matches!(how, JoinKind::Inner | JoinKind::Left) && r.rows > l.rows {
                    out.push(
                        Diagnostic::new(
                            Severity::Warning,
                            "join-multiplies",
                            Some(id),
                            format!(
                                "gave {} rows from {} on the left on the last run: some rows match more than one row on the right. If each should match once, the right side has repeated keys",
                                r.rows, l.rows
                            ),
                        )
                        .last_run(),
                    );
                }
            }
            (StepKind::Filter { .. } | StepKind::Sql { .. }, Some(r), Some(l)) if r.rows == 0 && l.rows > 0 => {
                out.push(Diagnostic::new(Severity::Warning, "no-rows", Some(id), format!("kept none of {} rows on the last run", l.rows)).last_run());
            }
            (StepKind::Derive { columns }, Some(r), _) if r.rows > 0 => {
                let r = r.clone();
                if let Some(profile) = self.profile(s, &r) {
                    for (i, c) in columns.iter().enumerate() {
                        if profile.iter().any(|p| p.name == c.name && p.nulls == p.rows) {
                            out.push(
                                Diagnostic::new(
                                    Severity::Warning,
                                    "all-missing",
                                    Some(id),
                                    format!("`{}` is missing in every row on the last run; check the columns it uses for missing values or zeros", c.name),
                                )
                                .at(format!("columns.{i}.formula"), None, None)
                                .last_run(),
                            );
                        }
                    }
                }
            }
            (StepKind::Chart { mark: crate::model::Mark::Bar, x, .. }, _, Some(l)) => {
                let l = l.clone();
                if let Some(profile) = self.profile(s, &l) {
                    if let Some(p) = profile.iter().find(|p| &p.name == x) {
                        if p.distinct > MAX_BARS {
                            out.push(
                                Diagnostic::new(
                                    Severity::Warning,
                                    "chart-categories",
                                    Some(id),
                                    format!("draws {} bars; group the data or keep the top values with a sort first", p.distinct),
                                )
                                .last_run(),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// A validation message without the step prefix the core adds for logs.
fn plain(e: &Error) -> String {
    let m = e.to_string();
    let m = m.strip_prefix("invalid analysis: ").unwrap_or(&m).to_string();
    match m.split_once("`: ") {
        Some((head, rest)) if head.starts_with("step `") => rest.to_string(),
        _ => m,
    }
}

/// The character a formula message points at ("... at character 7").
fn character_of(message: &str) -> Option<usize> {
    let rest = message.split("at character ").nth(1)?;
    rest.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()
}

/// Parse Python code with `ast.parse` in an isolated interpreter.
fn parse_python(interpreter: &std::path::Path, code: &str) -> Option<PythonFacts> {
    let mut child = Command::new(interpreter)
        .args(["-I", "-c", PY_CHECK])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(code.as_bytes()).ok()?;
    let start = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let out = child.wait_with_output().ok()?;
    let j = Json::parse(&String::from_utf8_lossy(&out.stdout)).ok()?;
    let error = match j.get("error") {
        Some(Json::Arr(e)) => Some((
            e.first().and_then(Json::as_str).unwrap_or("invalid syntax").to_string(),
            e.get(1).and_then(Json::as_f64).map(|n| n as usize),
            e.get(2).and_then(Json::as_f64).map(|n| n as usize),
        )),
        _ => None,
    };
    Some(PythonFacts {
        error,
        assigns_result: j.get("assigns_result") == Some(&Json::Bool(true)),
        row_loops: match j.get("row_loops") {
            Some(Json::Arr(l)) => l.iter().filter_map(Json::as_f64).map(|n| n as usize).collect(),
            _ => vec![],
        },
    })
}
