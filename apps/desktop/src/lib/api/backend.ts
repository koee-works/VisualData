// One interface for the UI; implemented by Tauri IPC in the app and by an
// in-memory mock in tests and browser previews.

import type {
  AnalysisView,
  NotebookCell,
  Page,
  Problem,
  Problems,
  ProfileRow,
  RunSummary,
  RunView,
  StepEvent,
  StepOp,
  CheckReport,
  ColumnDetail,
  Parameter, Comment, Every, PythonView, ScheduleView, VersionView } from "./types";

export interface RunHandlers {
  onStep: (e: StepEvent) => void;
}

export interface Backend {
  createProject(dir: string, csv: string, name: string): Promise<AnalysisView>;
  openProject(dir: string): Promise<AnalysisView>;
  view(): Promise<AnalysisView>;
  sourceProfile(sourceId: string): Promise<ProfileRow[]>;
  sourcePage(sourceId: string, offset: number, limit: number): Promise<Page>;
  /** Copy a CSV into the project as another source. */
  addSource(csv: string): Promise<AnalysisView>;
  removeSource(id: string): Promise<AnalysisView>;
  /** Sheet names of a spreadsheet source (empty for other formats). */
  sourceSheets(id: string): Promise<string[]>;
  /** Rename a source and/or choose another sheet. */
  updateSource(id: string, newId: string | null, sheet: string | null): Promise<AnalysisView>;
  page(stepId: string, offset: number, limit: number): Promise<Page>;
  chart(stepId: string): Promise<unknown>;
  /** `inputs[0]` is the main input. */
  addStep(inputs: string[], label: string | null, op: StepOp): Promise<AnalysisView>;
  /** `inputs` replaces the step's inputs when given. */
  updateStep(id: string, label: string | null, op: StepOp, inputs?: string[]): Promise<AnalysisView>;
  removeStep(id: string): Promise<AnalysisView>;
  setCells(cells: NotebookCell[]): Promise<AnalysisView>;
  rename(name: string): Promise<AnalysisView>;
  /** Approve the current code of these steps so they can run. */
  approve(stepIds: string[]): Promise<AnalysisView>;
  /** Run all Python without review until the app quits, or stop. */
  setTrustAll(on: boolean): Promise<AnalysisView | null>;
  /** Resolves when the run ends; progress arrives through `onStep`. */
  run(handlers: RunHandlers): Promise<RunSummary>;
  cancel(): Promise<void>;
  /** Project folder passed on the command line, if any. */
  startupProject(): Promise<string | null>;
  pickCsv(): Promise<string | null>;
  pickFolder(): Promise<string | null>;
  /** Ask where to save a file; `null` if cancelled. */
  pickSavePath(defaultName: string): Promise<string | null>;
  /** Everything the linter finds, errors first. */
  problems(): Promise<Problems>;
  /** Check a step being edited without saving it (`id` null for a new step). */
  checkDraft(id: string | null, inputs: string[], op: StepOp): Promise<Problem[]>;
  dismiss(step: string | null, code: string, dismissed: boolean): Promise<Problems>;
  restoreDismissed(): Promise<Problems>;
  /** Latest runs first. */
  runLog(limit: number): Promise<RunView[]>;
  /** Export a source or a current step result; the format follows the extension. */
  exportResult(id: string, path: string): Promise<number>;
  /** Replace the parameters (added, removed, new values). */
  setParameters(params: Parameter[]): Promise<AnalysisView>;
  /** Rename a parameter where formulas, SQL and filters use it; returns Python steps still naming it. */
  renameParameter(old: string, name: string): Promise<[AnalysisView, string[]]>;
  /** The last report of a check step. */
  checkReport(stepId: string): Promise<CheckReport | null>;
  /** Columns of a source or step result with averages, spread, common values. */
  columnDetails(id: string): Promise<ColumnDetail[]>;
  /** Write a Jupyter notebook that reproduces the analysis. */
  exportNotebook(path: string): Promise<void>;
  /** Write an HTML report of the notebook. */
  saveReport(path: string, html: string): Promise<void>;
  /** Ask where to save a report or notebook. */
  pickDocumentPath(defaultName: string, kind: "html" | "ipynb"): Promise<string | null>;
  /** What a database source reads: a table, or a query (which wins). */
  setSourceRead(id: string, table: string | null, query: string | null): Promise<AnalysisView>;
  undo(): Promise<AnalysisView>;
  redo(): Promise<AnalysisView>;
  /** Saved versions, newest first. */
  history(limit: number): Promise<VersionView[]>;
  restoreVersion(version: number): Promise<AnalysisView>;
  comments(): Promise<Comment[]>;
  addComment(target: string, text: string, replyTo: string | null): Promise<Comment[]>;
  resolveComment(id: string, resolved: boolean): Promise<Comment[]>;
  deleteComment(id: string): Promise<Comment[]>;
  pythonEnvironment(): Promise<PythonView>;
  setPython(path: string): Promise<PythonView>;
  /** The scheduler entry that refreshes the project with vda-run. */
  schedule(every: Every, hour: number, minute: number, exports: [string, string][]): Promise<ScheduleView>;
}

/** Errors from the backend carry a user-facing message. */
export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
