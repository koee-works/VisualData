// Workspace state shared by the components (Svelte 5 runes).
// The backend is the source of truth; every edit returns a fresh view.

import { prefs } from "./prefs.svelte";
import { ALL_PANES, loadPanes, savePanes, toggled, type PaneName, type Panes } from "./layout";
import { errorMessage, type Backend } from "./api/backend";
import type { AnalysisView, Column, Comment, NotebookCell, Problem, Problems, SourceView, StepEvent, StepOp, StepView, Parameter } from "./api/types";
import { SOURCE_ID } from "./api/types";
import { stepTitle } from "./format";

export type CenterTab = "step" | "notebook" | "settings";

export class Workspace {
  view = $state<AnalysisView | null>(null);
  /** Selected node in the pipeline: a step id or "source". */
  selected = $state<string>(SOURCE_ID);
  centerTab = $state<CenterTab>("step");
  /** The Settings tab is open (beside Step and Notebook), and which section to show. */
  settingsOpen = $state(false);
  settingsSection = $state<"general" | "appearance" | "editor" | "environments" | null>(null);

  openSettings(section: "general" | "appearance" | "editor" | "environments" | null = null) {
    this.settingsOpen = true;
    this.settingsSection = section;
    this.centerTab = "settings";
  }

  closeSettings() {
    this.settingsOpen = false;
    if (this.centerTab === "settings") this.centerTab = "step";
  }
  running = $state(false);
  /** Progress of the current or last run, by step id. */
  progress = $state<Record<string, StepEvent>>({});
  /** Last error to show, with what the user was doing. */
  error = $state<{ action: string; message: string } | null>(null);
  /** Bumped after every change, so result views refetch. */
  revision = $state(0);
  /** Whether the code review dialog is open. */
  reviewOpen = $state(false);
  /** A short confirmation to show (an export finished, say). */
  #notice = $state<string | null>(null);
  #noticeTimer: ReturnType<typeof setTimeout> | null = null;
  /** A passing message: it hides by itself after long enough to read it
   *  (errors stay until dismissed). */
  get notice(): string | null {
    return this.#notice;
  }
  set notice(text: string | null) {
    this.#notice = text;
    if (this.#noticeTimer) clearTimeout(this.#noticeTimer);
    this.#noticeTimer = null;
    if (text && prefs.value.autoHideMessages) {
      const ms = Math.min(15000, 5000 + text.length * 60);
      this.#noticeTimer = setTimeout(() => {
        if (this.#notice === text) this.#notice = null;
      }, ms);
    }
  }
  /** What the linter found, refreshed after every change and run. */
  problems = $state<Problems>({ items: [], dismissed: 0 });
  /** Panels shown, chosen with the status bar's dock buttons (remembered). */
  panes = $state<Panes>(loadPanes());
  /** The Activity dock is open (kept equal to `panes.activity`). */
  consoleOpen = $state(this.panes.activity);
  /** Kinds of step and how the pipeline adds one (registered by the pipeline). */
  stepKinds: { type: StepOp["type"]; name: string; needsTwo: boolean }[] = [];
  addStepKind: ((type: StepOp["type"]) => boolean) | null = null;
  /** The command palette is open. */
  paletteOpen = $state(false);
  /** The code console in the right dock, and the editor's "show this line". */
  codeConsole: { run(code: string, fromLine: number | null): Promise<void>; compare(): Promise<void> } | null = null;
  focusCodeLine: ((line: number) => void) | null = null;

  togglePane(name: PaneName) {
    this.panes = toggled(this.panes, name);
    if (name === "activity") this.consoleOpen = this.panes.activity;
    savePanes($state.snapshot(this.panes));
  }

  /** Show everything again. */
  showAllPanes() {
    this.panes = { ...ALL_PANES };
    savePanes($state.snapshot(this.panes));
  }
  consoleTab = $state<"problems" | "output" | "log" | "history">("problems");
  /** Comments on steps, sources and the analysis (kept with the project). */
  comments = $state<Comment[]>([]);
  pythonOpen = $state(false);
  rOpen = $state(false);
  /** Code steps open as files in other editors: step id → file. */
  external = $state<Record<string, string>>({});
  private syncTimer: ReturnType<typeof setInterval> | null = null;

