// In-memory backend for UI tests and `npm run dev` in a plain browser.
// It evaluates filters and aggregates itself so the UI sees realistic state
// changes (not run → fresh → stale), but it is not the engine: the real
// semantics live in Rust and are tested there.

import type { Backend, RunHandlers } from "./backend";
import type {
  AnalysisView,
  Cell,
  Column,
  Literal,
  NotebookCell,
  Page,
  Problem,
  Problems,
  ProfileRow,
  RunView,
  RunSummary,
  StepEvent,
  StepOp,
  StepStateLabel,
  StepView,
} from "./types";
import { SOURCE_ID } from "./types";

type Row = Cell[];
interface Result {
  def: string;
  columns: Column[];
  rows: Row[];
}
interface MockStep {
  id: string;
  inputs: string[];
  label: string | null;
  op: StepOp;
}

const REGIONS = ["East", "North", "South", "West"];

function sourceTable(n: number): { columns: Column[]; rows: Row[] } {
  const columns: Column[] = [
    { name: "order_id", type: "BIGINT" },
    { name: "order_date", type: "DATE" },
    { name: "region", type: "VARCHAR" },
    { name: "revenue", type: "DOUBLE" },
  ];
  const rows: Row[] = [];
  for (let i = 1; i <= n; i++) {
    const day = String(1 + (i % 28)).padStart(2, "0");
    rows.push([
      { t: "int", v: String(i) },
      { t: "date", v: `2026-07-${day}` },
      i % 23 === 0 ? { t: "null" } : { t: "text", v: REGIONS[i % 4]! },
      i % 31 === 0 ? { t: "null" } : { t: "float", v: ((i * 37) % 400) + 0.5 },
    ]);
  }
  return { columns, rows };
}

function num(c: Cell): number | null {
  if (c.t === "int") return Number(c.v);
  if (c.t === "float") return c.v;
  return null;
}

function literal(l: Literal): Cell {
  if (l === null) return { t: "null" };
  if (typeof l === "boolean") return { t: "bool", v: l };
  if (typeof l === "string") return { t: "text", v: l };
  if ("int" in l) return { t: "int", v: l.int };
  if ("float" in l) return { t: "float", v: l.float };
  if ("date" in l) return { t: "date", v: l.date };
  return { t: "timestamp", v: l.timestamp };
}

function compare(a: Cell, b: Cell): number | null {
  if (a.t === "null" || b.t === "null") return null;
  const x = num(a);
  const y = num(b);
  if (x !== null && y !== null) return x - y;
  const s = String((a as { v: unknown }).v);
  const t = String((b as { v: unknown }).v);
  return s < t ? -1 : s > t ? 1 : 0;
}

function regionsTable(): { columns: Column[]; rows: Row[] } {
  const people = ["Priya Raman", "Tom Okafor", "Lena Hart", "Diego Alvarez", "Mei Chen"];
  return {
    columns: [
      { name: "region", type: "VARCHAR" },
      { name: "manager", type: "VARCHAR" },
      { name: "target", type: "BIGINT" },
    ],
    rows: [...REGIONS, "Central"].map((r, i) => [
      { t: "text", v: r },
      { t: "text", v: people[i]! },
      { t: "int", v: String(1200 + i * 300) },
    ]),
  };
}

interface MockSource {
  id: string;
  path: string;
  sheet?: string;
  columns: Column[];
  rows: Row[];
}

export class MockBackend implements Backend {
  name = "Regional revenue";
  sources: MockSource[] = [{ id: SOURCE_ID, path: "sales.csv", ...sourceTable(240) }];
  get source(): MockSource {
    return this.sources[0]!;
  }
  steps: MockStep[] = [];
  cells: NotebookCell[] = [];
  results = new Map<string, Result>();
  failed = new Map<string, string>();
  /** Python code (exact text) the user has approved. */
  approved = new Set<string>();
  trustAll = false;
  /** Milliseconds each step takes in `run` (tests use 0). */
  stepDelay = 0;
  private cancelled = false;

  private def(id: string): string {
    const s = this.steps.find((x) => x.id === id);
    if (!s) return id;
    return JSON.stringify([s.op, s.inputs.map((i) => this.def(i))]);
  }

  private inputOf(id: string): Result | { columns: Column[]; rows: Row[] } | undefined {
    return this.sources.find((s) => s.id === id) ?? this.results.get(id);
  }

  private upstreamNeedsReview(s: MockStep): boolean {
    return s.inputs.some((i) => {
      const up = this.steps.find((x) => x.id === i);
      return !!up && (this.needsReview(up) || this.upstreamNeedsReview(up));
    });
  }

