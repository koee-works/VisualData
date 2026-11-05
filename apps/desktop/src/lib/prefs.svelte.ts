// The user's preferences: kept in their settings file through the backend
// (so they move with the user's settings, not with one browser profile), with
// a local copy so the theme and text size apply before the first paint.

import { apply, buildCustom, resolve, THEMES, type CustomTheme, type Theme, type ThemeChoice } from "./theme";
import { cellDisplay, numberDisplay, type DateFormat, type MissingText } from "./format";

export type TextSize = "small" | "default" | "large" | "larger";
export type RowDensity = "compact" | "normal" | "roomy";
export type ExportFormat = "csv" | "xlsx" | "parquet";

/** Height of a table row, in px, for each density. */
export const ROW_HEIGHTS: Record<RowDensity, number> = { compact: 22, normal: 26, roomy: 32 };

export interface Preferences {
  /** The name comments are signed with. */
  author?: string;
  theme: ThemeChoice | string;
  customThemes: CustomTheme[];
  textSize: TextSize;
  /** Run the analysis each time a step is saved. */
  runAfterSave: boolean;
  /** Passing messages hide by themselves. */
  autoHideMessages: boolean;
  thousandsSeparators: boolean;
  /** Most decimals shown for a column of decimals (the exact value is in the tooltip). */
  maxDecimals: number;
  editorKeys: "standard" | "vim";
  wrapCode: boolean;

  // Data
  rowDensity: RowDensity;
  /** What a missing cell shows. */
  missingText: MissingText;
  dateFormat: DateFormat;
  /** Values a new Clean step treats as missing. */
  missingMarkers: string[];

  // Code editor
  /** Text size in code editors, in px. */
  editorFontSize: number;
  /** Spaces per indent. */
  tabWidth: number;
  lineNumbers: boolean;
  autoCloseBrackets: boolean;
  highlightActiveLine: boolean;

  // Accessibility
  reduceMotion: boolean;

  // Running
  /** Run the analysis when a project opens. */
  runOnOpen: boolean;
  /** Time limit a new Python or R step starts with. */
  stepTimeoutSeconds: number;
  /** Memory limit for Python and R, in MB: absent means each language's own limit, 0 none. */
  codeMemoryMb?: number;

  // Projects
  /** Saved versions kept in a project's history. */
  versionsKept: number;
  exportFormat: ExportFormat;
  reopenLastProject: boolean;
  /** The project open when the app last closed (for reopening it). */
  lastProject?: string;
  /** Where the project and file pickers start; empty means the system's choice. */
  projectsFolder?: string;
  exportsFolder?: string;
}

export const DEFAULTS: Preferences = {
  theme: "system",
  customThemes: [],
  textSize: "default",
  runAfterSave: false,
  autoHideMessages: true,
  thousandsSeparators: true,
  maxDecimals: 4,
  editorKeys: "standard",
  wrapCode: false,
  rowDensity: "normal",
  missingText: "missing",
  dateFormat: "iso",
  missingMarkers: ["N/A", "NA"],
  editorFontSize: 13,
  tabWidth: 2,
  lineNumbers: true,
  autoCloseBrackets: true,
  highlightActiveLine: true,
  reduceMotion: false,
  runOnOpen: false,
  stepTimeoutSeconds: 60,
  versionsKept: 500,
  exportFormat: "csv",
  reopenLastProject: false,
};

export const ROW_DENSITIES: RowDensity[] = ["compact", "normal", "roomy"];
export const MISSING_TEXTS: MissingText[] = ["missing", "dash", "blank", "null"];
export const DATE_FORMATS: DateFormat[] = ["iso", "us", "eu", "long"];
export const EXPORT_FORMATS: ExportFormat[] = ["csv", "xlsx", "parquet"];
export const TAB_WIDTHS = [2, 4, 8];
export const STEP_TIMEOUTS = [10, 30, 60, 120, 300, 600, 1800];
export const MEMORY_LIMITS_MB = [1024, 2048, 4096, 8192, 16384];
export const VERSION_COUNTS = [50, 100, 250, 500, 1000, 2000];

