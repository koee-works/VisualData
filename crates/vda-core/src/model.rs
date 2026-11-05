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
    /// Named values steps can refer to (`@name` in formulas, `$name` in SQL).
    pub parameters: Vec<crate::params::Parameter>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// How steps (and custom SQL) refer to this source.
    pub id: String,
    /// Path relative to the project folder (or absolute).
    pub path: String,
    pub format: SourceFormat,
    /// For spreadsheets: the sheet to read; for databases, the table. Stored
    /// by name, so reordering sheets or adding tables changes nothing.
    pub sheet: Option<String>,
    /// For databases: a read-only query, read instead of a table.
    pub query: Option<String>,
}

impl Source {
    /// A source whose format follows from its file extension.
    pub fn new(id: impl Into<String>, path: impl Into<String>) -> Self {
        let path = path.into();
        Source { id: id.into(), format: SourceFormat::from_path(&path).unwrap_or(SourceFormat::Csv), path, sheet: None, query: None }
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
    /// A SQLite database file: a table or a query.
    Sqlite,
    /// A DuckDB database file: a table or a query.
    DuckDb,
}

impl SourceFormat {
    pub fn name(self) -> &'static str {
        match self {
            SourceFormat::Csv => "csv",
            SourceFormat::Parquet => "parquet",
            SourceFormat::Json => "json",
            SourceFormat::Excel => "excel",
            SourceFormat::Sqlite => "sqlite",
            SourceFormat::DuckDb => "duckdb",
        }
    }

    pub fn parse(s: &str) -> Option<SourceFormat> {
        [SourceFormat::Csv, SourceFormat::Parquet, SourceFormat::Json, SourceFormat::Excel, SourceFormat::Sqlite, SourceFormat::DuckDb]
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
            "sqlite" | "sqlite3" | "db3" => Some(SourceFormat::Sqlite),
            "duckdb" | "ddb" => Some(SourceFormat::DuckDb),
            _ => None,
        }
    }

    /// A database file, read as a table or a query.
    pub fn is_database(self) -> bool {
        matches!(self, SourceFormat::Sqlite | SourceFormat::DuckDb)
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
    /// Visual: add or replace columns computed by formulas, in order; each
    /// formula sees the columns before it, including earlier ones here.
    Derive { columns: Vec<Derived> },
    /// Visual: keep these columns, in this order, optionally renamed.
    Select { columns: Vec<Pick> },
    /// Visual: order rows (missing values last), ties in input order, then
    /// optionally keep only the first `limit` rows.
    Sort { by: Vec<SortKey>, limit: Option<u64> },
    /// Visual: calculations across rows (running totals, ranks, previous
    /// values) within groups, in an order; rows keep their input order.
    Window { partition_by: Vec<String>, order_by: Vec<SortKey>, columns: Vec<WindowColumn> },
    /// Visual: keep one row per key: the first in `order_by` (ties in input
    /// order). No keys means rows identical in every column.
    Dedupe { keys: Vec<String>, order_by: Vec<SortKey> },
    /// Visual: one row per value of `column` becomes one column per value,
    /// summarizing `value` for each group of `rows`. The new columns are
    /// known only after it runs.
    Pivot { rows: Vec<String>, column: String, value: Option<String>, func: AggFunc },
    /// Visual: columns become rows: each input row gives one row per listed
    /// column, with the column's name and value.
    Unpivot { columns: Vec<String>, name: String, value: String },
    /// Visual: rules the data must meet. Rows pass through unchanged; with
    /// `stop`, a broken rule fails the step (and what reads it waits).
    Check { rules: Vec<Rule>, stop: bool },
    /// Visual: a statistical test of columns `a` and `b` (see [`TestKind`]),
    /// giving one row: the statistic, the p-value and a sentence reading it.
    Test { test: TestKind, a: String, b: String },
    /// Visual: find missing values in some columns (empty cells, values that
    /// stand for missing such as "N/A" or -999, and rows a formula picks
    /// out) and fill them in, each column its own way (see clean.rs).
    /// `markers` apply to every listed column; `flag` adds a `<column>_missing`
    /// column per listed column; `seed` makes random draws repeatable.
    /// `internal` (set by the app, never saved) keeps dropped rows and adds
    /// the columns its report is counted from (`__vda_r`, `__vda_w<n>`,
    /// `__vda_drop`).
    Clean { markers: Vec<String>, columns: Vec<CleanColumn>, flag: bool, seed: u64, internal: bool },
    /// Custom SQL. Reads the first input as the relation `input`, and every
    /// input by its id.
    Sql { sql: String },
    /// Custom Python. Receives the first input as `df` and every input in the
    /// dict `dfs` by id (pandas), and must assign a DataFrame to `result`.
    Python { code: String, timeout_ms: u64 },
    /// Custom R. Receives the first input as the data frame `df` and every
    /// input in the named list `dfs`, and must assign a data frame to `result`.
    R { code: String, timeout_ms: u64 },
    /// Visual: chart specification over the upstream result. With a
    /// `summary`, one value per `x` (and `color`) is computed over all rows;
    /// `y` is unused by histograms and row counts, `x` optional for box plots.
    Chart { mark: Mark, x: String, y: String, title: Option<String>, color: Option<String>, summary: Option<AggFunc> },
}

