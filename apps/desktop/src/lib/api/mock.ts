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
  ProfileRow,
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
  input: string;
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

export class MockBackend implements Backend {
  name = "Regional revenue";
  source = sourceTable(240);
  steps: MockStep[] = [];
  cells: NotebookCell[] = [];
  results = new Map<string, Result>();
  failed = new Map<string, string>();
  /** Milliseconds each step takes in `run` (tests use 0). */
  stepDelay = 0;
  private cancelled = false;

  private def(id: string): string {
    const s = this.steps.find((x) => x.id === id);
    if (!s) return "source";
    return JSON.stringify([s.op, this.def(s.input)]);
  }

  private inputOf(id: string): Result | { columns: Column[]; rows: Row[] } | undefined {
    if (id === SOURCE_ID) return this.source;
    return this.results.get(id);
  }

  private state(s: MockStep): StepStateLabel {
    const r = this.results.get(s.id);
    if (this.failed.has(s.id)) return "failed";
    if (!r) return "not run";
    return r.def === this.def(s.id) ? "fresh" : "stale";
  }

  async view(): Promise<AnalysisView> {
    const steps: StepView[] = this.steps.map((s) => {
      const r = this.results.get(s.id);
      return {
        id: s.id,
        input: s.input,
        label: s.label,
        op: s.op,
        mode: s.op.type === "filter" || s.op.type === "aggregate" ? "visual" : s.op.type,
        state: this.state(s),
        detail: this.failed.get(s.id) ?? null,
        rows: r ? r.rows.length : null,
        columns: r ? r.columns : null,
        sql: s.op.type === "sql" ? s.op.sql : s.op.type === "chart" || s.op.type === "python" ? null : `SELECT … FROM input`,
        dependents: this.steps.filter((o) => o.input === s.id).map((o) => o.id),
      };
    });
    return {
      project_dir: "/projects/regional-revenue",
      name: this.name,
      source: { path: "sales.csv", rows: this.source.rows.length, columns: this.source.columns },
      steps,
      cells: this.cells,
      python: "python 3.13 | pandas 3.0",
    };
  }

  async createProject(): Promise<AnalysisView> {
    return this.view();
  }
  async openProject(): Promise<AnalysisView> {
    return this.view();
  }

  async sourceProfile(): Promise<ProfileRow[]> {
    return this.source.columns.map((c, i) => {
      const vals = this.source.rows.map((r) => r[i]!).filter((v) => v.t !== "null");
      const sorted = [...vals].sort((a, b) => compare(a, b) ?? 0);
      return {
        name: c.name,
        type: c.type,
        rows: this.source.rows.length,
        missing: this.source.rows.length - vals.length,
        distinct: new Set(vals.map((v) => JSON.stringify(v))).size,
        min: sorted[0] ?? { t: "null" },
        max: sorted[sorted.length - 1] ?? { t: "null" },
      };
    });
  }

