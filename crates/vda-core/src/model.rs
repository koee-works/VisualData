//! The analysis model: one or more sources, a set of steps that form a
//! dependency graph (a step may read several inputs), and notebook cells in
//! document order.
//!
//! Execution order comes from the step graph; document order comes from
//! `cells`. The two are deliberately separate (see the designer brief).

use crate::table::Value;

/// Id of an analysis's first source. Projects from format 1 had only this one.
pub const SOURCE_ID: &str = "source";

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub name: String,
    /// Data files the steps read. Ids share one namespace with step ids.
    pub sources: Vec<Source>,
    pub steps: Vec<Step>,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// How steps (and custom SQL) refer to this source.
    pub id: String,
    /// Path relative to the project folder (or absolute).
    pub path: String,
    pub format: SourceFormat,
    /// For spreadsheets: the sheet to read. Stored by name, so reordering
    /// the workbook's sheets does not change the data.
    pub sheet: Option<String>,
}

impl Source {
    /// A source whose format follows from its file extension.
    pub fn new(id: impl Into<String>, path: impl Into<String>) -> Self {
        let path = path.into();
        Source { id: id.into(), format: SourceFormat::from_path(&path).unwrap_or(SourceFormat::Csv), path, sheet: None }
    }

    /// The first source of an analysis, with id [`SOURCE_ID`].
    pub fn primary(path: impl Into<String>) -> Self {
        Source::new(SOURCE_ID, path)
    }
}

/// File formats a source can be read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Csv,
    Parquet,
    /// A JSON array of objects, or one object per line.
    Json,
    /// A spreadsheet: xlsx, xlsm, xlsb, xls or ods.
    Excel,
}

impl SourceFormat {
    pub fn name(self) -> &'static str {
        match self {
            SourceFormat::Csv => "csv",
            SourceFormat::Parquet => "parquet",
            SourceFormat::Json => "json",
            SourceFormat::Excel => "excel",
        }
    }

    pub fn parse(s: &str) -> Option<SourceFormat> {
        [SourceFormat::Csv, SourceFormat::Parquet, SourceFormat::Json, SourceFormat::Excel]
            .into_iter()
            .find(|f| f.name() == s)
    }

    /// The format a file name's extension implies, if it is one the app reads.
    pub fn from_path(path: &str) -> Option<SourceFormat> {
        let ext = std::path::Path::new(path).extension()?.to_string_lossy().to_ascii_lowercase();
        match ext.as_str() {
            "csv" | "txt" => Some(SourceFormat::Csv),
            "parquet" | "pq" => Some(SourceFormat::Parquet),
            "json" | "jsonl" | "ndjson" => Some(SourceFormat::Json),
            e if crate::excel::EXTENSIONS.contains(&e) => Some(SourceFormat::Excel),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: String,
    /// Ids of the steps or sources this step reads, at least one. The first
    /// is the main input (`input` in SQL, `df` in Python); a join reads two.
    pub inputs: Vec<String>,
    /// Display name only. Changing it does not invalidate results.
    pub label: Option<String>,
    pub kind: StepKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StepKind {
    /// Visual: keep rows where every predicate holds (AND).
    Filter { predicates: Vec<Predicate> },
    /// Visual: group and summarize.
    Aggregate { group_by: Vec<String>, measures: Vec<Measure> },
    /// Visual: combine the first input (left) with the second (right) on
    /// pairs of equal key columns.
    Join { how: JoinKind, on: Vec<JoinKey> },
    /// Visual: stack the rows of every input, matching columns by name. A
    /// column an input lacks is missing in its rows. Optionally drop
    /// repeated rows, and record which input each row came from.
    Append { distinct: bool, origin: Option<String> },
    /// Custom SQL. Reads the first input as the relation `input`, and every
    /// input by its id.
    Sql { sql: String },
    /// Custom Python. Receives the first input as `df` and every input in the
    /// dict `dfs` by id (pandas), and must assign a DataFrame to `result`.
    Python { code: String, timeout_ms: u64 },
    /// Visual: chart specification over the upstream result.
    Chart { mark: Mark, x: String, y: String, title: Option<String> },
}

impl StepKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            StepKind::Filter { .. } => "filter",
            StepKind::Aggregate { .. } => "aggregate",
            StepKind::Join { .. } => "join",
            StepKind::Append { .. } => "append",
            StepKind::Sql { .. } => "sql",
            StepKind::Python { .. } => "python",
            StepKind::Chart { .. } => "chart",
        }
    }

    /// Whether the step is edited through visual controls. Custom SQL and
    /// Python are never converted back into visual controls.
    pub fn is_visual(&self) -> bool {
        matches!(
            self,
            StepKind::Filter { .. }
                | StepKind::Aggregate { .. }
                | StepKind::Join { .. }
                | StepKind::Append { .. }
                | StepKind::Chart { .. }
        )
    }

    /// Whether the step produces a table other steps can read.
    pub fn produces_table(&self) -> bool {
        !matches!(self, StepKind::Chart { .. })
    }

    /// How many inputs the step takes: `(min, max)`; `None` means any number.
    pub fn arity(&self) -> (usize, Option<usize>) {
        match self {
            StepKind::Join { .. } => (2, Some(2)),
            StepKind::Append { .. } => (2, None),
            StepKind::Sql { .. } | StepKind::Python { .. } => (1, None),
            _ => (1, Some(1)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    /// Rows with a match on both sides.
    Inner,
    /// Every left row; right columns are missing where nothing matches.
    Left,
    /// Every row from both sides.
    Full,
}

impl JoinKind {
    pub fn name(self) -> &'static str {
        match self {
            JoinKind::Inner => "inner",
            JoinKind::Left => "left",
            JoinKind::Full => "full",
        }
    }
    pub fn sql(self) -> &'static str {
        match self {
            JoinKind::Inner => "INNER JOIN",
            JoinKind::Left => "LEFT JOIN",
            JoinKind::Full => "FULL OUTER JOIN",
        }
    }
    pub fn parse(s: &str) -> Option<JoinKind> {
        [JoinKind::Inner, JoinKind::Left, JoinKind::Full].into_iter().find(|k| k.name() == s)
    }
}

