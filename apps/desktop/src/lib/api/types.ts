// Mirrors crates/vda-app/src/lib.rs. Steps use the analysis.json format.

export type Cell =
  | { t: "null" }
  | { t: "bool"; v: boolean }
  | { t: "int"; v: string }
  | { t: "float"; v: number }
  | { t: "text"; v: string }
  | { t: "date"; v: string }
  | { t: "timestamp"; v: string };

export type ColumnType = "BOOLEAN" | "BIGINT" | "DOUBLE" | "VARCHAR" | "DATE" | "TIMESTAMP";

export interface Column {
  name: string;
  type: ColumnType;
}

export interface Page {
  offset: number;
  total_rows: number;
  columns: Column[];
  rows: Cell[][];
  stale: boolean;
}

export interface ProfileRow {
  name: string;
  type: ColumnType;
  rows: number;
  missing: number;
  distinct: number;
  min: Cell;
  max: Cell;
}

/** A literal in a filter predicate, as stored in analysis.json. */
export type Literal =
  | null
  | boolean
  | string
  | { int: string }
  | { float: number }
  | { date: string }
  | { timestamp: string };

export type CmpOp = "=" | "<>" | "<" | "<=" | ">" | ">=" | "IS NULL" | "IS NOT NULL";
export type JoinHow = "inner" | "left" | "full";

export interface JoinKey {
  left: string;
  /** Comparison between the two columns; `=` when left out. */
  op?: CmpOp;
  right: string;
}

export type AggFunc =
  | "COUNT"
  | "SUM"
  | "AVG"
  | "MIN"
  | "MAX"
  | "MEDIAN"
  | "COUNT_DISTINCT"
  | "STDDEV"
  /** The p-th percentile, P1 to P99. */
  | `P${number}`;
export type Mark = "bar" | "line" | "point" | "area" | "histogram" | "boxplot";

export interface Predicate {
  column: string;
  op: CmpOp;
  value: Literal;
  /** Compare with this parameter's value instead of `value`. */
  param?: string | null;
}

export interface Measure {
  func: AggFunc;
  column: string | null;
  alias: string;
}

export interface SortKey {
  column: string;
  descending: boolean;
}

export type WindowFunc =
  | "running_sum"
  | "row_number"
  | "rank"
  | "previous"
  | "next"
  | "moving_average"
  | "change"
  | "percent_change"
  | "share_of_total";

export interface WindowColumn {
  name: string;
  func: WindowFunc;
  /** Not used by row number and rank. */
  column: string | null;
  /** Rows back or ahead (previous, next), or the window (moving average). */
  n: number | null;
}

/** The language of the code cells in a Jupyter notebook export. */
export type NotebookLanguage = "python" | "sql" | "r";

export type StepOp =
  | { type: "filter"; predicates: Predicate[] }
  | { type: "aggregate"; group_by: string[]; measures: Measure[] }
  | { type: "join"; how: JoinHow; on: JoinKey[] }
  | { type: "append"; distinct: boolean; origin: string | null }
  | { type: "derive"; columns: { name: string; formula: string }[] }
  | { type: "select"; columns: { column: string; as: string | null }[] }
  | { type: "sort"; by: { column: string; descending: boolean }[]; limit: number | null }
  | { type: "window"; partition_by: string[]; order_by: SortKey[]; columns: WindowColumn[] }
  | { type: "dedupe"; keys: string[]; order_by: SortKey[] }
  | { type: "pivot"; rows: string[]; column: string; value: string | null; func: AggFunc }
  | { type: "unpivot"; columns: string[]; name: string; value: string }
  | { type: "check"; stop: boolean; rules: Rule[] }
  | { type: "test"; test: TestKind; a: string; b: string }
  | { type: "clean"; markers: string[]; columns: CleanColumn[]; flag: boolean; seed: number }
  | { type: "sql"; sql: string }
  | { type: "python"; code: string; timeout_ms: number }
  | { type: "r"; code: string; timeout_ms: number }
  | {
      type: "chart";
      mark: Mark;
      x: string;
      y: string;
      title: string | null;
      /** One color per value of this column (bars stack). */
      color?: string | null;
      /** One value per x (and color), computed over every row. */
      summary?: AggFunc | null;
    };

/** How a Clean step fills one column's missing values (clean.rs). */
export type Fill =
  | { method: "keep" }
  | { method: "mean" }
  | { method: "median" }
  | { method: "mode" }
  | { method: "constant"; value: string }
  | { method: "random" }
  | { method: "previous"; order_by: SortKey[] }
  | { method: "next"; order_by: SortKey[] }
  | { method: "regression"; predictors: string[] }
  | { method: "neighbours"; predictors: string[]; k: number }
  | { method: "drop" };