  private needsReview(s: MockStep): boolean {
    return s.op.type === "python" && !this.trustAll && !this.approved.has(s.op.code);
  }

  private state(s: MockStep): StepStateLabel {
    const r = this.results.get(s.id);
    if (r && r.def === this.def(s.id)) return "fresh";
    if (this.needsReview(s)) return "needs review";
    if (this.upstreamNeedsReview(s)) return "waiting on review";
    if (this.failed.has(s.id)) return "failed";
    if (!r) return "not run";
    return r.def === this.def(s.id) ? "fresh" : "stale";
  }

  async view(): Promise<AnalysisView> {
    const steps: StepView[] = this.steps.map((s) => {
      const r = this.results.get(s.id);
      return {
        id: s.id,
        inputs: s.inputs,
        label: s.label,
        op: s.op,
        mode: ["filter", "aggregate", "join", "append", "derive", "select", "sort", "window", "dedupe", "pivot", "unpivot"].includes(s.op.type)
          ? "visual"
          : (s.op.type as "sql" | "python" | "chart"),
        state: this.state(s),
        detail: this.failed.get(s.id) ?? null,
        rows: r ? r.rows.length : null,
        columns: r ? r.columns : null,
        sql: s.op.type === "sql" ? s.op.sql : s.op.type === "chart" || s.op.type === "python" ? null : `SELECT … FROM input`,
        dependents: this.steps.filter((o) => o.inputs.includes(s.id)).map((o) => o.id),
      };
    });
    return {
      project_dir: "/projects/regional-revenue",
      name: this.name,
      sources: this.sources.map((x) => ({
        id: x.id,
        path: x.path,
        rows: x.rows.length,
        columns: x.columns,
        external: false,
        format: x.path.endsWith(".xlsx") ? "excel" : "csv",
        sheet: x.sheet ?? null,
      })),
      steps,
      cells: this.cells,
      python: "python 3.13 | pandas 3.0",
      review: this.steps
        .filter((s) => this.needsReview(s))
        .map((s) => ({ step_id: s.id, label: s.label, inputs: s.inputs, code: s.op.type === "python" ? s.op.code : "" })),
      trust_all: this.trustAll,
    };
  }

  async createProject(): Promise<AnalysisView> {
    return this.view();
  }
  async openProject(): Promise<AnalysisView> {
    return this.view();
  }

  private sourceById(id: string): MockSource {
    const s = this.sources.find((x) => x.id === id);
    if (!s) throw { message: `unknown source \`${id}\`` };
    return s;
  }

