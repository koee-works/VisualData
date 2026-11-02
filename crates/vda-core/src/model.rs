//! The analysis model: one source, a set of steps that form a dependency
//! graph, and notebook cells in document order.
//!
//! Execution order comes from the step graph; document order comes from
//! `cells`. The two are deliberately separate (see the designer brief).

use crate::table::Value;

/// Reserved id that steps use to read the analysis source.
pub const SOURCE_ID: &str = "source";

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub name: String,
    pub source: Source,
    pub steps: Vec<Step>,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// Path relative to the project folder (or absolute).
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: String,
    /// Id of the upstream step, or [`SOURCE_ID`].
    pub input: String,
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
    /// Custom SQL. Reads the upstream result as the relation `input`.
    Sql { sql: String },
    /// Custom Python. Receives the upstream result as `df` (pandas) and must
    /// assign a DataFrame to `result`.
    Python { code: String, timeout_ms: u64 },
    /// Visual: chart specification over the upstream result.
    Chart { mark: Mark, x: String, y: String, title: Option<String> },
}

impl StepKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            StepKind::Filter { .. } => "filter",
            StepKind::Aggregate { .. } => "aggregate",
            StepKind::Sql { .. } => "sql",
            StepKind::Python { .. } => "python",
            StepKind::Chart { .. } => "chart",
        }
    }

    /// Whether the step is edited through visual controls. Custom SQL and
    /// Python are never converted back into visual controls.
    pub fn is_visual(&self) -> bool {
        matches!(self, StepKind::Filter { .. } | StepKind::Aggregate { .. } | StepKind::Chart { .. })
    }

    /// Whether the step produces a table other steps can read.
    pub fn produces_table(&self) -> bool {
        !matches!(self, StepKind::Chart { .. })
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

impl Analysis {
    pub fn step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }

    pub fn step_mut(&mut self, id: &str) -> Option<&mut Step> {
        self.steps.iter_mut().find(|s| s.id == id)
    }
}