  async sourcePage(offset: number, limit: number): Promise<Page> {
    const { columns, rows } = this.source;
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
    const input = this.inputOf(s.input);
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

  async addStep(input: string, label: string | null, op: StepOp): Promise<AnalysisView> {
    if (input !== SOURCE_ID && !this.steps.some((s) => s.id === input)) throw { message: `unknown step \`${input}\`` };
    const id = this.nextId(op.type);
    this.steps.push({ id, input, label, op });
    this.cells.push({ type: "step", step: id });
    return this.view();
  }

  async updateStep(id: string, label: string | null, op: StepOp): Promise<AnalysisView> {
    const s = this.steps.find((x) => x.id === id);
    if (!s) throw { message: `unknown step \`${id}\`` };
    s.op = op;
    s.label = label;
    this.failed.delete(id);
    return this.view();
  }

  async removeStep(id: string): Promise<AnalysisView> {
    const dep = this.steps.find((s) => s.input === id);
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

  async rename(name: string): Promise<AnalysisView> {
    this.name = name;
    return this.view();
  }

  private evaluate(s: MockStep): Result {
    const input = this.inputOf(s.input);
    if (!input) throw new Error(`upstream step \`${s.input}\` did not succeed`);
    const op = s.op;
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
            const sum = xs.reduce((a, b) => a + b, 0);
            const v = { SUM: sum, AVG: sum / xs.length, MIN: Math.min(...xs), MAX: Math.max(...xs) }[m.func];
            return { t: "float", v };
          }),
        ]);
      return { def: this.def(s.id), columns, rows };
    }
    if (op.type === "python" && op.code.includes("raise")) throw new Error("ValueError: raised by the step (line 1)");
    return { def: this.def(s.id), columns: input.columns, rows: input.rows.slice(0, 20) };
  }

  async run({ onStep }: RunHandlers): Promise<RunSummary> {
    this.cancelled = false;
    const events: StepEvent[] = [];
    const ok = new Set<string>([SOURCE_ID]);
    let halted = false;
    for (const s of this.steps) {
      if (this.stepDelay) await new Promise((r) => setTimeout(r, this.stepDelay));
      let e: StepEvent;
      if (halted || this.cancelled) {
        halted = true;
        e = { step_id: s.id, status: "skipped", cached: false, detail: "run was cancelled", rows: null, duration_ms: 0 };
      } else if (!ok.has(s.input)) {
        e = { step_id: s.id, status: "skipped", cached: false, detail: `upstream step \`${s.input}\` did not succeed`, rows: null, duration_ms: 0 };
      } else if (this.state(s) === "fresh") {
        ok.add(s.id);
        e = { step_id: s.id, status: "succeeded", cached: true, detail: null, rows: this.results.get(s.id)!.rows.length, duration_ms: 0 };
      } else {
        try {
          const r = this.evaluate(s);
          this.results.set(s.id, r);
          this.failed.delete(s.id);
          ok.add(s.id);
          e = { step_id: s.id, status: "succeeded", cached: false, detail: null, rows: r.rows.length, duration_ms: 3 };
        } catch (err) {
          this.failed.set(s.id, (err as Error).message);
          e = { step_id: s.id, status: "failed", cached: false, detail: (err as Error).message, rows: null, duration_ms: 1 };
        }
      }
      events.push(e);
      onStep(e);
    }
    return { run_id: `run-${Date.now()}`, succeeded: events.every((e) => e.status === "succeeded"), steps: events };
  }

  async cancel(): Promise<void> {
    this.cancelled = true;
  }

  /** A small worked analysis for previews: filter → aggregate → Python → chart. */
  async seedDemo(opts: { run: boolean; editAfterRun: boolean }) {
    await this.addStep(SOURCE_ID, "Orders with a region", {
      type: "filter",
      predicates: [
        { column: "region", op: "IS NOT NULL", value: null },
        { column: "revenue", op: ">=", value: { int: "20" } },
      ],
    });
    await this.addStep("filter_1", "Revenue by region", {
      type: "aggregate",
      group_by: ["region"],
      measures: [
        { func: "COUNT", column: null, alias: "orders" },
        { func: "SUM", column: "revenue", alias: "revenue" },
      ],
    });
    await this.addStep("aggregate_1", "Share of revenue", {
      type: "python",
      code: "result = df.assign(share=df['revenue'] / df['revenue'].sum())",
      timeout_ms: 60000,
    });
    await this.addStep("python_1", null, { type: "chart", mark: "bar", x: "region", y: "revenue", title: null });
    this.cells.unshift({ type: "markdown", text: "## Question\nWhich regions bring in most of the revenue?" });
    if (opts.run) await this.run({ onStep: () => {} });
    if (opts.editAfterRun) {
      const f = this.steps[0]!;
      if (f.op.type === "filter") f.op.predicates[1]!.value = { int: "100" };
    }
  }

  async startupProject(): Promise<string | null> {
    return "/projects/regional-revenue";
  }
  async pickCsv(): Promise<string | null> {
    return "/data/sales.csv";
  }
  async pickFolder(): Promise<string | null> {
    return "/projects/new";
  }
}