export const TEXT_SIZES: Record<TextSize, { px: number; name: string }> = {
  small: { px: 15, name: "Smaller" },
  default: { px: 16, name: "Default" },
  large: { px: 17.5, name: "Larger" },
  larger: { px: 19, name: "Largest" },
};

const LOCAL = "vda.preferences";

/** Fill gaps with defaults and drop what does not fit, so a hand-edited or
 *  older settings file cannot break the app. */
export function normalize(raw: unknown): Preferences {
  const p = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const pick = <K extends keyof Preferences>(k: K, ok: (v: unknown) => boolean): Preferences[K] => (ok(p[k]) ? (p[k] as Preferences[K]) : DEFAULTS[k]);
  const customs = Array.isArray(p.customThemes) ? (p.customThemes as unknown[]).filter(isCustomTheme) : [];
  return {
    author: typeof p.author === "string" ? p.author : undefined,
    theme: pick("theme", (v) => typeof v === "string"),
    customThemes: customs,
    textSize: pick("textSize", (v) => typeof v === "string" && v in TEXT_SIZES),
    runAfterSave: pick("runAfterSave", (v) => typeof v === "boolean"),
    autoHideMessages: pick("autoHideMessages", (v) => typeof v === "boolean"),
    thousandsSeparators: pick("thousandsSeparators", (v) => typeof v === "boolean"),
    maxDecimals: pick("maxDecimals", (v) => typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= 10),
    editorKeys: pick("editorKeys", (v) => v === "standard" || v === "vim"),
    wrapCode: pick("wrapCode", (v) => typeof v === "boolean"),
    rowDensity: pick("rowDensity", (v) => ROW_DENSITIES.includes(v as RowDensity)),
    missingText: pick("missingText", (v) => MISSING_TEXTS.includes(v as MissingText)),
    dateFormat: pick("dateFormat", (v) => DATE_FORMATS.includes(v as DateFormat)),
    missingMarkers: Array.isArray(p.missingMarkers) && p.missingMarkers.length <= 50 && p.missingMarkers.every((m) => typeof m === "string" && m.length <= 40) ? (p.missingMarkers as string[]) : [...DEFAULTS.missingMarkers],
    editorFontSize: pick("editorFontSize", (v) => typeof v === "number" && Number.isInteger(v) && v >= 10 && v <= 24),
    tabWidth: pick("tabWidth", (v) => typeof v === "number" && TAB_WIDTHS.includes(v)),
    lineNumbers: pick("lineNumbers", (v) => typeof v === "boolean"),
    autoCloseBrackets: pick("autoCloseBrackets", (v) => typeof v === "boolean"),
    highlightActiveLine: pick("highlightActiveLine", (v) => typeof v === "boolean"),
    reduceMotion: pick("reduceMotion", (v) => typeof v === "boolean"),
    runOnOpen: pick("runOnOpen", (v) => typeof v === "boolean"),
    stepTimeoutSeconds: pick("stepTimeoutSeconds", (v) => typeof v === "number" && Number.isInteger(v) && v >= 1 && v <= 86400),
    codeMemoryMb: typeof p.codeMemoryMb === "number" && Number.isInteger(p.codeMemoryMb) && p.codeMemoryMb >= 0 && p.codeMemoryMb <= 262144 ? p.codeMemoryMb : undefined,
    versionsKept: pick("versionsKept", (v) => typeof v === "number" && Number.isInteger(v) && v >= 10 && v <= 5000),
    exportFormat: pick("exportFormat", (v) => EXPORT_FORMATS.includes(v as ExportFormat)),
    reopenLastProject: pick("reopenLastProject", (v) => typeof v === "boolean"),
    lastProject: typeof p.lastProject === "string" && p.lastProject ? p.lastProject : undefined,
    projectsFolder: typeof p.projectsFolder === "string" && p.projectsFolder.trim() ? p.projectsFolder : undefined,
    exportsFolder: typeof p.exportsFolder === "string" && p.exportsFolder.trim() ? p.exportsFolder : undefined,
  };
}

