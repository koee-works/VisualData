// Workspace state shared by the components (Svelte 5 runes).
// The backend is the source of truth; every edit returns a fresh view.

import { errorMessage, type Backend } from "./api/backend";
import type { AnalysisView, Column, NotebookCell, SourceView, StepEvent, StepOp, StepView } from "./api/types";
import { SOURCE_ID } from "./api/types";
import { stepTitle } from "./format";

export type CenterTab = "step" | "notebook";

export class Workspace {
  view = $state<AnalysisView | null>(null);
  /** Selected node in the pipeline: a step id or "source". */
  selected = $state<string>(SOURCE_ID);
  centerTab = $state<CenterTab>("step");
  running = $state(false);
  /** Progress of the current or last run, by step id. */
  progress = $state<Record<string, StepEvent>>({});
  /** Last error to show, with what the user was doing. */
  error = $state<{ action: string; message: string } | null>(null);
  /** Bumped after every change, so result views refetch. */
  revision = $state(0);
  /** Whether the code review dialog is open. */
  reviewOpen = $state(false);

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

  private apply(v: AnalysisView | undefined) {
    if (!v) return;
    this.view = v;
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
