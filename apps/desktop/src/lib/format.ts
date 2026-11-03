import type { Cell, ColumnType, Literal, StepOp, StepView } from "./api/types";

/** A cell's full value, for tooltips. */
export function exactCell(c: Cell): string {
  return c.t === "float" ? String(c.v) : formatCell(c);
}

/** Display a cell the way the core does (whole floats keep one decimal). */
export function formatCell(c: Cell): string {
  switch (c.t) {
    case "null":
      return "";
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

/** One-line description of what a step does, for the pipeline. */
export function describe(op: StepOp): string {
  switch (op.type) {
    case "filter":
      if (!op.predicates.length) return "Keep all rows";
      return (
        "Keep rows where " +
        op.predicates
          .map((p) => `${p.column} ${opWords[p.op]}${p.op.startsWith("IS") ? "" : ` ${literalText(p.value)}`}`)
          .join(" and ")
      );
    case "aggregate": {
      const ms = op.measures.map((m) => (m.func === "COUNT" && !m.column ? "count" : `${m.func.toLowerCase()} of ${m.column}`));
      return `${ms.join(", ") || "Groups"}${op.group_by.length ? ` by ${op.group_by.join(", ")}` : ""}`;
    }
    case "sql":
      return "Custom SQL";
    case "python":
      return "Custom Python";
    case "chart":
      return `${op.mark[0]!.toUpperCase()}${op.mark.slice(1)} chart of ${op.y} by ${op.x}`;
  }
}

export function stepTitle(s: StepView): string {
  return s.label?.trim() || describe(s.op);
}

export function formatCount(n: number): string {
  return n.toLocaleString("en-US");
}