export function isCustomTheme(v: unknown): v is CustomTheme {
  if (!v || typeof v !== "object") return false;
  const t = v as Record<string, unknown>;
  return (
    typeof t.id === "string" &&
    t.id.startsWith("custom-") &&
    typeof t.name === "string" &&
    THEMES.some((b) => b.id === t.base) &&
    !!t.colors &&
    typeof t.colors === "object" &&
    Object.values(t.colors as object).every((c) => typeof c === "string" && /^#[0-9a-fA-F]{6}$/.test(c))
  );
}

function readLocal(): Preferences {
  try {
    return normalize(JSON.parse(localStorage.getItem(LOCAL) ?? "{}"));
  } catch {
    return { ...DEFAULTS };
  }
}

export const prefs = $state<{ value: Preferences; themeId: string }>({ value: readLocal(), themeId: "paper" });

/** The theme a choice means, custom themes included. */
export function themeFor(choice: string, customs: CustomTheme[] = prefs.value.customThemes): Theme {
  const custom = customs.find((c) => c.id === choice);
  return custom ? buildCustom(custom) : resolve((THEMES.some((t) => t.id === choice) ? choice : "system") as ThemeChoice);
}

/** Put the preferences that change the whole page into effect. */
export function applyPrefs(p: Preferences = prefs.value) {
  const t = themeFor(p.theme, p.customThemes);
  apply(t);
  prefs.themeId = t.id;
  if (typeof document !== "undefined") document.documentElement.style.fontSize = `${TEXT_SIZES[p.textSize].px}px`;
  numberDisplay.thousands = p.thousandsSeparators;
  numberDisplay.maxDecimals = p.maxDecimals;
  cellDisplay.missing = p.missingText;
  cellDisplay.dates = p.dateFormat;
  if (typeof document !== "undefined") {
    const root = document.documentElement;
    root.style.setProperty("--editor-font-size", `${p.editorFontSize / 16}rem`);
    root.style.setProperty("--row-height", `${ROW_HEIGHTS[p.rowDensity]}px`);
    if (p.reduceMotion) root.setAttribute("data-reduce-motion", "true");
    else root.removeAttribute("data-reduce-motion");
  }
}

type Store = { preferences(): Promise<unknown>; setPreferences(p: unknown): Promise<unknown> };
let store: Store | null = null;

/** At start: apply the local copy at once, then the saved preferences. */
export async function initPrefs(backend: Store) {
  store = backend;
  applyPrefs();
  try {
    const saved = normalize(await backend.preferences());
    // A first start keeps what this copy already had (an earlier version kept the theme locally).
    prefs.value = { ...saved, theme: saved.theme === DEFAULTS.theme ? prefs.value.theme : saved.theme };
    keepLocal();
    applyPrefs();
  } catch {
    // Keep the local copy.
  }
  if (typeof matchMedia === "function") {
    matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
      if (prefs.value.theme === "system") applyPrefs();
    });
  }
}

function keepLocal() {
  try {
    localStorage.setItem(LOCAL, JSON.stringify(prefs.value));
  } catch {
    // The backend has them.
  }
}

/** Change some preferences: applied at once, then saved. */
export async function updatePrefs(patch: Partial<Preferences>): Promise<string | null> {
  prefs.value = normalize({ ...prefs.value, ...patch });
  applyPrefs();
  keepLocal();
  if (!store) return null;
  try {
    const saved = normalize(await store.setPreferences($state.snapshot(prefs.value)));
    prefs.value = saved;
    keepLocal();
    return null;
  } catch (e) {
    return (e as { message?: string }).message ?? String(e);
  }
}
