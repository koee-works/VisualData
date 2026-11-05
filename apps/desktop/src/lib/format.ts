import type { Cell, ColumnType, Literal, StepOp, StepView, TestKind, WindowFunc } from "./api/types";

/** A cell's full value, for tooltips. */
export function exactCell(c: Cell): string {
  if (c.t === "date" || c.t === "timestamp") return c.v;
  return c.t === "float" ? String(c.v) : formatCell(c);
}

/** How numbers are shown (from the user's preferences). */
export const numberDisplay = { thousands: true, maxDecimals: 4 };

export type MissingText = "missing" | "dash" | "blank" | "null";
export type DateFormat = "iso" | "us" | "eu" | "long";

/** How missing cells and dates are shown (from the user's preferences). */
export const cellDisplay: { missing: MissingText; dates: DateFormat } = { missing: "missing", dates: "iso" };

/** What a missing cell shows in a table. */
export function missingLabel(): string {
  return { missing: "missing", dash: "—", blank: "", null: "null" }[cellDisplay.missing];
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** A date or date and time (as DuckDB prints them: 2026-07-02 or
 *  2026-07-02 13:45:00) in the chosen format. Anything else is left alone. */
export function formatDateText(text: string, format: DateFormat = cellDisplay.dates): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:[ T](.+))?$/.exec(text);
  if (!m || format === "iso") return text;
  const [, y, mo, d, time] = m;
  const month = Number(mo);
  const day = Number(d);
  if (month < 1 || month > 12) return text;
  const date = { us: `${month}/${day}/${y}`, eu: `${d}.${mo}.${y}`, long: `${day} ${MONTHS[month - 1]} ${y}` }[format];
  return time ? `${date} ${time}` : date;
}

/** How many decimals a column's decimal values need on screen, so they line
 *  up: as many as the most precise visible value, at least one, at most four
 *  (six for columns of small fractions). The exact value is in each cell's title. */
export function columnDecimals(cells: Cell[]): number {
  let need = 1;
  let big = 0;
  for (const c of cells) {
    if (c.t !== "float" || !Number.isFinite(c.v)) continue;
    big = Math.max(big, Math.abs(c.v));
    const text = String(Number(c.v.toFixed(6)));
    const dot = text.indexOf(".");
    if (dot >= 0 && !text.includes("e")) need = Math.max(need, text.length - dot - 1);
  }
  const cap = Math.max(numberDisplay.maxDecimals, 1);
  return Math.min(need, big > 0 && big < 1 ? Math.max(cap, 6) : cap);
}

/** A decimal with a fixed number of decimals and thousands separators. */
export function formatDecimal(v: number, decimals: number): string {
  if (!Number.isFinite(v)) return String(v);
  if (Math.abs(v) >= 1e15 || (v !== 0 && Math.abs(v) < 1e-6)) return String(v);
  return v.toLocaleString("en-US", { minimumFractionDigits: decimals, maximumFractionDigits: decimals, useGrouping: numberDisplay.thousands });
}

/** Display a cell the way the core does (whole floats keep one decimal).
 *  With `decimals`, a decimal is shown with that many, for a column. */
export function formatCell(c: Cell, decimals?: number): string {
  if (c.t === "float" && decimals !== undefined) return formatDecimal(c.v, decimals);
  switch (c.t) {
    case "null":
      return "";
    case "date":
    case "timestamp":
      return formatDateText(c.v);
    case "bool":
      return c.v ? "true" : "false";
    case "float":
      if (Number.isInteger(c.v) && Math.abs(c.v) < 1e15) return c.v.toFixed(1);
      // At most six decimals on screen; the exact value is in the cell's title.
      if (Math.abs(c.v) >= 1e-6 && Math.abs(c.v) < 1e15) return String(Number(c.v.toFixed(6)));
      return String(c.v);
    default:
      return c.v;
  }
}

export function isNumericType(t: ColumnType): boolean {
  return t === "BIGINT" || t === "DOUBLE";
}

/** Short human type names for column headers. */
export const typeLabel: Record<ColumnType, string> = {
  BOOLEAN: "true/false",
  BIGINT: "integer",
  DOUBLE: "decimal",
  VARCHAR: "text",
  DATE: "date",
  TIMESTAMP: "date & time",
};

