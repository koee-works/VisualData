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
  | { type: "sql"; sql: string }
  | { type: "python"; code: string; timeout_ms: number }
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

export type StepMode = "visual" | "sql" | "python" | "chart";
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
}

/** Code the user is asked to read before it runs. */
export interface ReviewItem {
  step_id: string;
  label: string | null;
  inputs: string[];
  code: string;
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
  /** For spreadsheets, the sheet that is read. */
  sheet: string | null;
}

export type SourceFormat = "csv" | "parquet" | "json" | "excel";

/** Id of an analysis's first source. */
export const SOURCE_ID = "source";
