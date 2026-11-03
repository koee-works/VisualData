// Workspace state shared by the components (Svelte 5 runes).
// The backend is the source of truth; every edit returns a fresh view.

import { errorMessage, type Backend } from "./api/backend";
import type { AnalysisView, NotebookCell, StepEvent, StepOp, StepView } from "./api/types";
import { SOURCE_ID } from "./api/types";

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
    if (this.selected !== SOURCE_ID && !v.steps.some((s) => s.id === this.selected)) this.selected = SOURCE_ID;
    this.revision++;
  }

  async open(dir: string) {
    this.apply(await this.act("Open project", () => this.backend.openProject(dir)));
    this.selected = SOURCE_ID;
    this.progress = {};
    // Code someone else wrote is shown before anything can run it.
    this.reviewOpen = this.pendingReview.length > 0;
  }

  /** Approve the shown code of these steps; closes the dialog when none are left. */
  async approve(stepIds: string[]) {
    this.apply(await this.act("Approve code", () => this.backend.approve(stepIds)));
    if (!this.pendingReview.length) this.reviewOpen = false;
  }

  async create(dir: string, csv: string, name: string) {
    this.apply(await this.act("Create project", () => this.backend.createProject(dir, csv, name)));
    this.selected = SOURCE_ID;
  }

  async refresh() {
    this.apply(await this.act("Refresh", () => this.backend.view()));
  }

  /** Add a step reading the selected node, then select it. */
  async addStep(op: StepOp, label: string | null = null) {
    const input = this.selected;
    const before = new Set(this.view?.steps.map((s) => s.id));
    const v = await this.act("Add step", () => this.backend.addStep(input, label, op));
    this.apply(v);
    const added = v?.steps.find((s) => !before.has(s.id));
    if (added) {
      this.selected = added.id;
      this.centerTab = "step";
    }
    return added;
  }

  async saveStep(id: string, label: string | null, op: StepOp) {
    const v = await this.act("Save step", () => this.backend.updateStep(id, label, op));
    this.apply(v);
    return v !== undefined;
  }

  async removeStep(id: string) {
    const step = this.view?.steps.find((s) => s.id === id);
    const v = await this.act("Delete step", () => this.backend.removeStep(id));
    this.apply(v);
    if (v && step) this.selected = step.input;
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

  /** Depth of each step in the pipeline tree, for indentation. */
  depth = $derived.by(() => {
    const d: Record<string, number> = { [SOURCE_ID]: 0 };
    for (const s of this.view?.steps ?? []) d[s.id] = (d[s.input] ?? 0) + 1;
    return d;
  });

  /** Steps in pipeline order: each step after its input, siblings in creation order. */
  ordered = $derived.by(() => {
    const steps = this.view?.steps ?? [];
    const out: StepView[] = [];
    const visit = (parent: string) => {
      for (const s of steps.filter((x) => x.input === parent)) {
        out.push(s);
        visit(s.id);
      }
    };
    visit(SOURCE_ID);
    return out;
  });
}