  async addSource(csv: string): Promise<AnalysisView> {
    const stem = csv.split(/[\\/]/).pop()!.replace(/\.[^.]*$/, "");
    let id = stem.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "") || "data";
    if (!/^[a-z]/.test(id)) id = `data_${id}`;
    const base = id;
    for (let n = 2; this.sources.some((s) => s.id === id) || this.steps.some((s) => s.id === id); n++) id = `${base}_${n}`;
    this.sources.push({ id, path: `${id === base ? stem : id}.csv`, ...regionsTable() });
    return this.view();
  }

  async sourceSheets(id: string): Promise<string[]> {
    const s = this.sourceById(id);
    return s.path.endsWith(".xlsx") ? ["Notes", "Targets"] : [];
  }

  async updateSource(id: string, newId: string | null, sheet: string | null): Promise<AnalysisView> {
    const s = this.sourceById(id);
    if (sheet !== null) s.sheet = sheet;
    if (newId && newId !== id) {
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(newId) || newId.toLowerCase() === "input")
        throw { message: `\`${newId}\` cannot be a source name: start with a letter and use only letters, digits and _` };
      if (this.sources.some((x) => x.id === newId) || this.steps.some((x) => x.id === newId))
        throw { message: `\`${newId}\` is already used in this analysis` };
      s.id = newId;
      for (const st of this.steps) {
        st.inputs = st.inputs.map((i) => (i === id ? newId : i));
        if (st.op.type === "sql") st.op = { ...st.op, sql: st.op.sql.replace(new RegExp(`\\b${id}\\b`, "g"), newId) };
      }
    }
    return this.view();
  }

  async removeSource(id: string): Promise<AnalysisView> {
    this.sourceById(id);
    if (this.sources.length === 1) throw { message: "an analysis needs at least one source" };
    const dep = this.steps.find((s) => s.inputs.includes(id));
    if (dep) throw { message: `step \`${dep.id}\` reads \`${id}\`; remove or repoint it first` };
    this.sources = this.sources.filter((s) => s.id !== id);
    return this.view();
  }

  async sourceProfile(sourceId: string): Promise<ProfileRow[]> {
    const source = this.sourceById(sourceId);
    return source.columns.map((c, i) => {
      const vals = source.rows.map((r) => r[i]!).filter((v) => v.t !== "null");
      const sorted = [...vals].sort((a, b) => compare(a, b) ?? 0);
      return {
        name: c.name,
        type: c.type,
        rows: source.rows.length,
        missing: source.rows.length - vals.length,
        distinct: new Set(vals.map((v) => JSON.stringify(v))).size,
        min: sorted[0] ?? { t: "null" },
        max: sorted[sorted.length - 1] ?? { t: "null" },
      };
    });
  }

  async sourcePage(sourceId: string, offset: number, limit: number): Promise<Page> {
    const { columns, rows } = this.sourceById(sourceId);
    return { offset, total_rows: rows.length, columns, rows: rows.slice(offset, offset + limit), stale: false };
  }

  async page(stepId: string, offset: number, limit: number): Promise<Page> {
    const r = this.results.get(stepId);
    if (!r) throw { message: `step \`${stepId}\` has no result yet` };
    const s = this.steps.find((x) => x.id === stepId)!;
    return {
      offset,
      total_rows: r.rows.length,
      columns: r.columns,
      rows: r.rows.slice(offset, offset + limit),
      stale: this.state(s) !== "fresh",
    };
  }

  async chart(stepId: string): Promise<unknown> {
    const s = this.steps.find((x) => x.id === stepId);
    if (!this.results.has(stepId) || !s || s.op.type !== "chart") throw { message: `step \`${stepId}\` has no chart yet` };
    const input = this.inputOf(s.inputs[0]!);
    const op = s.op;
    const xi = input?.columns.findIndex((c) => c.name === op.x) ?? -1;
    const yi = input?.columns.findIndex((c) => c.name === op.y) ?? -1;
    const value = (c: Cell | undefined) => (c && c.t !== "null" ? (num(c) ?? (c as { v: unknown }).v) : null);
    return {
      $schema: "https://vega.github.io/schema/vega-lite/v5.json",
      title: op.title ?? `${op.y} by ${op.x}`,
      mark: { type: op.mark, tooltip: true },
      data: { values: (input?.rows ?? []).map((r) => ({ [op.x]: value(r[xi]), [op.y]: value(r[yi]) })) },
      encoding: {
        x: { field: op.x, type: "nominal", title: op.x },
        y: { field: op.y, type: "quantitative", title: op.y },
      },
    };
  }

  private nextId(type: string): string {
    for (let n = 1; ; n++) if (!this.steps.some((s) => s.id === `${type}_${n}`)) return `${type}_${n}`;
  }

  private checkInputs(op: StepOp, inputs: string[]) {
    for (const i of inputs)
      if (!this.sources.some((s) => s.id === i) && !this.steps.some((s) => s.id === i)) throw { message: `unknown step \`${i}\`` };
    const many = op.type === "sql" || op.type === "python";
    const ok =
      op.type === "join" ? inputs.length === 2 : op.type === "append" ? inputs.length >= 2 : many ? inputs.length >= 1 : inputs.length === 1;
    if (!ok) throw { message: `step \`${op.type}\` cannot read ${inputs.length} input${inputs.length === 1 ? "" : "s"}` };
  }

  async addStep(inputs: string[], label: string | null, op: StepOp): Promise<AnalysisView> {
    this.checkInputs(op, inputs);
    const id = this.nextId(op.type);
    this.steps.push({ id, inputs: [...inputs], label, op });
    if (op.type === "python") this.approved.add(op.code);
    this.cells.push({ type: "step", step: id });
    return this.view();
  }

  async updateStep(id: string, label: string | null, op: StepOp, inputs?: string[]): Promise<AnalysisView> {
    const s = this.steps.find((x) => x.id === id);
    if (!s) throw { message: `unknown step \`${id}\`` };
    if (inputs) {
      this.checkInputs(op, inputs);
      s.inputs = [...inputs];
    }
    if (op.type === "python" && (s.op.type !== "python" || s.op.code !== op.code)) this.approved.add(op.code);
    s.op = op;
    s.label = label;
    this.failed.delete(id);
    return this.view();
  }

  async removeStep(id: string): Promise<AnalysisView> {
    const dep = this.steps.find((s) => s.inputs.includes(id));
    if (dep) throw { message: `step \`${dep.id}\` reads \`${id}\`; remove or repoint it first` };
    this.steps = this.steps.filter((s) => s.id !== id);
    this.cells = this.cells.filter((c) => !(c.type === "step" && c.step === id));
    this.results.delete(id);
    return this.view();
  }

  async setCells(cells: NotebookCell[]): Promise<AnalysisView> {
    this.cells = cells;
    return this.view();
  }

  async approve(stepIds: string[]): Promise<AnalysisView> {
    for (const id of stepIds) {
      const s = this.steps.find((x) => x.id === id);
      if (s?.op.type === "python") this.approved.add(s.op.code);
    }
    return this.view();
  }

  async setTrustAll(on: boolean): Promise<AnalysisView | null> {
    this.trustAll = on;
    return this.view();
  }

  async rename(name: string): Promise<AnalysisView> {
    this.name = name;
    return this.view();
  }

  private evaluate(s: MockStep): Result {
    const input = this.inputOf(s.inputs[0]!);
    if (!input) throw new Error(`upstream step \`${s.inputs[0]}\` did not succeed`);
    const op = s.op;
    if (op.type === "select") {
      const idx = op.columns.map((c) => input.columns.findIndex((x) => x.name === c.column));
      if (idx.some((i) => i < 0)) throw new Error("column not found");
      return {
        def: this.def(s.id),
        columns: op.columns.map((c, k) => ({ ...input.columns[idx[k]!]!, name: c.as ?? c.column })),
        rows: input.rows.map((r) => idx.map((i) => r[i]!)),
      };
    }
    if (op.type === "sort") {
      const keys = op.by.map((k) => ({ i: input.columns.findIndex((c) => c.name === k.column), d: k.descending }));
      const rows = [...input.rows].sort((a, b) => {
        for (const { i, d } of keys) {
          const [x, y] = [a[i]!, b[i]!];
          if (x.t === "null" || y.t === "null") {
            if (x.t !== y.t) return x.t === "null" ? 1 : -1;
            continue;
          }
          const c = compare(x, y) ?? 0;
          if (c) return d ? -c : c;
        }
        return 0;
      });
      return { def: this.def(s.id), columns: input.columns, rows: op.limit === null ? rows : rows.slice(0, op.limit) };
    }
    const col = (name: string) => {
      const i = input.columns.findIndex((c) => c.name === name);
      if (i < 0) throw new Error(`column \`${name}\` not found`);
      return i;
    };
    const ordered = (rows: number[], by: { column: string; descending: boolean }[]) => {
      const keys = by.map((k) => ({ i: col(k.column), d: k.descending }));
      return [...rows].sort((a, b) => {
        for (const { i, d } of keys) {
          const [x, y] = [input.rows[a]![i]!, input.rows[b]![i]!];
          if (x.t === "null" || y.t === "null") {
            if (x.t !== y.t) return x.t === "null" ? 1 : -1;
            continue;
          }
          const c = compare(x, y) ?? 0;
          if (c) return d ? -c : c;
        }
        return a - b;
      });
    };
    const groupsOf = (cols: number[]) => {
      const m = new Map<string, number[]>();
      input.rows.forEach((r, i) => {
        const k = JSON.stringify(cols.map((c) => r[c]));
        m.set(k, [...(m.get(k) ?? []), i]);
      });
      return [...m.values()];
    };
    if (op.type === "window") {
      // A stand-in: totals, numbering, previous and next values.
      const out = input.rows.map((r) => [...r]);
      const columns = [...input.columns];
      for (const c of op.columns) {
        if (columns.some((x) => x.name === c.name)) throw new Error(`there is already a column \`${c.name}\`; choose another name`);
        const ci = c.column ? col(c.column) : -1;
        const type: Column["type"] = ["row_number", "rank"].includes(c.func) ? "BIGINT" : ["previous", "next", "running_sum", "change"].includes(c.func) ? input.columns[ci]!.type : "DOUBLE";
        columns.push({ name: c.name, type });
        for (const g of groupsOf(op.partition_by.map(col))) {
          const rows = ordered(g, op.order_by);
          let run = 0;
          rows.forEach((ri, pos) => {
            const v = ci >= 0 ? input.rows[ri]![ci]! : ({ t: "null" } as Cell);
            const n = c.n ?? 1;
            const at = (p: number): Cell => (p >= 0 && p < rows.length ? input.rows[rows[p]!]![ci]! : { t: "null" });
            const x = num(v);
            if (x !== null) run += x;
            const cell: Cell =
              c.func === "row_number" || c.func === "rank"
                ? { t: "int", v: String(pos + 1) }
                : c.func === "previous"
                  ? at(pos - n)
                  : c.func === "next"
                    ? at(pos + n)
                    : c.func === "running_sum"
                      ? type === "BIGINT" ? { t: "int", v: String(run) } : { t: "float", v: run }
                      : { t: "null" };
            out[ri]!.push(cell);
          });
        }
      }
      return { def: this.def(s.id), columns, rows: out };
    }
    if (op.type === "dedupe") {
      const keys = op.keys.length ? op.keys.map(col) : input.columns.map((_, i) => i);
      const keep = groupsOf(keys).map((g) => ordered(g, op.order_by)[0]!).sort((a, b) => a - b);
      return { def: this.def(s.id), columns: input.columns, rows: keep.map((i) => input.rows[i]!) };
    }
    if (op.type === "unpivot") {
      const idx = op.columns.map(col);
      const kept = input.columns.map((_, i) => i).filter((i) => !idx.includes(i));
      const type = input.columns[idx[0] ?? 0]?.type ?? "VARCHAR";
      return {
        def: this.def(s.id),
        columns: [...kept.map((i) => input.columns[i]!), { name: op.name, type: "VARCHAR" }, { name: op.value, type }],
        rows: input.rows.flatMap((r) => op.columns.map((c, k): Row => [...kept.map((i) => r[i]!), { t: "text", v: c }, r[idx[k]!]!])),
      };
    }
    if (op.type === "pivot") {
      // Counts, or sums of a column; values in text order.
      const pi = col(op.column);
      const vi = op.value ? col(op.value) : -1;
      const values = [...new Set(input.rows.map((r) => r[pi]!).filter((c) => c.t !== "null").map((c) => String(c.v)))].sort();
      const gs = groupsOf(op.rows.map(col));
      return {
        def: this.def(s.id),
        columns: [
          ...op.rows.map((r) => input.columns[col(r)]!),
          ...values.map((v) => ({ name: v, type: (op.func === "COUNT" ? "BIGINT" : "DOUBLE") as Column["type"] })),
        ],
        rows: gs.map((g) => [
          ...op.rows.map((r) => input.rows[g[0]!]![col(r)]!),
          ...values.map((v): Cell => {
            const hit = g.filter((i) => input.rows[i]![pi]!.t !== "null" && String(input.rows[i]![pi]!.v) === v);
            if (op.func === "COUNT") return { t: "int", v: String(hit.length) };
            const xs = hit.map((i) => num(input.rows[i]![vi]!)).filter((x): x is number => x !== null);
            return xs.length ? { t: "float", v: xs.reduce((a, b) => a + b, 0) } : { t: "null" };
          }),
        ]),
      };
    }
    if (op.type === "derive") {
      // The mock does not evaluate formulas: a bare column name is copied,
      // anything else is left missing. The real evaluator is in Rust.
      let columns = [...input.columns];
      let rows = input.rows.map((r) => [...r]);
      for (const d of op.columns) {
        const src = columns.findIndex((c) => c.name === d.formula.trim());
        const at = columns.findIndex((c) => c.name === d.name);
        const type = src >= 0 ? columns[src]!.type : "VARCHAR";
        rows = rows.map((r) => {
          const v: Cell = src >= 0 ? r[src]! : { t: "null" };
          if (at >= 0) r[at] = v;
          else r.push(v);
          return r;
        });
        if (at >= 0) columns[at] = { name: d.name, type };
        else columns = [...columns, { name: d.name, type }];
      }
      return { def: this.def(s.id), columns, rows };
    }
    if (op.type === "append") {
      const tables = s.inputs.map((i) => {
        const t = this.inputOf(i);
        if (!t) throw new Error(`upstream step \`${i}\` did not succeed`);
        return { id: i, t };
      });
      const columns: Column[] = [];
      for (const { t } of tables) for (const c of t.columns) if (!columns.some((x) => x.name === c.name)) columns.push({ ...c });
      if (op.origin) columns.push({ name: op.origin, type: "VARCHAR" });
      const rows: Row[] = [];
      const seen = new Set<string>();
      for (const { id, t } of tables)
        for (const r of t.rows) {
          const row: Row = columns.map((c) => {
            if (c.name === op.origin) return { t: "text", v: id };
            const i = t.columns.findIndex((x) => x.name === c.name);
            return i < 0 ? { t: "null" } : r[i]!;
          });
          const key = JSON.stringify(row);
          if (op.distinct && seen.has(key)) continue;
          seen.add(key);
          rows.push(row);
        }
      return { def: this.def(s.id), columns, rows };
    }
    if (op.type === "join") {
      const right = this.inputOf(s.inputs[1]!);
      if (!right) throw new Error(`upstream step \`${s.inputs[1]}\` did not succeed`);
      const li = op.on.map((k) => input.columns.findIndex((c) => c.name === k.left));
      const ri = op.on.map((k) => right.columns.findIndex((c) => c.name === k.right));
      if (!op.on.length || li.some((i) => i < 0) || ri.some((i) => i < 0)) throw new Error("choose columns to join on");
      const isEq = op.on.map((k) => (k.op ?? "=") === "=");
      const keep = right.columns.map((_, i) => i).filter((i) => !ri.some((r, k) => r === i && isEq[k]));
      const names = new Set(input.columns.map((c) => c.name));
      const columns = [
        ...input.columns,
        ...keep.map((i) => {
          const c = right.columns[i]!;
          return names.has(c.name) ? { ...c, name: `${c.name}_${s.inputs[1]}` } : c;
        }),
      ];
      const nulls = (n: number): Cell[] => Array.from({ length: n }, () => ({ t: "null" }));
      const test = { "=": (c: number) => c === 0, "<>": (c: number) => c !== 0, "<": (c: number) => c < 0, "<=": (c: number) => c <= 0, ">": (c: number) => c > 0, ">=": (c: number) => c >= 0 } as Record<string, (c: number) => boolean>;
      const eq = (a: Row, b: Row) =>
        li.every((x, k) => {
          const c = compare(a[x]!, b[ri[k]!]!);
          return c !== null && (test[op.on[k]!.op ?? "="] ?? (() => false))(c);
        });
      const rows: Row[] = [];
      const used = new Set<number>();
      for (const l of input.rows) {
        let any = false;
        right.rows.forEach((r, j) => {
          if (eq(l, r)) {
            any = true;
            used.add(j);
            rows.push([...l, ...keep.map((i) => r[i]!)]);
          }
        });
        if (!any && op.how !== "inner") rows.push([...l, ...nulls(keep.length)]);
      }
      if (op.how === "full")
        right.rows.forEach((r, j) => {
          if (used.has(j)) return;
          const left = nulls(input.columns.length);
          op.on.forEach((k, n) => isEq[n] && (left[li[n]!] = r[ri[n]!]!));
          rows.push([...left, ...keep.map((i) => r[i]!)]);
        });
      return { def: this.def(s.id), columns, rows };
    }
    if (op.type === "filter") {
      const idx = op.predicates.map((p) => input.columns.findIndex((c) => c.name === p.column));
      if (idx.some((i) => i < 0)) throw new Error("column not found");
      const rows = input.rows.filter((r) =>
        op.predicates.every((p, k) => {
          const v = r[idx[k]!]!;
          if (p.op === "IS NULL") return v.t === "null";
          if (p.op === "IS NOT NULL") return v.t !== "null";
          const c = compare(v, literal(p.value));
          if (c === null) return false;
          return { "=": c === 0, "<>": c !== 0, "<": c < 0, "<=": c <= 0, ">": c > 0, ">=": c >= 0 }[p.op];
        }),
      );
      return { def: this.def(s.id), columns: input.columns, rows };
    }
    if (op.type === "aggregate") {
      const gi = op.group_by.map((g) => input.columns.findIndex((c) => c.name === g));
      const groups = new Map<string, Row[]>();
      for (const r of input.rows) {
        const k = JSON.stringify(gi.map((i) => r[i]));
        groups.set(k, [...(groups.get(k) ?? []), r]);
      }
      const columns: Column[] = [
        ...gi.map((i) => input.columns[i]!),
        ...op.measures.map((m) => ({ name: m.alias, type: (m.func === "COUNT" ? "BIGINT" : "DOUBLE") as Column["type"] })),
      ];
      const rows = [...groups.entries()]
        .sort(([a], [b]) => (a < b ? -1 : 1))
        .map(([k, rs]) => [
          ...(JSON.parse(k) as Cell[]),
          ...op.measures.map((m): Cell => {
            if (m.func === "COUNT") return { t: "int", v: String(rs.length) };
            const ci = input.columns.findIndex((c) => c.name === m.column);
            const xs = rs.map((r) => num(r[ci]!)).filter((x): x is number => x !== null);
            if (!xs.length) return { t: "null" };
            if (m.func === "COUNT_DISTINCT") return { t: "int", v: String(new Set(xs).size) };
            const sum = xs.reduce((a, b) => a + b, 0);
            const sorted = [...xs].sort((a, b) => a - b);
            const q = (p: number) => {
              const h = (sorted.length - 1) * p;
              const lo = Math.floor(h);
              return sorted[lo]! + (h - lo) * (sorted[Math.min(lo + 1, sorted.length - 1)]! - sorted[lo]!);
            };
            const mean = sum / xs.length;
            const fixed: Record<string, number | null> = {
              SUM: sum,
              AVG: mean,
              MIN: Math.min(...xs),
              MAX: Math.max(...xs),
              MEDIAN: q(0.5),
              STDDEV: xs.length < 2 ? null : Math.sqrt(xs.reduce((a, x) => a + (x - mean) ** 2, 0) / (xs.length - 1)),
            };
            const v = m.func in fixed ? fixed[m.func]! : q(Number(m.func.slice(1)) / 100);
            return v === null ? { t: "null" } : { t: "float", v };
          }),
        ]);
      return { def: this.def(s.id), columns, rows };
    }
    if (op.type === "python" && op.code.includes("raise")) throw new Error("ValueError: raised by the step (line 1)");
    return { def: this.def(s.id), columns: input.columns, rows: input.rows.slice(0, 20) };
  }

  // ---- console. The mock's checks are a small stand-in for the Rust linter.
  dismissed = new Set<string>();
  runs: RunView[] = [];

  private lintStep(st: { id: string; op: StepOp }): Problem[] {
    const out: Problem[] = [];
    const base = { step: st.id, line: null, column: null, from_last_run: false, dismissable: false };
    if (st.op.type === "derive")
      st.op.columns.forEach((c, i) => {
        if (!c.formula.trim() || /[+\-*/&,(]\s*$/.test(c.formula))
          out.push({ ...base, severity: "error", code: "formula", field: `columns.${i}.formula`, message: "the formula ends too early" });
      });
    if (st.op.type === "python") {
      if (!/\bresult\s*=/.test(st.op.code))
        out.push({ ...base, severity: "error", code: "python-result", field: "code", message: "never assigns a DataFrame to `result`" });
      st.op.code.split("\n").forEach((l, i) => {
        if (l.includes(".iterrows("))
          out.push({
            ...base,
            severity: "warning",
            code: "row-loop",
            field: "code",
            line: i + 1,
            dismissable: true,
            message: "works through rows one at a time; operations on whole columns are usually far faster on large inputs",
          });
      });
    }
    return out;
  }

  async problems(): Promise<Problems> {
    let items: Problem[] = [];
    let stale = 0;
    for (const st of this.steps) {
      items.push(...this.lintStep(st));
      const state = this.state(st);
      if (state === "failed")
        items.push({ severity: "error", code: "failed", step: st.id, field: null, line: null, column: null, message: `failed on the last run: ${this.failed.get(st.id)}`, from_last_run: true, dismissable: false });
      if (state === "stale") stale++;
    }
    if (stale)
      items.push({ severity: "info", code: "outdated", step: null, field: null, line: null, column: null, message: `${stale} result${stale === 1 ? " is" : "s are"} outdated; run the analysis to update`, from_last_run: false, dismissable: true });
    const before = items.length;
    items = items.filter((p) => !(p.dismissable && this.dismissed.has(`${p.step}|${p.code}`)));
    const rank = { error: 0, warning: 1, info: 2 };
    items.sort((a, b) => rank[a.severity] - rank[b.severity]);
    return { items, dismissed: before - items.length };
  }

  async checkDraft(id: string | null, _inputs: string[], op: StepOp): Promise<Problem[]> {
    return this.lintStep({ id: id ?? "new", op });
  }

  async dismiss(step: string | null, code: string, dismissed: boolean): Promise<Problems> {
    const key = `${step}|${code}`;
    if (dismissed) this.dismissed.add(key);
    else this.dismissed.delete(key);
    return this.problems();
  }

  async restoreDismissed(): Promise<Problems> {
    this.dismissed.clear();
    return this.problems();
  }

  async runLog(limit: number): Promise<RunView[]> {
    return this.runs.slice(0, limit);
  }

  async run({ onStep }: RunHandlers): Promise<RunSummary> {
    this.cancelled = false;
    const events: StepEvent[] = [];
    const ok = new Set<string>(this.sources.map((x) => x.id));
    const review = new Set<string>();
    let halted = false;
    for (const s of this.steps) {
      if (this.stepDelay) await new Promise((r) => setTimeout(r, this.stepDelay));
      let e: StepEvent;
      if (halted || this.cancelled) {
        halted = true;
        e = { step_id: s.id, status: "skipped", cached: false, detail: "run was cancelled", rows: null, duration_ms: 0 };
      } else if (s.inputs.some((i) => !ok.has(i))) {
        const bad = s.inputs.find((i) => !ok.has(i))!;
        const waiting = review.has(bad);
        if (waiting) review.add(s.id);
        const detail = waiting ? `step \`${bad}\` is waiting for your review` : `upstream step \`${bad}\` did not succeed`;
        e = { step_id: s.id, status: "skipped", cached: false, detail, rows: null, duration_ms: 0 };
      } else if (this.state(s) === "fresh") {
        ok.add(s.id);
        e = { step_id: s.id, status: "succeeded", cached: true, detail: null, rows: this.results.get(s.id)!.rows.length, duration_ms: 0 };
      } else if (this.needsReview(s)) {
        review.add(s.id);
        e = { step_id: s.id, status: "needs review", cached: false, detail: null, rows: null, duration_ms: 0 };
      } else {
        try {
          const r = this.evaluate(s);
          this.results.set(s.id, r);
          this.failed.delete(s.id);
          ok.add(s.id);
          const printed = s.op.type === "python" && s.op.code.includes("print(") ? `(mock) printed by ${s.id}\n` : null;
          e = { step_id: s.id, status: "succeeded", cached: false, detail: null, rows: r.rows.length, duration_ms: 3, output: printed };
        } catch (err) {
          this.failed.set(s.id, (err as Error).message);
          e = { step_id: s.id, status: "failed", cached: false, detail: (err as Error).message, rows: null, duration_ms: 1 };
        }
      }
      events.push(e);
      onStep(e);
    }
    const summary = { run_id: `run-${Date.now()}`, succeeded: events.every((e) => e.status === "succeeded"), steps: events };
    this.runs.unshift({ ...summary, started_unix_ms: Date.now() });
    return summary;
  }

  async cancel(): Promise<void> {
    this.cancelled = true;
  }

  /** A small worked analysis for previews: filter → aggregate → Python → chart. */
  async seedDemo(opts: { run: boolean; editAfterRun: boolean; received?: boolean; join?: boolean }) {
    await this.addStep([SOURCE_ID], "Orders with a region", {
      type: "filter",
      predicates: [
        { column: "region", op: "IS NOT NULL", value: null },
        { column: "revenue", op: ">=", value: { int: "20" } },
      ],
    });
    await this.addStep(["filter_1"], "Revenue by region", {
      type: "aggregate",
      group_by: ["region"],
      measures: [
        { func: "COUNT", column: null, alias: "orders" },
        { func: "SUM", column: "revenue", alias: "revenue" },
      ],
    });
    await this.addStep(["aggregate_1"], "Share of revenue", {
      type: "python",
      code: "result = df.assign(share=df['revenue'] / df['revenue'].sum())",
      timeout_ms: 60000,
    });
    await this.addStep(["python_1"], null, { type: "chart", mark: "bar", x: "region", y: "revenue", title: null });
    this.cells.unshift({ type: "markdown", text: "## Question\nWhich regions bring in most of the revenue?" });
    if (opts.join) {
      await this.addSource("/data/regions.csv");
      await this.addStep(["aggregate_1", "regions"], "Revenue against target", {
        type: "join",
        how: "full",
        on: [{ left: "region", right: "region" }],
      });
    }
    // A project received from someone else: their code is not approved here.
    if (opts.received) {
      this.approved.clear();
      this.name = "Regional revenue (from Dana)";
    }
    if (opts.run) await this.run({ onStep: () => {} });
    if (opts.editAfterRun) {
      const f = this.steps[0]!;
      if (f.op.type === "filter") f.op.predicates[1]!.value = { int: "100" };
    }
  }

  async startupProject(): Promise<string | null> {
    return "/projects/regional-revenue";
  }
  /** The file the next "choose a CSV" dialog returns. */
  csvToPick = "/data/regions.csv";
  async pickCsv(): Promise<string | null> {
    return this.csvToPick;
  }
  /** Exports made, for tests: [id, path]. */
  exports: [string, string][] = [];
  async pickSavePath(defaultName: string): Promise<string | null> {
    return `/exports/${defaultName}`;
  }
  async exportResult(id: string, path: string): Promise<number> {
    const s = this.steps.find((x) => x.id === id);
    if (s && this.state(s) !== "fresh") throw { message: `the result of \`${id}\` is outdated; run the analysis first so the file matches the step` };
    const t = this.inputOf(id);
    if (!t) throw { message: `step \`${id}\` has no result yet; run the analysis first` };
    this.exports.push([id, path]);
    return t.rows.length;
  }
  async pickFolder(): Promise<string | null> {
    return "/projects/new";
  }
}