/** Parse what the user typed into a literal of the column's type. */
export function parseLiteral(raw: string, type: ColumnType): Literal | { error: string } {
  const s = raw.trim();
  switch (type) {
    case "BIGINT":
      if (/^-?\d+$/.test(s)) return { int: s };
      if (/^-?\d*\.\d+$|^-?\d+\.\d*$/.test(s)) return { float: Number(s) };
      return { error: "Enter a whole number, like 20" };
    case "DOUBLE":
      if (s !== "" && Number.isFinite(Number(s))) return /^-?\d+$/.test(s) ? { int: s } : { float: Number(s) };
      return { error: "Enter a number, like 19.5" };
    case "BOOLEAN":
      if (s === "true" || s === "false") return s === "true";
      return { error: "Enter true or false" };
    case "DATE":
      if (/^\d{4}-\d{2}-\d{2}$/.test(s)) return { date: s };
      return { error: "Enter a date as YYYY-MM-DD" };
    case "TIMESTAMP":
      if (/^\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(\.\d{1,6})?$/.test(s)) return { timestamp: s.replace("T", " ") };
      if (/^\d{4}-\d{2}-\d{2}$/.test(s)) return { date: s };
      return { error: "Enter a date and time as YYYY-MM-DD HH:MM:SS" };
    case "VARCHAR":
      return s === "" ? { error: "Enter some text" } : raw;
  }
}

export function literalText(l: Literal): string {
  if (l === null) return "";
  if (typeof l === "boolean") return String(l);
  if (typeof l === "string") return l;
  if ("int" in l) return l.int;
  if ("float" in l) return String(l.float);
  if ("date" in l) return l.date;
  return l.timestamp;
}

const opWords: Record<string, string> = {
  "=": "is",
  "<>": "is not",
  "<": "below",
  "<=": "at most",
  ">": "above",
  ">=": "at least",
  "IS NULL": "is missing",
  "IS NOT NULL": "is present",
};

/** How an aggregate function is named in the interface. */
export function funcWords(f: string): string {
  const fixed: Record<string, string> = {
    COUNT: "Count",
    SUM: "Sum",
    AVG: "Average",
    MIN: "Minimum",
    MAX: "Maximum",
    MEDIAN: "Median",
    COUNT_DISTINCT: "Distinct count",
    STDDEV: "Standard deviation",
  };
  if (fixed[f]) return fixed[f]!;
  const p = Number(f.slice(1));
  const suffix = p % 10 === 1 && p !== 11 ? "st" : p % 10 === 2 && p !== 12 ? "nd" : p % 10 === 3 && p !== 13 ? "rd" : "th";
  return `${p}${suffix} percentile`;
}

/** Window calculations as the interface names them. */
export const windowWords: Record<WindowFunc, string> = {
  running_sum: "Running total",
  row_number: "Row number",
  rank: "Rank",
  previous: "Previous value",
  next: "Next value",
  moving_average: "Moving average",
  change: "Change from previous",
  percent_change: "% change from previous",
  share_of_total: "Share of total",
};

/** Comparison symbols as people write them. */
export const symbols: Record<string, string> = { "=": "=", "<>": "≠", "<": "<", "<=": "≤", ">": ">", ">=": "≥" };

/** The kind of step, in a word or two, for the editor's header. */
export const kindWords: Record<StepOp["type"], string> = {
  filter: "Filter",
  derive: "Calculate",
  select: "Columns",
  sort: "Sort",
  aggregate: "Summarize",
  window: "Across rows",
  dedupe: "Duplicates",
  pivot: "Pivot",
  unpivot: "Unpivot",
  join: "Join",
  append: "Append",
  check: "Check",
  test: "Test",
  clean: "Clean",
  chart: "Chart",
  sql: "SQL",
  python: "Python",
  r: "R",
};

