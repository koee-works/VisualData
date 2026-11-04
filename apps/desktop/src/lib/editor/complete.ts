// What the code editor offers as you type. Pure functions of the text before
// the cursor and what the step can read, so they are tested without an editor.

import functions from "./formula-functions.json";
import { typeLabel } from "../format";

export type Lang = "sql" | "python" | "r" | "formula";

export interface EditorInput {
  id: string;
  name: string;
  columns: { name: string; type: string }[] | null;
}

export interface EditorContext {
  /** The step's inputs; the first is `df` (Python, R) or `input` (SQL). */
  inputs: EditorInput[];
  parameters: { name: string; kind?: string }[];
}

export interface Suggestion {
  label: string;
  type: "column" | "table" | "parameter" | "function" | "variable" | "method" | "keyword";
  detail?: string;
  /** Text inserted, when it differs from the label. */
  apply?: string;
  boost?: number;
}

export interface Completion {
  /** Offset in `before` where the completed word starts. */
  from: number;
  options: Suggestion[];
}

const IDENT = /^[A-Za-z_][A-Za-z0-9_]*$/;

const PANDAS = [
  "head", "tail", "describe", "info", "groupby", "agg", "assign", "query", "loc", "iloc", "merge", "join",
  "sort_values", "drop", "dropna", "fillna", "rename", "pivot_table", "melt", "value_counts", "unique",
  "nunique", "astype", "apply", "copy", "reset_index", "set_index", "isna", "sum", "mean", "median",
  "min", "max", "count", "std", "shape", "columns", "dtypes",
];

const R_FUNCTIONS = [
  "filter", "mutate", "summarise", "group_by", "ungroup", "arrange", "desc", "select", "rename", "distinct",
  "left_join", "inner_join", "full_join", "bind_rows", "n", "n_distinct", "case_when", "if_else",
  "pivot_longer", "pivot_wider", "head", "nrow", "ncol", "names", "subset", "transform", "aggregate",
  "merge", "order", "is.na", "ifelse", "mean", "median", "sum", "sd", "round", "paste0", "as.Date",
  "as.numeric", "as.character", "lm", "summary", "table", "data.frame",
];

const first = (ctx: EditorContext) => ctx.inputs[0];

function columns(input: EditorInput | undefined, detail = true): Suggestion[] {
  return (input?.columns ?? []).map((c) => ({
    label: c.name,
    type: "column",
    detail: detail ? ((typeLabel as Record<string, string>)[c.type] ?? c.type) : undefined,
    boost: 2,
  }));
}

function byId(ctx: EditorContext, id: string) {
  return ctx.inputs.find((i) => i.id === id);
}

function params(ctx: EditorContext): Suggestion[] {
  return ctx.parameters.map((p) => ({ label: p.name, type: "parameter", detail: p.kind, boost: 1 }));
}

function inputIds(ctx: EditorContext): Suggestion[] {
  return ctx.inputs.map((i) => ({ label: i.id, type: "table", detail: i.name }));
}

/** The word being typed at the end of `before`, and where it starts. */
function word(before: string, pattern = /[A-Za-z_][A-Za-z0-9_.]*$/) {
  const m = before.match(pattern);
  return m ? { from: before.length - m[0].length, text: m[0] } : { from: before.length, text: "" };
}

function filter(c: Completion | null): Completion | null {
  return c && c.options.length ? c : null;
}