  /** Open a code step in Jupyter or RStudio (the computer's app for the file). */
  async editExternally(stepId: string) {
    const x = await this.act("Open in another editor", () => this.backend.editExternally(stepId));
    if (!x) return;
    this.external[stepId] = x.path;
    this.notice = `Opened ${x.path.split(/[\\/]/).pop()} (${x.kind}). Saving it there updates this step; its first ${x.kind === "notebook" ? "cell loads" : "line loads"} the step's inputs.`;
    if (!this.syncTimer) this.syncTimer = setInterval(() => void this.syncExternal(), 1500);
  }

  async syncExternal() {
    if (this.running) return;
    try {
      const s = await this.backend.syncExternal();
      if (s.view) {
        this.apply(s.view);
        this.notice = `Updated ${s.updated.map((id) => this.nameOf(id)).join(", ")} from the other editor. Run the analysis to see its result.`;
      }
    } catch {
      // Checked again shortly; a half-saved file is read next time.
    }
  }
  scheduleOpen = $state(false);
  /** Bumped when a run finishes, so the run log refetches. */
  runs = $state(0);
  private lintSeq = 0;

  constructor(readonly backend: Backend) {}

  /** Python steps waiting for approval. */
  pendingReview = $derived(this.view?.review ?? []);

  selectedStep: StepView | null = $derived(this.view?.steps.find((s) => s.id === this.selected) ?? null);
  selectedSource: SourceView | null = $derived(this.view?.sources.find((s) => s.id === this.selected) ?? null);

  isSource(id: string): boolean {
    return this.view?.sources.some((s) => s.id === id) ?? false;
  }

  /** What to call a source or step in the interface. */
  nameOf(id: string): string {
    const src = this.view?.sources.find((s) => s.id === id);
    if (src) return src.path;
    const step = this.view?.steps.find((s) => s.id === id);
    return step ? stepTitle(step) : id;
  }

  /** Columns of a source or step result, when known. */
  columnsOf(id: string): Column[] | null {
    const src = this.view?.sources.find((s) => s.id === id);
    if (src) return src.columns;
    return this.view?.steps.find((s) => s.id === id)?.columns ?? null;
  }

  /** Sources and table steps that `id` could read without creating a cycle. */
  readable(id: string | null): { id: string; name: string }[] {
    const v = this.view;
    if (!v) return [];
    const downstream = new Set<string>(id ? [id] : []);
    let grew = true;
    while (grew) {
      grew = false;
      for (const s of v.steps)
        if (!downstream.has(s.id) && s.inputs.some((i) => downstream.has(i))) {
          downstream.add(s.id);
          grew = true;
        }
    }
    return [
      ...v.sources.map((s) => ({ id: s.id, name: s.path })),
      ...this.ordered.filter((s) => s.mode !== "chart" && !downstream.has(s.id)).map((s) => ({ id: s.id, name: stepTitle(s) })),
    ];
  }

  private async act<T>(action: string, f: () => Promise<T>): Promise<T | undefined> {
    try {
      const r = await f();
      this.error = null;
      return r;
    } catch (e) {
      this.error = { action, message: errorMessage(e) };
      return undefined;
    }
  }

  /** Re-run the linter; only the latest answer is kept. */
  async refreshProblems() {
    const seq = ++this.lintSeq;
    try {
      const p = await this.backend.problems();
      if (seq === this.lintSeq) this.problems = p;
    } catch {
      // A failing check must not get in the way of editing.
    }
  }

  problemsFor(id: string): Problem[] {
    return this.problems.items.filter((p) => p.step === id);
  }

  counts = $derived.by(() => {
    const c = { error: 0, warning: 0, info: 0 };
    for (const p of this.problems.items) c[p.severity]++;
    return c;
  });

  async dismiss(p: Problem, dismissed = true) {
    const r = await this.act("Dismiss", () => this.backend.dismiss(p.step, p.code, dismissed));
    if (r) this.problems = r;
  }

  async restoreDismissed() {
    const r = await this.act("Show dismissed", () => this.backend.restoreDismissed());
    if (r) this.problems = r;
  }

  /** Select the step a problem is about and show its editor. */
  reveal(p: Problem) {
    if (!p.step) return;
    this.selected = p.step;
    this.centerTab = "step";
  }

  private apply(v: AnalysisView | undefined) {
    if (!v) return;
    const reloadComments = this.view?.project_dir !== v.project_dir;
    this.view = v;
    if (reloadComments) void this.loadComments();
    void this.refreshProblems();
    const known = v.sources.some((s) => s.id === this.selected) || v.steps.some((s) => s.id === this.selected);
    if (!known) this.selected = v.sources[0]?.id ?? SOURCE_ID;
    this.revision++;
  }

