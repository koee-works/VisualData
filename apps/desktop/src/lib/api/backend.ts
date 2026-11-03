// One interface for the UI; implemented by Tauri IPC in the app and by an
// in-memory mock in tests and browser previews.

import type { AnalysisView, NotebookCell, Page, ProfileRow, RunSummary, StepEvent, StepOp } from "./types";

export interface RunHandlers {
  onStep: (e: StepEvent) => void;
}

export interface Backend {
  createProject(dir: string, csv: string, name: string): Promise<AnalysisView>;
  openProject(dir: string): Promise<AnalysisView>;
  view(): Promise<AnalysisView>;
  sourceProfile(): Promise<ProfileRow[]>;
  sourcePage(offset: number, limit: number): Promise<Page>;
  page(stepId: string, offset: number, limit: number): Promise<Page>;
  chart(stepId: string): Promise<unknown>;
  addStep(input: string, label: string | null, op: StepOp): Promise<AnalysisView>;
  updateStep(id: string, label: string | null, op: StepOp): Promise<AnalysisView>;
  removeStep(id: string): Promise<AnalysisView>;
  setCells(cells: NotebookCell[]): Promise<AnalysisView>;
  rename(name: string): Promise<AnalysisView>;
  /** Approve the current code of these steps so they can run. */
  approve(stepIds: string[]): Promise<AnalysisView>;
  /** Resolves when the run ends; progress arrives through `onStep`. */
  run(handlers: RunHandlers): Promise<RunSummary>;
  cancel(): Promise<void>;
  /** Project folder passed on the command line, if any. */
  startupProject(): Promise<string | null>;
  pickCsv(): Promise<string | null>;
  pickFolder(): Promise<string | null>;
}

/** Errors from the backend carry a user-facing message. */
export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
