// How the window is shared out: the step editor and its result, and the
// pipeline sidebar. Choices the user makes by dragging are remembered per
// kind of step in this browser; until then the result takes only the height
// a small result needs.

import type { SourceView, StepView } from "./api/types";

export type PaneKind = "source" | "visual" | "code" | "chart";

export function paneKind(step: StepView | null, source: SourceView | null): PaneKind {
  if (source) return "source";
  if (!step) return "visual";
  if (step.op.type === "chart") return "chart";
  if (step.op.type === "sql" || step.op.type === "python" || step.op.type === "r") return "code";
  return "visual";
}

// Measured from the result panel: tabs, table header, footer, one row.
const TABS = 41;
const HEADER = 44;
const FOOTER = 25;
const ROW = 26;
const BANNER = 38;

/** Height (px) the result needs to show `rows` rows without empty space,
 *  or null when it should simply share the space (charts, many rows,
 *  nothing to show yet). */
export function resultHeight(kind: PaneKind, rows: number | null, outdated: boolean): number | null {
  if (kind === "chart" || kind === "source" || rows === null || rows > 12) return null;
  return TABS + HEADER + FOOTER + Math.max(rows, 2) * ROW + (outdated ? BANNER : 0) + 8;
}

/** Share of the centre column the editor gets when the user has not chosen. */
export function defaultSplit(kind: PaneKind): number {
  return kind === "code" ? 0.62 : kind === "chart" ? 0.45 : 0.5;
}

export const SPLIT_MIN = 0.15;
export const SPLIT_MAX = 0.85;
export const SIDEBAR_MIN = 200;
export const SIDEBAR_MAX = 480;

function read(key: string): number | null {
  try {
    const v = Number(localStorage.getItem(key));
    return Number.isFinite(v) && v > 0 ? v : null;
  } catch {
    return null;
  }
}

function write(key: string, v: number | null) {
  try {
    if (v === null) localStorage.removeItem(key);
    else localStorage.setItem(key, String(v));
  } catch {
    // A convenience only.
  }
}

export const savedSplit = (kind: PaneKind) => read(`vda.split.${kind}`);
export const saveSplit = (kind: PaneKind, v: number | null) => write(`vda.split.${kind}`, v);
export const savedSidebar = () => read("vda.sidebar");
export const saveSidebar = (v: number) => write("vda.sidebar", v);

export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
