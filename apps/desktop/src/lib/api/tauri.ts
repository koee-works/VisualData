import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type { Backend, RunHandlers } from "./backend";
import type { AnalysisView, NotebookCell, Page, ProfileRow, RunSummary, StepEvent, StepOp } from "./types";

/** Commands registered in apps/desktop/src-tauri/src/main.rs. */
export const tauriBackend: Backend = {
  createProject: (dir, csv, name) => invoke<AnalysisView>("create_project", { dir, csv, name }),
  openProject: (dir) => invoke<AnalysisView>("open_project", { dir }),
  view: () => invoke<AnalysisView>("view"),
  sourceProfile: () => invoke<ProfileRow[]>("source_profile"),
  sourcePage: (offset, limit) => invoke<Page>("source_page", { offset, limit }),
  page: (stepId, offset, limit) => invoke<Page>("page", { stepId, offset, limit }),
  chart: (stepId) => invoke<unknown>("chart", { stepId }),
  addStep: (input, label, op: StepOp) => invoke<AnalysisView>("add_step", { input, label, op }),
  updateStep: (id, label, op: StepOp) => invoke<AnalysisView>("update_step", { id, label, op }),
  removeStep: (id) => invoke<AnalysisView>("remove_step", { id }),
  setCells: (cells: NotebookCell[]) => invoke<AnalysisView>("set_cells", { cells }),
  rename: (name) => invoke<AnalysisView>("rename", { name }),
  approve: (stepIds: string[]) => invoke<AnalysisView>("approve", { stepIds }),

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
    const p = await open({ multiple: false, directory: false, filters: [{ name: "CSV", extensions: ["csv"] }] });
    return typeof p === "string" ? p : null;
  },
  async pickFolder() {
    const p = await open({ multiple: false, directory: true });
    return typeof p === "string" ? p : null;
  },
};
