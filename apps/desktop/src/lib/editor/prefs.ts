// The parts of the code editor needed before CodeMirror itself loads.

export type Keys = "standard" | "vim";

export interface RunRequest {
  /** What to run: the selection or the current line (Ctrl+Enter, as in
   *  RStudio), all the code (Ctrl+Shift+Enter), or save and run the step
   *  itself (Shift+Enter, as a Jupyter cell). */
  what: "selection" | "all" | "step";
  code: string;
  /** Line in the editor where `code` starts, to place errors. */
  fromLine: number;
}

export function loadKeys(): Keys {
  try {
    return localStorage.getItem("vda.editorKeys") === "vim" ? "vim" : "standard";
  } catch {
    return "standard";
  }
}

export function saveKeys(k: Keys) {
  try {
    localStorage.setItem("vda.editorKeys", k);
  } catch {
    // A preference only; the editor works without it.
  }
}
