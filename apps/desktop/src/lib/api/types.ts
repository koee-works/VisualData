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
export type AggFunc = "COUNT" | "SUM" | "AVG" | "MIN" | "MAX";
export type Mark = "bar" | "line" | "point";

export interface Predicate {
  column: string;
  op: CmpOp;
  value: Literal;
}

export interface Measure {
  func: AggFunc;
  column: string | null;
  alias: string;
}

export type StepOp =
  | { type: "filter"; predicates: Predicate[] }
  | { type: "aggregate"; group_by: string[]; measures: Measure[] }
  | { type: "sql"; sql: string }
  | { type: "python"; code: string; timeout_ms: number }
  | { type: "chart"; mark: Mark; x: string; y: string; title: string | null };

export type StepMode = "visual" | "sql" | "python" | "chart";
export type StepStateLabel = "fresh" | "stale" | "not run" | "failed" | "cancelled" | "blocked" | "needs review" | "waiting on review";

export interface StepView {
  id: string;
  input: string;
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
  source: { path: string; rows: number; columns: Column[] };
  steps: StepView[];
  cells: NotebookCell[];
  python: string | null;
  /** Python steps waiting for the user's approval, in run order (ADR-0004). */
  review: ReviewItem[];
  /** Absolute path of a source kept outside the project folder, if any. */
  external_source: string | null;
}

/** Code the user is asked to read before it runs. */
export interface ReviewItem {
  step_id: string;
  label: string | null;
  input: string;
  code: string;
}

export interface StepEvent {
  step_id: string;
  status: "succeeded" | "failed" | "cancelled" | "skipped" | "needs review";
  cached: boolean;
  detail: string | null;
  rows: number | null;
  duration_ms: number;
}

export interface RunSummary {
  run_id: string;
  succeeded: boolean;
  steps: StepEvent[];
}

export const SOURCE_ID = "source";