  async open(dir: string) {
    this.apply(await this.act("Open project", () => this.backend.openProject(dir)));
    this.selected = this.view?.sources[0]?.id ?? SOURCE_ID;
    this.progress = {};
    // Code someone else wrote is shown before anything can run it.
    this.reviewOpen = this.pendingReview.length > 0;
  }

  /** Ask where to save, then export a source or a current result. */
  async exportResult(id: string, ext: "csv" | "xlsx" | "parquet") {
    const base = (this.nameOf(id) || id).replace(/\.[^.]+$/, "").replace(/[\\/:*?"<>|]+/g, "-");
    const path = await this.backend.pickSavePath(`${base}.${ext}`);
    if (!path) return;
    const rows = await this.act("Export", () => this.backend.exportResult(id, path));
    if (rows !== undefined) this.notice = `Exported ${rows.toLocaleString("en-US")} row${rows === 1 ? "" : "s"} to ${path}`;
  }

  /** Go back one change (the toolbar's Undo, and Ctrl/Cmd+Z). */
  async undo() {
    const what = this.view?.undo;
    const v = await this.act("Undo", () => this.backend.undo());
    this.apply(v);
    if (v && what) this.notice = `Undid: ${what}`;
  }

  async redo() {
    const what = this.view?.redo;
    const v = await this.act("Redo", () => this.backend.redo());
    this.apply(v);
    if (v && what) this.notice = `Redid: ${what}`;
  }

  /** Make a saved version current again; undo goes back. */
  async restoreVersion(version: number) {
    const v = await this.act("Restore version", () => this.backend.restoreVersion(version));
    this.apply(v);
    if (v) this.notice = `Restored version ${version}. Undo goes back.`;
    return v !== undefined;
  }

  async loadComments() {
    try {
      this.comments = await this.backend.comments();
    } catch {
      this.comments = [];
    }
  }

  /** Open comments (not resolved) on `target`, without replies. */
  openComments(target: string): number {
    return this.comments.filter((c) => c.target === target && !c.reply_to && !c.resolved).length;
  }

  async addComment(target: string, text: string, replyTo: string | null = null) {
    const r = await this.act("Comment", () => this.backend.addComment(target, text, replyTo));
    if (r) this.comments = r;
    return r !== undefined;
  }

  async resolveComment(id: string, resolved: boolean) {
    const r = await this.act(resolved ? "Resolve comment" : "Reopen comment", () => this.backend.resolveComment(id, resolved));
    if (r) this.comments = r;
  }

  async deleteComment(id: string) {
    const r = await this.act("Delete comment", () => this.backend.deleteComment(id));
    if (r) this.comments = r;
  }

  /** What a database source reads. */
  async setSourceRead(id: string, table: string | null, query: string | null) {
    const v = await this.act(query ? "Read with a query" : "Choose table", () => this.backend.setSourceRead(id, table, query));
    this.apply(v);
    return v !== undefined;
  }

  /** Replace the parameters; steps using a changed one become outdated. */
  async setParameters(params: Parameter[]) {
    const v = await this.act("Save parameters", () => this.backend.setParameters(params));
    this.apply(v);
    return v !== undefined;
  }

  async renameParameter(old: string, name: string) {
    const r = await this.act("Rename parameter", () => this.backend.renameParameter(old, name));
    if (!r) return false;
    this.apply(r[0]);
    if (r[1].length)
      this.notice = `Renamed @${old} to @${name}. Python code is not changed: edit params["${old}"] in ${r[1].map((id) => this.nameOf(id)).join(", ")}.`;
    return true;
  }

  /** Save the notebook as an HTML report or a Jupyter notebook. */
  async exportNotebook(kind: "html" | "ipynb") {
    const base = (this.view?.name || "notebook").replace(/[\\/:*?"<>|]+/g, "-");
    const path = await this.backend.pickDocumentPath(`${base}.${kind}`, kind);
    if (!path) return;
    const ok = await this.act(kind === "html" ? "Save report" : "Export notebook", async () => {
      if (kind === "html") {
        const { buildReport } = await import("./report");
        await this.backend.saveReport(path, await buildReport(this));
      } else await this.backend.exportNotebook(path);
      return true;
    });
    if (ok) this.notice = kind === "html" ? `Saved the report to ${path}` : `Saved a Jupyter notebook that reproduces the analysis to ${path}`;
  }

  /** "Trust all in this session": run all Python without review until the app quits. */
  async setTrustAll(on: boolean) {
    const v = await this.act(on ? "Trust all Python" : "Ask before running Python", () => this.backend.setTrustAll(on));
    if (v) this.apply(v);
    if (on) this.reviewOpen = false;
    else this.reviewOpen = this.pendingReview.length > 0;
  }

  /** Approve the shown code of these steps; closes the dialog when none are left. */
  async approve(stepIds: string[]) {
    this.apply(await this.act("Approve code", () => this.backend.approve(stepIds)));
    if (!this.pendingReview.length) this.reviewOpen = false;
  }

  async create(dir: string, csv: string, name: string) {
    this.apply(await this.act("Create project", () => this.backend.createProject(dir, csv, name)));
    this.selected = this.view?.sources[0]?.id ?? SOURCE_ID;
  }

  async refresh() {
    this.apply(await this.act("Refresh", () => this.backend.view()));
  }

  /** Add a step reading the selected node (and `more` inputs), then select it. */
  async addStep(op: StepOp, label: string | null = null, more: string[] = []) {
    const inputs = [this.selected, ...more];
    const before = new Set(this.view?.steps.map((s) => s.id));
    const v = await this.act("Add step", () => this.backend.addStep(inputs, label, op));
    this.apply(v);
    const added = v?.steps.find((s) => !before.has(s.id));
    if (added) {
      this.selected = added.id;
      this.centerTab = "step";
    }
    return added;
  }

  async saveStep(id: string, label: string | null, op: StepOp, inputs?: string[]) {
    const v = await this.act("Save step", () => this.backend.updateStep(id, label, op, inputs));
    this.apply(v);
    return v !== undefined;
  }

  async removeStep(id: string) {
    const step = this.view?.steps.find((s) => s.id === id);
    const v = await this.act("Delete step", () => this.backend.removeStep(id));
    this.apply(v);
    if (v && step) this.selected = step.inputs[0]!;
  }

  /** Ask for a CSV file and add it to the project as another source. */
  async addSource() {
    const csv = await this.backend.pickCsv();
    if (!csv) return;
    const before = new Set(this.view?.sources.map((s) => s.id));
    const v = await this.act("Add source", () => this.backend.addSource(csv));
    this.apply(v);
    const added = v?.sources.find((s) => !before.has(s.id));
    if (added) {
      this.selected = added.id;
      this.centerTab = "step";
    }
  }

  /** Rename a source and/or pick another sheet; selects it under its new name. */
  async updateSource(id: string, newId: string | null, sheet: string | null) {
    const v = await this.act(newId ? "Rename source" : "Choose sheet", () => this.backend.updateSource(id, newId, sheet));
    if (v && newId && this.selected === id) this.selected = newId;
    this.apply(v);
    return v !== undefined;
  }

  async removeSource(id: string) {
    this.apply(await this.act("Remove source", () => this.backend.removeSource(id)));
  }

  async setCells(cells: NotebookCell[]) {
    this.apply(await this.act("Update notebook", () => this.backend.setCells(cells)));
  }

  async rename(name: string) {
    this.apply(await this.act("Rename", () => this.backend.rename(name)));
  }

  async run() {
    if (this.running || !this.view) return;
    this.running = true;
    this.progress = {};
    await this.act("Run analysis", () =>
      this.backend.run({
        onStep: (e) => {
          this.progress = { ...this.progress, [e.step_id]: e };
        },
      }),
    );
    this.running = false;
    this.runs++;
    await this.refresh();
  }

  async cancel() {
    await this.act("Cancel run", () => this.backend.cancel());
  }

  /**
   * The pipeline as trees, one per source: each step sits under its main
   * input; steps reading more inputs say so on their row.
   */
  tree = $derived.by(() => {
    const steps = this.view?.steps ?? [];
    const out: { source: SourceView; steps: { step: StepView; depth: number }[] }[] = [];
    for (const source of this.view?.sources ?? []) {
      const rows: { step: StepView; depth: number }[] = [];
      const visit = (parent: string, depth: number) => {
        for (const s of steps.filter((x) => x.inputs[0] === parent)) {
          rows.push({ step: s, depth });
          visit(s.id, depth + 1);
        }
      };
      visit(source.id, 1);
      out.push({ source, steps: rows });
    }
    return out;
  });

  /** Depth of each step in the pipeline tree, for indentation. */
  depth = $derived.by(() => {
    const d: Record<string, number> = {};
    for (const t of this.tree) for (const r of t.steps) d[r.step.id] = r.depth;
    return d;
  });

  /** Steps in pipeline order: each source's tree in turn, depth first. */
  ordered = $derived.by(() => this.tree.flatMap((t) => t.steps.map((r) => r.step)));
}