/// A condition between a left and a right column: equal (`=`, the usual
/// key), or a comparison (`<`, `<=`, `>`, `>=`, `<>`) for range joins such
/// as "order date on or after the price's start date". Missing values never
/// match. A pair of rows matches when every condition holds.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinKey {
    pub left: String,
    pub op: CmpOp,
    pub right: String,
}

impl JoinKey {
    pub fn equal(left: impl Into<String>, right: impl Into<String>) -> Self {
        JoinKey { left: left.into(), op: CmpOp::Eq, right: right.into() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Predicate {
    pub column: String,
    pub op: CmpOp,
    /// Ignored for `IsNull` / `IsNotNull`.
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    IsNull,
    IsNotNull,
}

impl CmpOp {
    pub fn token(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "<>",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
            CmpOp::IsNull => "IS NULL",
            CmpOp::IsNotNull => "IS NOT NULL",
        }
    }
    pub fn from_token(s: &str) -> Option<CmpOp> {
        [
            CmpOp::Eq,
            CmpOp::Ne,
            CmpOp::Lt,
            CmpOp::Le,
            CmpOp::Gt,
            CmpOp::Ge,
            CmpOp::IsNull,
            CmpOp::IsNotNull,
        ]
        .into_iter()
        .find(|o| o.token() == s)
    }
    pub fn takes_value(self) -> bool {
        !matches!(self, CmpOp::IsNull | CmpOp::IsNotNull)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Measure {
    pub func: AggFunc,
    /// `None` only for `Count`, meaning COUNT(*).
    pub column: Option<String>,
    pub alias: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl AggFunc {
    pub fn sql(self) -> &'static str {
        match self {
            AggFunc::Count => "COUNT",
            AggFunc::Sum => "SUM",
            AggFunc::Avg => "AVG",
            AggFunc::Min => "MIN",
            AggFunc::Max => "MAX",
        }
    }
    pub fn from_sql(s: &str) -> Option<AggFunc> {
        [AggFunc::Count, AggFunc::Sum, AggFunc::Avg, AggFunc::Min, AggFunc::Max]
            .into_iter()
            .find(|f| f.sql() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Bar,
    Line,
    Point,
}

impl Mark {
    pub fn name(self) -> &'static str {
        match self {
            Mark::Bar => "bar",
            Mark::Line => "line",
            Mark::Point => "point",
        }
    }
    pub fn parse(s: &str) -> Option<Mark> {
        [Mark::Bar, Mark::Line, Mark::Point].into_iter().find(|m| m.name() == s)
    }
}

/// Notebook cells in document order.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Markdown { text: String },
    /// Shows a step's code/SQL, state and result in the notebook.
    Step { step_id: String },
}

impl Step {
    /// The main input.
    pub fn input(&self) -> &str {
        &self.inputs[0]
    }
}

impl Analysis {
    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn is_source(&self, id: &str) -> bool {
        self.source(id).is_some()
    }

    /// Steps that read `id` (directly).
    pub fn readers<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Step> + 'a {
        self.steps.iter().filter(move |s| s.inputs.iter().any(|i| i == id))
    }

    pub fn step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }

    pub fn step_mut(&mut self, id: &str) -> Option<&mut Step> {
        self.steps.iter_mut().find(|s| s.id == id)
    }
}