/** One-line description of what a step does, for the pipeline. */
export function describe(op: StepOp): string {
  switch (op.type) {
    case "filter":
      if (!op.predicates.length) return "Keep all rows";
      return (
        "Keep rows where " +
        op.predicates
          .map((p) => `${p.column} ${opWords[p.op]}${p.op.startsWith("IS") ? "" : ` ${p.param ? `@${p.param}` : literalText(p.value)}`}`)
          .join(" and ")
      );
    case "aggregate": {
      const ms = op.measures.map((m) => (m.func === "COUNT" && !m.column ? "count" : `${funcWords(m.func).toLowerCase()} of ${m.column}`));
      return `${ms.join(", ") || "Groups"}${op.group_by.length ? ` by ${op.group_by.join(", ")}` : ""}`;
    }
    case "join": {
      const kind = { inner: "Join", left: "Left join", full: "Full join" }[op.how];
      const keys = op.on.map((k) =>
        (k.op ?? "=") === "=" && k.left === k.right ? k.left : `${k.left} ${symbols[k.op ?? "="]} ${k.right}`,
      );
      return keys.length ? `${kind} on ${keys.join(", ")}` : kind;
    }
    case "derive":
      return op.columns.length === 1 ? `Calculate ${op.columns[0]!.name || "a column"}` : `Calculate ${op.columns.map((c) => c.name).join(", ")}`;
    case "select":
      return `Keep ${op.columns.map((c) => c.as ?? c.column).join(", ")}`;
    case "sort": {
      const keys = op.by.map((k) => `${k.column}${k.descending ? " (highest first)" : ""}`).join(", ");
      const first = op.limit !== null ? `first ${op.limit}` : "";
      return keys ? `Sort by ${keys}${first ? `, ${first}` : ""}` : `Keep the ${first} rows`;
    }
    case "window": {
      const what = op.columns.map((c) => (c.column ? `${windowWords[c.func].toLowerCase()} of ${c.column}` : windowWords[c.func].toLowerCase()));
      const s = what.join(", ") || "Window calculations";
      return `${s[0]!.toUpperCase()}${s.slice(1)}${op.partition_by.length ? ` per ${op.partition_by.join(", ")}` : ""}`;
    }
    case "dedupe":
      return op.keys.length ? `One row per ${op.keys.join(", ")}` : "Remove repeated rows";
    case "pivot":
      return `Columns for each ${op.column || "value"}${op.rows.length ? `, rows by ${op.rows.join(", ")}` : ""}`;
    case "unpivot":
      return `Turn ${op.columns.join(", ") || "columns"} into rows`;
    case "append":
      return op.distinct ? "Append rows, without repeats" : "Append rows";
    case "sql":
      return "Custom SQL";
    case "python":
      return "Custom Python";
    case "r":
      return "Custom R";
    case "chart": {
      if (op.mark === "histogram") return `Histogram of ${op.x}`;
      if (op.mark === "boxplot") return `Box plot of ${op.y}${op.x ? ` by ${op.x}` : ""}`;
      const name = { bar: "Bar", line: "Line", point: "Point", area: "Area" }[op.mark];
      const y = !op.summary ? op.y : op.summary === "COUNT" ? "rows" : `${funcWords(op.summary).toLowerCase()} of ${op.y}`;
      return `${name} chart of ${y} by ${op.x}${op.color ? ` and ${op.color}` : ""}`;
    }
    case "check":
      return op.rules.length === 1 ? "Check 1 rule" : `Check ${op.rules.length} rules`;
    case "test":
      return testSentence(op.test, op.a, op.b);
    case "clean":
      return cleanSentence(op);
  }
}

/** A Clean step in words (the same wording as describe.rs). */
function cleanSentence(op: Extract<StepOp, { type: "clean" }>): string {
  const cols = op.columns;
  if (!cols.length) return "Clean missing values";
  if (cols.length > 1) {
    const names = cols.map((c) => c.column);
    return names.length > 3 ? `Clean ${names.slice(0, 2).join(", ")} and ${names.length - 2} more` : `Clean ${names.join(", ")}`;
  }
  const c = cols[0]!;
  const name = c.column || "a column";
  const per = c.group_by?.length ? ` per ${c.group_by.join(", ")}` : "";
  const f = c.fill;
  switch (f.method) {
    case "keep":
      return `Find missing ${name}`;
    case "drop":
      return `Drop rows with no ${name}`;
    case "mean":
      return `Fill missing ${name} with the mean${per}`;
    case "median":
      return `Fill missing ${name} with the median${per}`;
    case "mode":
      return `Fill missing ${name} with the most common value${per}`;
    case "constant":
      return `Fill missing ${name} with "${f.value}"`;
    case "random":
      return `Fill missing ${name} with a random known value${per}`;
    case "previous":
      return `Fill missing ${name} with the previous value`;
    case "next":
      return `Fill missing ${name} with the next value`;
    case "regression":
      return `Fill missing ${name} with a regression`;
    case "neighbours":
      return `Fill missing ${name} with nearest neighbours`;
  }
}

/** The statistical tests, as the interface names them. */
export const testWords: Record<TestKind, { name: string; a: string; b: string; hint: string }> = {
  compare_means: { name: "Compare two averages", a: "The average of", b: "between the two groups of", hint: "Welch's t-test" },
  compare_groups: { name: "Compare averages across groups", a: "The average of", b: "across the groups of", hint: "One-way analysis of variance" },
  independence: { name: "Are two categories related?", a: "Whether", b: "is related to", hint: "Chi-square test of independence" },
  correlation: { name: "Do two numbers move together?", a: "Whether", b: "moves with", hint: "Pearson correlation" },
};

function testSentence(t: TestKind, a: string, b: string): string {
  switch (t) {
    case "compare_means":
      return `Compare the average of ${a || "…"} between ${b || "two groups"}`;
    case "compare_groups":
      return `Compare the average of ${a || "…"} across ${b || "groups"}`;
    case "independence":
      return `Test whether ${a || "…"} and ${b || "…"} are related`;
    case "correlation":
      return `Correlation of ${a || "…"} and ${b || "…"}`;
  }
}

export function stepTitle(s: StepView): string {
  return s.label?.trim() || describe(s.op);
}

export function formatCount(n: number): string {
  return n.toLocaleString("en-US");
}