impl StepKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            StepKind::Filter { .. } => "filter",
            StepKind::Aggregate { .. } => "aggregate",
            StepKind::Join { .. } => "join",
            StepKind::Append { .. } => "append",
            StepKind::Derive { .. } => "derive",
            StepKind::Select { .. } => "select",
            StepKind::Sort { .. } => "sort",
            StepKind::Window { .. } => "window",
            StepKind::Dedupe { .. } => "dedupe",
            StepKind::Pivot { .. } => "pivot",
            StepKind::Unpivot { .. } => "unpivot",
            StepKind::Check { .. } => "check",
            StepKind::Test { .. } => "test",
            StepKind::Clean { .. } => "clean",
            StepKind::Sql { .. } => "sql",
            StepKind::Python { .. } => "python",
            StepKind::R { .. } => "r",
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
                | StepKind::Derive { .. }
                | StepKind::Select { .. }
                | StepKind::Sort { .. }
                | StepKind::Window { .. }
                | StepKind::Dedupe { .. }
                | StepKind::Pivot { .. }
                | StepKind::Unpivot { .. }
                | StepKind::Check { .. }
                | StepKind::Test { .. }
                | StepKind::Clean { .. }
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
            StepKind::Sql { .. } | StepKind::Python { .. } | StepKind::R { .. } => (1, None),
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

#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    /// A new column, or an existing one to replace.
    pub name: String,
    /// Formula text, as typed (see `formula`).
    pub formula: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub column: String,
    /// New name, if renamed.
    pub rename: Option<String>,
}

/// One column a Clean step looks at.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanColumn {
    pub column: String,
    /// Read a text column as numbers (`Float`), whole numbers (`Int`) or
    /// dates first; values that don't convert count as missing.
    pub as_type: Option<crate::table::DataType>,
    /// A formula over the input row: where it is true, the value counts as
    /// missing too (`[age] < 0`).
    pub missing_when: Option<String>,
    pub fill: Fill,
    /// Fill within groups of these columns (the mean per region). Random
    /// draws and previous or next values also stay within a group.
    pub group_by: Vec<String>,
}

impl CleanColumn {
    pub fn new(column: impl Into<String>, fill: Fill) -> Self {
        CleanColumn { column: column.into(), as_type: None, missing_when: None, fill, group_by: vec![] }
    }
}

/// How a Clean step fills a column's missing values. Each is computed from
/// the values found (after markers, conditions and conversion), within the
/// column's groups.
#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    /// Only find them: missing values stay missing.
    Keep,
    /// The average of the known values (rounded for whole numbers).
    Mean,
    /// The middle known value (halfway between the two middle ones).
    Median,
    /// The most common known value; ties go to the smallest.
    Mode,
    /// A fixed value, as typed (read as the column's type).
    Constant { value: String },
    /// A known value drawn at random (repeatably, from the step's seed), so
    /// the column keeps its spread of values.
    Random,
    /// The last known value before it, in this order (then input order).
    Previous { order_by: Vec<SortKey> },
    /// The next known value after it, in this order (then input order).
    Next { order_by: Vec<SortKey> },
    /// A straight line fitted to the rows where it and every predictor are
    /// known (least squares, with an intercept).
    Regression { predictors: Vec<String> },
    /// The `k` most similar rows by the predictors (standardized distance):
    /// their average, or their most common value for text.
    Neighbours { predictors: Vec<String>, k: u32 },
    /// Drop rows where the value is missing.
    DropRows,
    /// What a regression resolves to when the step runs (never saved).
    Linear { intercept: f64, terms: Vec<(String, f64)> },
    /// What nearest neighbours resolve to: row number (from 1, in input
    /// order) → value (never saved).
    Rows { values: Vec<(u64, Value)> },
}

