import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import type { Backend, RunHandlers } from "./backend";
import { prefs } from "../prefs.svelte";
import type {
  AboutView,
  AnalysisView,
  NotebookCell,
  NotebookLanguage,
  Page,
  Problem,
  Problems,
  ProfileRow,
  RunSummary,
  RunView,
  StepEvent,
  StepOp,
  CheckReport,
  CleanReport,
  Comparison,
  ColumnDetail, Comment, PythonView, RView, ConsoleView, ConsoleReply, ExternalView, SyncView, ScheduleView, VersionView } from "./types";

/** A file name inside the exports folder chosen in Settings, if there is one. */
function inExportsFolder(name: string): string {
  const dir = prefs.value.exportsFolder;
  if (!dir) return name;
  return `${dir.replace(/[\\/]+$/, "")}${dir.includes("\\") && !dir.includes("/") ? "\\" : "/"}${name}`;
}

/** Filters with the chosen export format first, which is the one a save dialog starts on. */
function exportFilters() {
  const all = [
    { id: "csv", name: "CSV", extensions: ["csv"] },
    { id: "xlsx", name: "Excel workbook", extensions: ["xlsx"] },
    { id: "parquet", name: "Parquet", extensions: ["parquet"] },
  ];
  const first = all.find((f) => f.id === prefs.value.exportFormat);
  return (first ? [first, ...all.filter((f) => f !== first)] : all).map(({ name, extensions }) => ({ name, extensions }));
}