function python(ctx: EditorContext, before: string): Completion | null {
  let m: RegExpMatchArray | null;
  // dfs["orders"]["re|
  if ((m = before.match(/\bdfs\[\s*["']([^"']+)["']\s*\]\[\s*["']([^"']*)$/))) {
    return { from: before.length - m[2]!.length, options: columns(byId(ctx, m[1]!)) };
  }
  // df["re|   dfs["or|   params["go|
  if ((m = before.match(/\b(df|dfs|params)\[\s*["']([^"']*)$/))) {
    const options = m[1] === "df" ? columns(first(ctx)) : m[1] === "dfs" ? inputIds(ctx) : params(ctx);
    return { from: before.length - m[2]!.length, options };
  }
  // df.re|
  if ((m = before.match(/\bdf\.([A-Za-z_]\w*)?$/))) {
    const typed = m[1] ?? "";
    const cols = columns(first(ctx)).filter((c) => IDENT.test(c.label));
    return { from: before.length - typed.length, options: [...cols, ...PANDAS.map((p) => ({ label: p, type: "method" as const }))] };
  }
  const w = word(before, /[A-Za-z_]\w*$/);
  if (!w.text || /[.\w]$/.test(before.slice(0, w.from))) return null;
  return {
    from: w.from,
    options: [
      { label: "df", type: "variable", detail: first(ctx)?.name },
      ...(ctx.inputs.length > 1 ? [{ label: "dfs", type: "variable" as const, detail: "every input" }] : []),
      { label: "params", type: "variable", detail: "parameters" },
      { label: "result", type: "variable", detail: "this step's result" },
      { label: "pd", type: "variable", detail: "pandas" },
    ],
  };
}

function r(ctx: EditorContext, before: string): Completion | null {
  let m: RegExpMatchArray | null;
  // dfs$orders$re|   dfs[["orders"]]$re|
  if ((m = before.match(/\bdfs(?:\$([A-Za-z_.][\w.]*)|\[\[\s*["']([^"']+)["']\s*\]\])\$([\w.]*)$/))) {
    return { from: before.length - m[3]!.length, options: columns(byId(ctx, (m[1] ?? m[2])!)) };
  }
  // df$re|   dfs$or|   params$go|
  if ((m = before.match(/\b(df|dfs|params)\$([\w.]*)$/))) {
    const options = m[1] === "df" ? columns(first(ctx)) : m[1] === "dfs" ? inputIds(ctx) : params(ctx);
    return { from: before.length - m[2]!.length, options };
  }
  // df[["re|
  if ((m = before.match(/\b(df|dfs|params)\[\[\s*["']([^"']*)$/))) {
    const options = m[1] === "df" ? columns(first(ctx)) : m[1] === "dfs" ? inputIds(ctx) : params(ctx);
    return { from: before.length - m[2]!.length, options };
  }
  const w = word(before);
  if (!w.text || /[$@\w]$/.test(before.slice(0, w.from))) return null;
  // Inside a dplyr verb, bare column names are what people type.
  const cols = columns(first(ctx)).filter((c) => /^[A-Za-z.][\w.]*$/.test(c.label));
  return {
    from: w.from,
    options: [
      { label: "df", type: "variable", detail: first(ctx)?.name, boost: 3 },
      ...(ctx.inputs.length > 1 ? [{ label: "dfs", type: "variable" as const, detail: "every input" }] : []),
      { label: "params", type: "variable", detail: "parameters" },
      { label: "result", type: "variable", detail: "this step's result" },
      ...cols,
      ...R_FUNCTIONS.map((f) => ({ label: f, type: "function" as const, apply: f + "(" })),
    ],
  };
}

function sql(ctx: EditorContext, before: string): Completion | null {
  // Column and table names come from the SQL language's own schema
  // completion (see schemaFor); parameters are `$name`.
  const m = before.match(/\$(\w*)$/);
  if (!m) return null;
  return { from: before.length - m[1]!.length, options: params(ctx) };
}

function formula(ctx: EditorContext, before: string): Completion | null {
  let m: RegExpMatchArray | null;
  if ((m = before.match(/@(\w*)$/))) return { from: before.length - m[1]!.length, options: params(ctx) };
  // [Unit price|
  if ((m = before.match(/\[([^\]]*)$/))) return { from: before.length - m[1]!.length, options: columns(first(ctx)) };
  const w = word(before, /[A-Za-z_]\w*$/);
  if (!w.text || /[@\w]$/.test(before.slice(0, w.from))) return null;
  return {
    from: w.from,
    options: [
      ...columns(first(ctx)).filter((c) => IDENT.test(c.label)),
      ...functions.map((f) => ({ label: f.name, type: "function" as const, apply: f.name + "(" })),
      ...["and", "or", "not", "is missing", "is not missing", "true", "false"].map((k) => ({ label: k, type: "keyword" as const })),
    ],
  };
}

export function suggest(lang: Lang, ctx: EditorContext, before: string): Completion | null {
  const f = { sql, python, r, formula }[lang];
  return filter(f(ctx, before));
}

/** Tables and their columns for SQL completion: `input` and each input's id. */
export function schemaFor(ctx: EditorContext): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  ctx.inputs.forEach((i, n) => {
    const cols = (i.columns ?? []).map((c) => c.name);
    if (n === 0) out["input"] = cols;
    out[i.id] = cols;
  });
  return out;
}

/** Parameters as the editor lists them, with their kind. */
export function editorParameters(ps: { name: string; value: unknown }[]): EditorContext["parameters"] {
  return ps.map((p) => ({ name: p.name, kind: p.value && typeof p.value === "object" ? Object.keys(p.value)[0] : undefined }));
}