impl Fill {
    pub const NAMES: [&'static str; 11] =
        ["keep", "mean", "median", "mode", "constant", "random", "previous", "next", "regression", "neighbours", "drop"];
    pub fn name(&self) -> &'static str {
        match self {
            Fill::Keep => "keep",
            Fill::Mean => "mean",
            Fill::Median => "median",
            Fill::Mode => "mode",
            Fill::Constant { .. } => "constant",
            Fill::Random => "random",
            Fill::Previous { .. } => "previous",
            Fill::Next { .. } => "next",
            Fill::Regression { .. } | Fill::Linear { .. } => "regression",
            Fill::Neighbours { .. } | Fill::Rows { .. } => "neighbours",
            Fill::DropRows => "drop",
        }
    }
    /// Whether the fill is fitted to the data before the step runs.
    pub fn is_fitted(&self) -> bool {
        matches!(self, Fill::Regression { .. } | Fill::Neighbours { .. })
    }
}

/// The statistical tests: what `a` and `b` of a test step are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestKind {
    /// Welch's t-test: the average of number column `a` in the two groups of `b`.
    CompareMeans,
    /// One-way analysis of variance: the average of `a` across the groups of `b`.
    CompareGroups,
    /// Chi-square test of independence between categories `a` and `b`.
    Independence,
    /// Pearson correlation between number columns `a` and `b`.
    Correlation,
}

impl TestKind {
    pub const ALL: [TestKind; 4] = [TestKind::CompareMeans, TestKind::CompareGroups, TestKind::Independence, TestKind::Correlation];
    pub fn name(self) -> &'static str {
        match self {
            TestKind::CompareMeans => "compare_means",
            TestKind::CompareGroups => "compare_groups",
            TestKind::Independence => "independence",
            TestKind::Correlation => "correlation",
        }
    }
    pub fn parse(s: &str) -> Option<TestKind> {
        TestKind::ALL.into_iter().find(|t| t.name() == s)
    }
}

/// A data quality rule.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    /// The formula is true for every row (missing counts as not true).
    Expect { formula: String },
    /// No two rows share these columns' values.
    Unique { columns: Vec<String> },
    /// The number of rows is within the bounds.
    RowCount { min: Option<u64>, max: Option<u64> },
}