/** Commands registered in apps/desktop/src-tauri/src/main.rs. */
export const tauriBackend: Backend = {
  createProject: (dir, csv, name) => invoke<AnalysisView>("create_project", { dir, csv, name }),
  openProject: (dir) => invoke<AnalysisView>("open_project", { dir }),
  view: () => invoke<AnalysisView>("view"),
  sourceProfile: (sourceId) => invoke<ProfileRow[]>("source_profile", { sourceId }),
  sourcePage: (sourceId, offset, limit) => invoke<Page>("source_page", { sourceId, offset, limit }),
  addSource: (csv) => invoke<AnalysisView>("add_source", { csv }),
  removeSource: (id) => invoke<AnalysisView>("remove_source", { id }),
  sourceSheets: (id) => invoke<string[]>("source_sheets", { id }),
  updateSource: (id, newId, sheet) => invoke<AnalysisView>("update_source", { id, newId, sheet }),
  page: (stepId, offset, limit) => invoke<Page>("page", { stepId, offset, limit }),
  chart: (stepId) => invoke<unknown>("chart", { stepId }),
  addStep: (inputs, label, op: StepOp) => invoke<AnalysisView>("add_step", { inputs, label, op }),
  updateStep: (id, label, op: StepOp, inputs?: string[]) =>
    invoke<AnalysisView>("update_step", { id, label, op, inputs: inputs ?? null }),
  removeStep: (id) => invoke<AnalysisView>("remove_step", { id }),
  setCells: (cells: NotebookCell[]) => invoke<AnalysisView>("set_cells", { cells }),
  rename: (name) => invoke<AnalysisView>("rename", { name }),
  approve: (stepIds: string[]) => invoke<AnalysisView>("approve", { stepIds }),
  setTrustAll: (on: boolean) => invoke<AnalysisView | null>("set_trust_all", { on }),
  about: () => invoke<AboutView>("about"),
  forgetApprovals: () => invoke<AnalysisView | null>("forget_approvals"),

  async run({ onStep }: RunHandlers) {
    // Listen before starting so no event is missed; the command itself
    // resolves only when the run has finished on the worker thread.
    const unlisten = await listen<StepEvent>("run://step", (e) => onStep(e.payload));
    try {
      return await invoke<RunSummary>("run");
    } finally {
      unlisten();
    }
  },
  cancel: () => invoke<void>("cancel"),

  startupProject: () => invoke<string | null>("startup_project"),
  async pickCsv() {
    const p = await open({
      multiple: false,
      directory: false,
      filters: [
        {
          name: "Data files",
          extensions: ["csv", "parquet", "json", "jsonl", "ndjson", "xlsx", "xlsm", "xls", "ods", "sqlite", "sqlite3", "db", "db3", "duckdb", "ddb"],
        },
      ],
    });
    return typeof p === "string" ? p : null;
  },
  async pickSavePath(defaultName) {
    const p = await save({
      defaultPath: inExportsFolder(defaultName),
      filters: exportFilters(),
    });
    return typeof p === "string" ? p : null;
  },
  exportResult: (id, path) => invoke<number>("export", { id, path }),
  problems: () => invoke<Problems>("problems"),
  checkDraft: (id, inputs, op) => invoke<Problem[]>("check_draft", { id, inputs, op }),
  dismiss: (step, code, dismissed) => invoke<Problems>("dismiss", { step, code, dismissed }),
  restoreDismissed: () => invoke<Problems>("restore_dismissed"),
  runLog: (limit) => invoke<RunView[]>("run_log", { limit }),
  setParameters: (params) => invoke<AnalysisView>("set_parameters", { params }),
  renameParameter: (old, name) => invoke<[AnalysisView, string[]]>("rename_parameter", { old, new: name }),
  checkReport: (stepId) => invoke<CheckReport | null>("check_report", { stepId }),
  cleanReport: (stepId) => invoke<CleanReport | null>("clean_report", { stepId }),
  compareFills: (stepId, op, column) => invoke<Comparison>("compare_fills", { stepId, op, column }),
  columnDetails: (id) => invoke<ColumnDetail[]>("column_details", { id }),
  exportNotebook: (path, language) => invoke<void>("export_notebook", { path, language }),
  saveReport: (path, html) => invoke<void>("save_report", { path, html }),
  setSourceRead: (id, table, query) => invoke<AnalysisView>("set_source_read", { id, table, query }),
  undo: () => invoke<AnalysisView>("undo"),
  redo: () => invoke<AnalysisView>("redo"),
  history: (limit) => invoke<VersionView[]>("history", { limit }),
  restoreVersion: (version) => invoke<AnalysisView>("restore_version", { version }),
  comments: () => invoke<Comment[]>("comments"),
  addComment: (target, text, replyTo) => invoke<Comment[]>("add_comment", { target, text, replyTo }),
  resolveComment: (id, resolved) => invoke<Comment[]>("resolve_comment", { id, resolved }),
  deleteComment: (id) => invoke<Comment[]>("delete_comment", { id }),
  pythonEnvironment: () => invoke<PythonView>("python_environment"),
  setPython: (path) => invoke<PythonView>("set_python", { path }),
  rEnvironment: () => invoke<RView>("r_environment"),
  preferences: () => invoke<unknown>("preferences"),
  setPreferences: (preferences) => invoke<unknown>("set_preferences", { preferences }),
  editExternally: (stepId) => invoke<ExternalView>("edit_externally", { stepId }),
  syncExternal: () => invoke<SyncView>("sync_external"),
  consoleStart: (stepId) => invoke<ConsoleView>("console_start", { stepId }),
  consoleRun: (code) => invoke<ConsoleReply>("console_run", { code }),
  consoleInterrupt: () => invoke<void>("console_interrupt"),
  consoleStop: () => invoke<void>("console_stop"),
  consoleCompare: (stepId) => invoke<string | null>("console_compare", { stepId }),
  setR: (path) => invoke<RView>("set_r", { path }),
  schedule: (every, hour, minute, exports) => invoke<ScheduleView>("schedule", { every, hour, minute, exports }),
  async pickDocumentPath(defaultName, kind) {
    const preset = await invoke<string | null>("preset_save_path", { name: defaultName });
    if (preset) return preset;
    const p = await save({
      defaultPath: inExportsFolder(defaultName),
      filters: [kind === "html" ? { name: "Web page", extensions: ["html"] } : { name: "Jupyter notebook", extensions: ["ipynb"] }],
    });
    return typeof p === "string" ? p : null;
  },
  async pickFolder(start) {
    const defaultPath = start ?? prefs.value.projectsFolder;
    const p = await open({ multiple: false, directory: true, ...(defaultPath ? { defaultPath } : {}) });
    return typeof p === "string" ? p : null;
  },
};
