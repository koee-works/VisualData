// The user's preferences: kept in their settings file through the backend
// (so they move with the user's settings, not with one browser profile), with
// a local copy so the theme and text size apply before the first paint.

import { apply, buildCustom, resolve, THEMES, type CustomTheme, type Theme, type ThemeChoice } from "./theme";
import { numberDisplay } from "./format";

export type TextSize = "small" | "default" | "large" | "larger";

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
};

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