export type FillMethod = Fill["method"];

/** One column a Clean step looks at. */
export interface CleanColumn {
  column: string;
  /** Read text as numbers, whole numbers or dates first. */
  as_type: "DOUBLE" | "BIGINT" | "DATE" | null;
  /** Where this formula is true the value counts as missing too. */
  missing_when: string | null;
  /** Fill within groups of these columns. */
  group_by: string[];
  fill: Fill;
}

export interface CleanColumnReport {
  column: string;
  empty: number;
  markers: number;
  condition: number;
  conversion: number;
  filled: number;
  left: number;
  method: FillMethod;
  detail: string;
}

export interface CleanReport {
  kind: "clean";
  rows: number;
  dropped: number;
  columns: CleanColumnReport[];
}

/** How one fill method did on hidden known values. */
export interface MethodScore {
  fill: Fill;
  label: string;
  filled: number;
  /** Numbers: average and root-mean-square distance from the truth. */
  mae: number | null;
  rmse: number | null;
  /** Other types: share filled exactly right. */
  accuracy: number | null;
  /** Share of the true values' spread the fills keep (1 = all of it). */
  spread: number | null;
  problem: string | null;
}

export interface Comparison {
  column: string;
  rows: number;
  known: number;
  hidden: number;
  predictors: string[];
  methods: MethodScore[];
}

/** compare_means: Welch's t-test of `a` between the two groups of `b`;
 * compare_groups: one-way ANOVA; independence: chi-square of categories
 * `a` and `b`; correlation: Pearson's r of numbers `a` and `b`. */
export type TestKind = "compare_means" | "compare_groups" | "independence" | "correlation";

export type Rule =
  | { rule: "expect"; formula: string }
  | { rule: "unique"; columns: string[] }
  | { rule: "rows"; min: number | null; max: number | null };

export interface RuleResult {
  rule: number;
  passed: boolean;
  failing: number;
  message: string;
  columns: string[];
  examples: string[][];
}

export interface CheckReport {
  rows: number;
  results: RuleResult[];
}

export interface Parameter {
  name: string;
  value: Literal;
}

/** A column's basic profile with its details (numbers null where they do not apply). */
export interface ColumnDetail {
  name: string;
  type: ColumnType;
  rows: number;
  missing: number;
  distinct: number;
  min: string | null;
  max: string | null;
  mean: number | null;
  stddev: number | null;
  q1: number | null;
  median: number | null;
  q3: number | null;
  zeros: number | null;
  negatives: number | null;
  blank: number | null;
  min_length: number | null;
  max_length: number | null;
  top: { value: string; count: number }[];
  /** Ten equal-width bars from min to max (numbers only). */
  histogram: number[];
}

export type StepMode = "visual" | "sql" | "python" | "r" | "chart";
export type StepStateLabel = "fresh" | "stale" | "not run" | "failed" | "cancelled" | "blocked" | "needs review" | "waiting on review";

export interface StepView {
  id: string;
  /** Steps or sources this step reads; the first is its main input. */
  inputs: string[];
  label: string | null;
  op: StepOp;
  mode: StepMode;
  state: StepStateLabel;
  detail: string | null;
  rows: number | null;
  columns: Column[] | null;
  sql: string | null;
  dependents: string[];
}

export type NotebookCell = { type: "markdown"; text: string } | { type: "step"; step: string };

export interface AnalysisView {
  project_dir: string;
  name: string;
  /** Data files, in the order they were added. */
  sources: SourceView[];
  steps: StepView[];
  cells: NotebookCell[];
  python: string | null;
  /** Python steps waiting for the user's approval, in run order (ADR-0004). */
  review: ReviewItem[];
  /** Python runs without review until the app quits (never saved). */
  trust_all: boolean;
  parameters: Parameter[];
  /** What undo and redo would do, if anything. */
  undo: string | null;
  redo: string | null;
}

/** Code the user is asked to read before it runs. */
export interface ReviewItem {
  step_id: string;
  label: string | null;
  inputs: string[];
  code: string;
  language?: "python" | "r";
}

export interface StepEvent {
  step_id: string;
  status: "succeeded" | "failed" | "cancelled" | "skipped" | "needs review";
  cached: boolean;
  detail: string | null;
  rows: number | null;
  duration_ms: number;
  /** What the step printed (Python), if anything. */
  output?: string | null;
}