impl Rule {
    pub fn formula(&self) -> Option<&str> {
        match self {
            Rule::Expect { formula } => Some(formula),
            _ => None,
        }
    }
    pub fn formula_mut(&mut self) -> Option<&mut String> {
        match self {
            Rule::Expect { formula } => Some(formula),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowColumn {
    pub name: String,
    pub func: WindowFunc,
    /// The column it works on (not used by row number and rank).
    pub column: Option<String>,
    /// Rows back or ahead (previous, next) or window size (moving average).
    pub n: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowFunc {
    RunningSum,
    RowNumber,
    /// Ties in the order share a rank; the next rank skips (1, 1, 3).
    Rank,
    Previous,
    Next,
    MovingAverage,
    /// This value minus the previous one.
    Change,
    /// Change divided by the previous value; missing when that is zero.
    PercentChange,
    /// This value divided by the group's total.
    ShareOfTotal,
}

impl WindowFunc {
    pub const ALL: [WindowFunc; 9] = [
        WindowFunc::RunningSum,
        WindowFunc::RowNumber,
        WindowFunc::Rank,
        WindowFunc::Previous,
        WindowFunc::Next,
        WindowFunc::MovingAverage,
        WindowFunc::Change,
        WindowFunc::PercentChange,
        WindowFunc::ShareOfTotal,
    ];
    pub fn name(self) -> &'static str {
        match self {
            WindowFunc::RunningSum => "running_sum",
            WindowFunc::RowNumber => "row_number",
            WindowFunc::Rank => "rank",
            WindowFunc::Previous => "previous",
            WindowFunc::Next => "next",
            WindowFunc::MovingAverage => "moving_average",
            WindowFunc::Change => "change",
            WindowFunc::PercentChange => "percent_change",
            WindowFunc::ShareOfTotal => "share_of_total",
        }
    }
    pub fn parse(s: &str) -> Option<WindowFunc> {
        WindowFunc::ALL.into_iter().find(|f| f.name() == s)
    }
    pub fn needs_column(self) -> bool {
        !matches!(self, WindowFunc::RowNumber | WindowFunc::Rank)
    }
    pub fn needs_number(self) -> bool {
        matches!(
            self,
            WindowFunc::RunningSum | WindowFunc::MovingAverage | WindowFunc::Change | WindowFunc::PercentChange | WindowFunc::ShareOfTotal
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    pub column: String,
    pub descending: bool,
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
    /// Compare with this parameter's value instead (set when steps are bound).
    pub param: Option<String>,
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
    /// The middle value (linear interpolation between the two middle values).
    Median,
    /// The p-th percentile, 1 to 99, interpolated like `Median`.
    Percentile(u8),
    /// Number of different values, missing values not counted.
    CountDistinct,
    /// Sample standard deviation; missing for fewer than two values.
    Stddev,
}

impl AggFunc {
    /// Name in `analysis.json` and messages: `COUNT`, `MEDIAN`, `P90`, ...
    pub fn token(self) -> String {
        match self {
            AggFunc::Count => "COUNT".into(),
            AggFunc::Sum => "SUM".into(),
            AggFunc::Avg => "AVG".into(),
            AggFunc::Min => "MIN".into(),
            AggFunc::Max => "MAX".into(),
            AggFunc::Median => "MEDIAN".into(),
            AggFunc::Percentile(p) => format!("P{p}"),
            AggFunc::CountDistinct => "COUNT_DISTINCT".into(),
            AggFunc::Stddev => "STDDEV".into(),
        }
    }

    pub fn parse(s: &str) -> Option<AggFunc> {
        Some(match s {
            "COUNT" => AggFunc::Count,
            "SUM" => AggFunc::Sum,
            "AVG" => AggFunc::Avg,
            "MIN" => AggFunc::Min,
            "MAX" => AggFunc::Max,
            "MEDIAN" => AggFunc::Median,
            "COUNT_DISTINCT" => AggFunc::CountDistinct,
            "STDDEV" => AggFunc::Stddev,
            p => match p.strip_prefix('P').and_then(|n| n.parse::<u8>().ok()) {
                Some(n) if (1..=99).contains(&n) => AggFunc::Percentile(n),
                _ => return None,
            },
        })
    }

    /// The SQL aggregate over `arg` (a quoted column, or `*` for COUNT).
    pub fn sql_over(self, arg: &str) -> String {
        match self {
            AggFunc::Median => format!("CAST(quantile_cont(CAST({arg} AS DOUBLE), 0.5) AS DOUBLE)"),
            AggFunc::Percentile(p) => format!("CAST(quantile_cont(CAST({arg} AS DOUBLE), {}) AS DOUBLE)", p as f64 / 100.0),
            AggFunc::CountDistinct => format!("COUNT(DISTINCT {arg})"),
            AggFunc::Stddev => format!("stddev_samp(CAST({arg} AS DOUBLE))"),
            f => format!("{}({arg})", f.token()),
        }
    }

    /// Needs a numeric column, and gives a decimal.
    pub fn is_statistical(self) -> bool {
        matches!(self, AggFunc::Median | AggFunc::Percentile(_) | AggFunc::Stddev)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Bar,
    Line,
    Point,
    Area,
    /// Counts of `x` in round-width bins.
    Histogram,
    /// The spread of `y` (min, quartiles, max), per value of `x` if given.
    Boxplot,
}

impl Mark {
    pub const ALL: [Mark; 6] = [Mark::Bar, Mark::Line, Mark::Point, Mark::Area, Mark::Histogram, Mark::Boxplot];
    pub fn name(self) -> &'static str {
        match self {
            Mark::Bar => "bar",
            Mark::Line => "line",
            Mark::Point => "point",
            Mark::Area => "area",
            Mark::Histogram => "histogram",
            Mark::Boxplot => "boxplot",
        }
    }
    pub fn parse(s: &str) -> Option<Mark> {
        Mark::ALL.into_iter().find(|m| m.name() == s)
    }
}

/// An aggregate in words, for titles ("sum", "90th percentile").
pub fn agg_words(f: AggFunc) -> String {
    match f {
        AggFunc::Count => "count".into(),
        AggFunc::Sum => "sum".into(),
        AggFunc::Avg => "average".into(),
        AggFunc::Min => "minimum".into(),
        AggFunc::Max => "maximum".into(),
        AggFunc::Median => "median".into(),
        AggFunc::CountDistinct => "distinct count".into(),
        AggFunc::Stddev => "standard deviation".into(),
        AggFunc::Percentile(p) => {
            let suffix = match (p % 10, p % 100) {
                (1, x) if x != 11 => "st",
                (2, x) if x != 12 => "nd",
                (3, x) if x != 13 => "rd",
                _ => "th",
            };
            format!("{p}{suffix} percentile")
        }
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
    /// Every step with parameter references replaced by their values: what
    /// is planned, keyed and run (see `params`).
    pub fn bound(&self) -> Analysis {
        if self.parameters.is_empty() {
            return self.clone();
        }
        Analysis {
            steps: self.steps.iter().map(|s| crate::params::bind_step(s, &self.parameters)).collect(),
            ..self.clone()
        }
    }

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
