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
  CheckReport,
  ColumnDetail,
  Parameter,
  Comment,
  Every,
  PythonView,
  ScheduleView,
  SourceFormat,
  VersionView,
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
  query?: string | null;
  columns: Column[];
  rows: Row[];
}

/** What a saved change keeps (results are not part of it). */
interface MockSnapshot {
  name: string;
  sources: MockSource[];
  steps: MockStep[];
  cells: NotebookCell[];
  parameters: Parameter[];
}

/** Methods that change the analysis, and how each is described. */
const MUTATORS: [string, (a: unknown[]) => string][] = [
  ["addStep", (a) => `Added ${(a[1] as string | null) ?? `a ${(a[2] as StepOp).type} step`}`],
  ["updateStep", (a) => `Edited ${(a[1] as string | null) ?? `step \`${a[0]}\``}`],
  ["removeStep", (a) => `Deleted step \`${a[0]}\``],
  ["setCells", () => "Edited the notebook"],
  ["rename", (a) => `Renamed the analysis to "${a[0]}"`],
  ["setParameters", () => "Changed parameters"],
  ["renameParameter", () => "Renamed a parameter"],
  ["addSource", (a) => `Added source ${String(a[0]).split(/[\\/]/).pop()}`],
  ["removeSource", (a) => `Removed source \`${a[0]}\``],
  ["updateSource", (a) => (a[1] ? `Renamed source \`${a[0]}\` to \`${a[1]}\`` : `Chose sheet ${a[2]}`)],
  ["setSourceRead", (a) => (a[2] ? "Changed the query" : `Read table ${a[1]}`)],
];

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

  parameters: Parameter[] = [];
  undoStack: { snap: MockSnapshot; what: string }[] = [];
  redoStack: { snap: MockSnapshot; what: string }[] = [];
  versions: (VersionView & { snap: MockSnapshot })[] = [];
  commentList: Comment[] = [];
  author = "you";
  python: PythonView = {
    interpreter: "python3",
    executable: "/usr/bin/python3",
    version: "3.13.1",
    packages: [
      { name: "pandas", purpose: "Python steps (required)", required: true, version: "3.0.5" },
      { name: "pyarrow", purpose: "Python steps (required)", required: true, version: "22.0.0" },
      { name: "numpy", purpose: "numerical work", required: false, version: "2.3.4" },
      { name: "scipy", purpose: "statistics", required: false, version: null },
      { name: "duckdb", purpose: "rerunning exported Jupyter notebooks", required: false, version: null },
    ],
    problem: null,
    chosen_by: "default",
    candidates: ["/projects/regional-revenue/.venv/bin/python", "/opt/conda/envs/stats/bin/python"],
  };
  checkReports = new Map<string, CheckReport>();

  constructor() {
    // Every change is recorded for undo and history, as the app does.
    const self = this as unknown as Record<string, (...a: unknown[]) => Promise<unknown>>;
    for (const [name, what] of MUTATORS) {
      const f = self[name]!.bind(this);
      self[name] = async (...args: unknown[]) => {
        const before = this.snapshot();
        const r = await f(...args);
        if (JSON.stringify(before) !== JSON.stringify(this.snapshot())) this.recordEdit(before, what(args));
        if (Array.isArray(r)) return [await this.view(), r[1]];
        return this.view();
      };
    }
  }

  private snapshot(): MockSnapshot {
    // JSON, not structuredClone: definitions passed in from the UI may be reactive proxies.
    return JSON.parse(JSON.stringify({ name: this.name, sources: this.sources, steps: this.steps, cells: this.cells, parameters: this.parameters }));
  }

  private restore(s: MockSnapshot) {
    const c = structuredClone(s);
    Object.assign(this, { name: c.name, sources: c.sources, steps: c.steps, cells: c.cells, parameters: c.parameters });
  }

  private addVersion(what: string, snap: MockSnapshot) {
    const version = (this.versions[0]?.version ?? 0) + 1;
    this.versions.unshift({ version, time_unix_ms: Date.now(), what, current: false, snap: structuredClone(snap) });
  }

  private recordEdit(before: MockSnapshot, what: string) {
    if (!this.versions.length) this.addVersion("As first opened", before);
    this.undoStack.push({ snap: before, what });
    this.redoStack = [];
    this.addVersion(what, this.snapshot());
  }

  async undo(): Promise<AnalysisView> {
    const e = this.undoStack.pop();
    if (!e) throw { message: "there is nothing to undo" };
    this.redoStack.push({ snap: this.snapshot(), what: e.what });
    this.restore(e.snap);
    this.addVersion(`Undid: ${e.what}`, this.snapshot());
    return this.view();
  }

  async redo(): Promise<AnalysisView> {
    const e = this.redoStack.pop();
    if (!e) throw { message: "there is nothing to redo" };
    this.undoStack.push({ snap: this.snapshot(), what: e.what });
    this.restore(e.snap);
    this.addVersion(`Redid: ${e.what}`, this.snapshot());
    return this.view();
  }

  async history(limit: number): Promise<VersionView[]> {
    const now = JSON.stringify(this.snapshot());
    let marked = false;
    return this.versions.slice(0, limit).map(({ snap, ...v }) => {
      const current = !marked && JSON.stringify(snap) === now;
      marked ||= current;
      return { ...v, current };
    });
  }

  async restoreVersion(version: number): Promise<AnalysisView> {
    const v = this.versions.find((x) => x.version === version);
    if (!v) throw { message: `there is no saved version ${version}` };
    const before = this.snapshot();
    this.restore(v.snap);
    this.recordEdit(before, `Restored version ${version}`);
    return this.view();
  }

  async comments(): Promise<Comment[]> {
    return this.commentList.map((c) => ({ ...c }));
  }

  async addComment(target: string, text: string, replyTo: string | null): Promise<Comment[]> {
    if (!text.trim()) throw { message: "write a comment first" };
    const parent = replyTo ? this.commentList.find((c) => c.id === replyTo) : null;
    if (replyTo && !parent) throw { message: "the comment being replied to no longer exists" };
    const n = Math.max(0, ...this.commentList.map((c) => Number(c.id.slice(1)))) + 1;
    this.commentList.push({ id: `c${n}`, target: parent?.target ?? target, reply_to: replyTo, author: this.author, time_unix_ms: Date.now(), text: text.trim(), resolved: false });
    return this.comments();
  }

  async resolveComment(id: string, resolved: boolean): Promise<Comment[]> {
    for (const c of this.commentList) if (c.id === id || c.reply_to === id) c.resolved = resolved;
    return this.comments();
  }

  async deleteComment(id: string): Promise<Comment[]> {
    const c = this.commentList.find((x) => x.id === id);
    if (!c) throw { message: "that comment no longer exists" };
    if (c.author !== this.author) throw { message: `only ${c.author} can delete this comment; you can resolve it instead` };
    this.commentList = this.commentList.filter((x) => x.id !== id && x.reply_to !== id);
    return this.comments();
  }

  async pythonEnvironment(): Promise<PythonView> {
    return structuredClone(this.python);
  }

  async setPython(path: string): Promise<PythonView> {
    if (path.includes("missing")) throw { message: `cannot use ${path}: it has no pandas or pyarrow; install with: ${path} -m pip install pandas pyarrow` };
    this.python = { ...this.python, interpreter: path, executable: path, chosen_by: "chosen", candidates: this.python.candidates.filter((c) => c !== path) };
    for (const [id, r] of this.results) if (this.steps.find((s) => s.id === id)?.op.type === "python") r.def = "";
    return this.pythonEnvironment();
  }

  async schedule(every: Every, hour: number, minute: number, exports: [string, string][]): Promise<ScheduleView> {
    const when = { hourly: `${minute} * * * *`, daily: `${minute} ${hour} * * *`, weekdays: `${minute} ${hour} * * 1-5`, weekly: `${minute} ${hour} * * 1` }[every];
    const command = ["vda-run", "/projects/regional-revenue", ...exports.flatMap(([id, p]) => ["--export", `${id}=${p}`])].join(" ");
    return { command, entry: `${when} ${command} >> /projects/regional-revenue/runs/scheduled.log 2>&1`, how: "Add this line with: crontab -e", scheduler: "cron" };
  }

  async setSourceRead(id: string, table: string | null, query: string | null): Promise<AnalysisView> {
    const s = this.sourceById(id);
    if (!/\.(sqlite|db|duckdb)$/.test(s.path)) throw { message: `\`${id}\` is not a database` };
    if (query && !/^\s*select\b/i.test(query)) throw { message: "the query must only read (SELECT); it cannot change the database" };
    if (!query && !table) throw { message: "choose a table, or write a query" };
    s.query = query || null;
    if (table) s.sheet = table;
    return this.view();
  }
  /** Reports and notebooks written, for tests: [path, contents]. */
  documents: [string, string][] = [];

  /** Parameters a step names: @x in formulas, $x in SQL, a filter's parameter. */
  private paramsOf(op: StepOp): Parameter[] {
    const text = JSON.stringify(op);
    return this.parameters.filter(
      (p) => op.type === "python" || new RegExp(`[@$]${p.name}\\b|"param":"${p.name}"`).test(text),
    );
  }

  private def(id: string): string {
    const s = this.steps.find((x) => x.id === id);
    if (!s) return id;
    return JSON.stringify([s.op, s.inputs.map((i) => this.def(i)), this.paramsOf(s.op).map((p) => p.value)]);
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
        mode: ["filter", "aggregate", "join", "append", "derive", "select", "sort", "window", "dedupe", "pivot", "unpivot", "check", "test"].includes(s.op.type)
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
        format: (x.path.endsWith(".xlsx") ? "excel" : /\.(sqlite|db)$/.test(x.path) ? "sqlite" : x.path.endsWith(".duckdb") ? "duckdb" : "csv") as SourceFormat,
        sheet: x.sheet ?? null,
        query: x.query ?? null,
      })),
      steps,
      cells: this.cells,
      python: "python 3.13 | pandas 3.0",
      review: this.steps
        .filter((s) => this.needsReview(s))
        .map((s) => ({ step_id: s.id, label: s.label, inputs: s.inputs, code: s.op.type === "python" ? s.op.code : "" })),
      trust_all: this.trustAll,
      parameters: this.parameters.map((p) => ({ ...p })),
      undo: this.undoStack.at(-1)?.what ?? null,
      redo: this.redoStack.at(-1)?.what ?? null,
    };
  }

  async setParameters(params: Parameter[]): Promise<AnalysisView> {
    for (const [i, p] of params.entries()) {
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(p.name)) throw { message: `parameter name \`${p.name}\`: use letters, digits and _, starting with a letter` };
      if (params.slice(0, i).some((q) => q.name === p.name)) throw { message: `there are two parameters called \`${p.name}\`` };
      if (p.value === null) throw { message: `parameter \`${p.name}\` needs a value: a number, text, true/false or a date` };
    }
    this.parameters = params.map((p) => ({ ...p }));
    return this.view();
  }

  async renameParameter(old: string, name: string): Promise<[AnalysisView, string[]]> {
    const p = this.parameters.find((x) => x.name === old);
    if (!p) throw { message: `unknown parameter \`${old}\`` };
    if (this.parameters.some((x) => x.name === name)) throw { message: `there is already a parameter called \`${name}\`` };
    p.name = name;
    const python: string[] = [];
    for (const s of this.steps) {
      if (s.op.type === "python") {
        if (s.op.code.includes(old)) python.push(s.id);
        continue;
      }
      s.op = JSON.parse(
        JSON.stringify(s.op)
          .replace(new RegExp(`([@$])${old}\\b`, "g"), `$1${name}`)
          .replace(`"param":"${old}"`, `"param":"${name}"`),
      ) as StepOp;
    }
    return [await this.view(), python];
  }

  async checkReport(stepId: string): Promise<CheckReport | null> {
    return this.checkReports.get(stepId) ?? null;
  }

  async columnDetails(id: string): Promise<ColumnDetail[]> {
    const t = this.inputOf(id);
    if (!t) throw { message: `\`${id}\` has no table result yet; run the analysis` };
    return t.columns.map((c, i) => {
      const vals = t.rows.map((r) => r[i]!).filter((v) => v.t !== "null");
      const nums = vals.map(num).filter((x): x is number => x !== null).sort((a, b) => a - b);
      const sorted = [...vals].sort((a, b) => compare(a, b) ?? 0);
      const counts = new Map<string, number>();
      for (const v of vals) counts.set(String((v as { v: unknown }).v), (counts.get(String((v as { v: unknown }).v)) ?? 0) + 1);
      const isNum = c.type === "BIGINT" || c.type === "DOUBLE";
      const q = (p: number) => (nums.length ? nums[Math.floor((nums.length - 1) * p)]! : null);
      const mean = nums.length ? nums.reduce((a, b) => a + b, 0) / nums.length : null;
      const lo = nums[0] ?? 0;
      const hi = nums[nums.length - 1] ?? 0;
      const bars = Array.from({ length: 10 }, () => 0);
      if (isNum) for (const x of nums) bars[hi > lo ? Math.min(9, Math.floor(((x - lo) / (hi - lo)) * 10)) : 0]!++;
      return {
        name: c.name,
        type: c.type,
        rows: t.rows.length,
        missing: t.rows.length - vals.length,
        distinct: counts.size,
        min: sorted[0] ? String((sorted[0] as { v: unknown }).v) : null,
        max: sorted.length ? String((sorted[sorted.length - 1] as { v: unknown }).v) : null,
        mean: isNum ? mean : null,
        stddev: null,
        q1: isNum ? q(0.25) : null,
        median: isNum ? q(0.5) : null,
        q3: isNum ? q(0.75) : null,
        zeros: isNum ? nums.filter((x) => x === 0).length : null,
        negatives: isNum ? nums.filter((x) => x < 0).length : null,
        blank: c.type === "VARCHAR" ? vals.filter((v) => String((v as { v: unknown }).v).trim() === "").length : null,
        min_length: null,
        max_length: null,
        top: [...counts.entries()].sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).slice(0, 5).map(([value, count]) => ({ value, count })),
        histogram: isNum ? bars : [],
      };
    });
  }

  async exportNotebook(path: string): Promise<void> {
    this.documents.push([path, JSON.stringify({ nbformat: 4, cells: [] })]);
  }

  async saveReport(path: string, html: string): Promise<void> {
    if (!path.endsWith(".html")) throw { message: "a report is saved as an .html file" };
    this.documents.push([path, html]);
  }

  async pickDocumentPath(defaultName: string): Promise<string | null> {
    return `/exports/${defaultName}`;
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
    const ext = /\.(sqlite|db|duckdb|xlsx)$/i.exec(csv)?.[0] ?? ".csv";
    const db = /\.(sqlite|db|duckdb)$/i.test(ext);
    this.sources.push({ id, path: `${id === base ? stem : id}${ext}`, ...(db ? { sheet: "regions" } : {}), ...regionsTable() });
    return this.view();
  }

  async sourceSheets(id: string): Promise<string[]> {
    const s = this.sourceById(id);
    if (/\.(sqlite|db|duckdb)$/.test(s.path)) return ["regions", "targets"];
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
          const fixed = p.param ? this.parameters.find((x) => x.name === p.param)?.value ?? null : p.value;
          const c = compare(v, literal(fixed));
          if (c === null) return false;
          return { "=": c === 0, "<>": c !== 0, "<": c < 0, "<=": c <= 0, ">": c > 0, ">=": c >= 0 }[p.op];
        }),
      );
      return { def: this.def(s.id), columns: input.columns, rows };
    }
    if (op.type === "test") {
      // A stand-in: real statistics are computed in Rust (stat_test.rs).
      const ai = col(op.a);
      const bi = col(op.b);
      const pairs = input.rows.filter((r) => r[ai]!.t !== "null" && r[bi]!.t !== "null");
      const f = (c: Cell) => num(c) ?? 0;
      const T = (n: string, t: Column["type"]): Column => ({ name: n, type: t });
      if (op.test === "correlation") {
        const xs = pairs.map((r) => f(r[ai]!));
        const ys = pairs.map((r) => f(r[bi]!));
        const mx = xs.reduce((a, b) => a + b, 0) / xs.length;
        const my = ys.reduce((a, b) => a + b, 0) / ys.length;
        const sxy = xs.reduce((a, x, i) => a + (x - mx) * (ys[i]! - my), 0);
        const sxx = xs.reduce((a, x) => a + (x - mx) ** 2, 0);
        const syy = ys.reduce((a, y) => a + (y - my) ** 2, 0);
        const r = sxy / Math.sqrt(sxx * syy);
        return {
          def: this.def(s.id),
          columns: [T("n", "BIGINT"), T("r", "DOUBLE"), T("t", "DOUBLE"), T("df", "BIGINT"), T("p_value", "DOUBLE"), T("reading", "VARCHAR")],
          rows: [[{ t: "int", v: String(xs.length) }, { t: "float", v: r }, { t: "float", v: 0 }, { t: "int", v: String(xs.length - 2) }, { t: "float", v: 0.5 }, { t: "text", v: `(mock) r = ${r.toFixed(2)}` }]],
        };
      }
      const groups = [...new Set(pairs.map((r) => String((r[bi] as { v: unknown }).v)))].sort();
      if (op.test === "compare_means" && groups.length !== 2)
        throw new Error(`comparing two averages needs exactly two groups, and \`${op.b}\` has ${groups.length} (${groups.slice(0, 6).join(", ")})`);
      return {
        def: this.def(s.id),
        columns: [T("groups", "BIGINT"), T("n", "BIGINT"), T("p_value", "DOUBLE"), T("reading", "VARCHAR")],
        rows: [[{ t: "int", v: String(groups.length) }, { t: "int", v: String(pairs.length) }, { t: "float", v: 0.04 }, { t: "text", v: `(mock) ${op.test} of ${op.a} by ${op.b}` }]],
      };
    }
    if (op.type === "check") {
      // A stand-in: formulas are not evaluated (they pass); keys and row counts are.
      const results = op.rules.map((r, i) => {
        if (r.rule === "rows") {
          const n = input.rows.length;
          const ok = (r.min === null || n >= r.min) && (r.max === null || n <= r.max);
          return { rule: i, passed: ok, failing: ok ? 0 : 1, message: `${n} rows`, columns: [], examples: [] };
        }
        if (r.rule === "unique") {
          const idx = r.columns.map(col);
          const seen = new Map<string, number>();
          for (const row of input.rows) {
            const k = JSON.stringify(idx.map((j) => row[j]));
            seen.set(k, (seen.get(k) ?? 0) + 1);
          }
          const rep = [...seen.entries()].filter(([, n]) => n > 1);
          return {
            rule: i,
            passed: rep.length === 0,
            failing: rep.length,
            message: rep.length ? `one row per ${r.columns.join(", ")}: ${rep.length} values appear more than once` : `one row per ${r.columns.join(", ")}: no repeats`,
            columns: [...r.columns, "rows"],
            examples: rep.slice(0, 5).map(([k, n]) => [...(JSON.parse(k) as Cell[]).map((c) => (c.t === "null" ? "missing" : String(c.v))), String(n)]),
          };
        }
        return { rule: i, passed: true, failing: 0, message: `\`${r.formula}\` holds for every row`, columns: [], examples: [] };
      });
      const report = { rows: input.rows.length, results };
      this.checkReports.set(s.id, report);
      const broken = results.filter((r) => !r.passed);
      if (op.stop && broken.length) throw new Error(`${broken.length} of ${results.length} checks failed: ${broken.map((r) => r.message).join("; ")}`);
      return { def: this.def(s.id), columns: input.columns, rows: input.rows };
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
    this.runs.unshift({ ...summary, started_unix_ms: Date.now(), trigger: null });
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
    // As a project that was just opened: nothing to undo yet.
    this.undoStack = [];
    this.redoStack = [];
    this.versions = [];
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