export type Severity = "error" | "warning" | "info";

/** A linter finding. */
export interface Problem {
  severity: Severity;
  code: string;
  step: string | null;
  /** Part of the step: `columns.1.formula`, `sql`, `code`, ... */
  field: string | null;
  line: number | null;
  column: number | null;
  message: string;
  from_last_run: boolean;
  dismissable: boolean;
}

export interface Problems {
  items: Problem[];
  dismissed: number;
}

export interface RunView {
  run_id: string;
  started_unix_ms: number;
  succeeded: boolean;
  steps: StepEvent[];
  /** "scheduled" for runs by vda-run. */
  trigger: string | null;
}

export interface RunSummary {
  run_id: string;
  succeeded: boolean;
  steps: StepEvent[];
}

export interface SourceView {
  /** How steps and custom SQL refer to this source. */
  id: string;
  path: string;
  rows: number;
  columns: Column[];
  /** Kept outside the project folder. */
  external: boolean;
  format: SourceFormat;
  /** For spreadsheets, the sheet that is read; for databases, the table. */
  sheet: string | null;
  /** For databases, the query read instead of a table. */
  query: string | null;
}

export type SourceFormat = "csv" | "parquet" | "json" | "excel" | "sqlite" | "duckdb";

/** A saved version of the analysis (newest first in lists). */
export interface VersionView {
  version: number;
  time_unix_ms: number;
  what: string;
  current: boolean;
}

export interface Comment {
  id: string;
  /** `step:<id>`, `source:<id>` or `analysis`. */
  target: string;
  reply_to: string | null;
  author: string;
  time_unix_ms: number;
  text: string;
  resolved: boolean;
}

export interface PythonPackage {
  name: string;
  purpose: string;
  required: boolean;
  version: string | null;
}

/** Where the app keeps what is yours, and what it holds. */
/** One kind of process and what all of that kind use together. */
export interface ResourcePart {
  name: "App" | "Python" | "R" | "Web view" | "Other";
  count: number;
  /** Percent of the whole computer's processing power. */
  cpu: number;
  memory_bytes: number;
}

export interface ResourceView {
  /** Percent of the whole computer's processing power, all parts together. */
  cpu: number;
  memory_bytes: number;
  parts: ResourcePart[];
  cores: number;
  total_memory_bytes: number;
}

export interface AboutView {
  version: string;
  settings_path: string | null;
  approvals_path: string | null;
  /** Pieces of Python and R code approved to run. */
  approvals: number;
}

export interface PythonView {
  interpreter: string;
  executable: string | null;
  version: string | null;
  packages: PythonPackage[];
  problem: string | null;
  /** "env" (VDA_PYTHON), "chosen" in the app, or "default". */
  chosen_by: "env" | "chosen" | "default";
  candidates: string[];
}

export interface RView {
  rscript: string;
  version: string | null;
  /** Package libraries R steps see (where RStudio installs packages). */
  libraries: string[];
  packages: PythonPackage[];
  problem: string | null;
  /** "env" (VDA_R), "chosen" in the app, or "default". */
  chosen_by: "env" | "chosen" | "default";
  candidates: string[];
}

/** An object held in a step's console, for its Environment pane. */
export interface ConsoleObject {
  name: string;
  kind: string;
  summary: string;
}

export interface ConsoleView {
  step_id: string;
  language: "python" | "r";
  version: string;
  objects: ConsoleObject[];
  /** Inputs whose results are from before a change upstream. */
  outdated: string[];
}

export interface ConsoleReply {
  ok: boolean;
  stdout: string;
  stderr: string;
  /** The last value, when it is a table: [name, type] columns and the first rows. */
  table: { columns: [string, string][]; rows: unknown[][]; total: number } | null;
  text: string | null;
  error: string | null;
  /** Line of the error in the code that was run. */
  line: number | null;
  /** Plots as data: URLs. */
  images: string[];
  objects: ConsoleObject[];
  elapsed_ms: number;
}

export interface ExternalView {
  step_id: string;
  path: string;
  kind: "notebook" | "R script";
}

export interface SyncView {
  updated: string[];
  view: AnalysisView | null;
}

export type Every = "hourly" | "daily" | "weekdays" | "weekly";

export interface ScheduleView {
  command: string;
  entry: string;
  how: string;
  scheduler: "cron" | "launchd" | "schtasks";
}

/** Id of an analysis's first source. */
export const SOURCE_ID = "source";
