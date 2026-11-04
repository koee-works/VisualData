// The app's themes: each a full set of colour tokens (the same names as
// app.css's @theme), so every theme keeps the meanings of the state colours
// (teal current, ochre outdated, brick failed, indigo running, plum review)
// and is checked for contrast in theme.test.ts.

export type ThemeId = "paper" | "night" | "sepia" | "contrast";
export type ThemeChoice = ThemeId | "system";

export interface Theme {
  id: ThemeId;
  name: string;
  /** One line for the picker. */
  hint: string;
  dark: boolean;
  colors: Record<string, string>;
  /** Categorical colours for charts, in order. */
  chart: string[];
  /** Shadow under menus and dialogs. */
  shadow: string;
}

const paper: Theme = {
  id: "paper",
  name: "Paper",
  hint: "Cool drafting paper in navy ink",
  dark: false,
  colors: {
    ink: "#1d2a3f",
    "ink-soft": "#4a5872",
    "ink-faint": "#5c687c",
    paper: "#eef2f5",
    panel: "#ffffff",
    rule: "#d5dce4",
    "rule-strong": "#8592a3",
    select: "#dfe7f7",
    "active-line": "#f6f8fa",
    fresh: "#1b6f62",
    "fresh-wash": "#e3f2ef",
    stale: "#8a560c",
    "stale-wash": "#fbf1df",
    failed: "#a8322d",
    "failed-wash": "#f9e5e3",
    running: "#4652c8",
    "running-wash": "#e6e8fa",
    review: "#7b3b8f",
    "review-wash": "#f5ebf8",
  },
  chart: ["#2f7fb8", "#c27c1e", "#2b9a7a", "#c45a3b", "#5b5fb0", "#a44a7c", "#7c8a99"],
  shadow: "rgba(29,42,63,0.18)",
};

const night: Theme = {
  id: "night",
  name: "Night",
  hint: "Dark slate, for long sessions and dim rooms",
  dark: true,
  colors: {
    ink: "#e4e9f1",
    "ink-soft": "#b4bfce",
    "ink-faint": "#949fb1",
    paper: "#12161e",
    panel: "#1a1f29",
    rule: "#2c3342",
    "rule-strong": "#69758a",
    select: "#263352",
    "active-line": "#202633",
    fresh: "#5cc8ad",
    "fresh-wash": "#15302b",
    stale: "#e6ad55",
    "stale-wash": "#33270f",
    failed: "#f2867c",
    "failed-wash": "#3a1c1a",
    running: "#98a2ff",
    "running-wash": "#22264a",
    review: "#d7a2ea",
    "review-wash": "#311f39",
  },
  chart: ["#5aa9e6", "#f0a84c", "#4fc1a0", "#f07f5e", "#9a9df0", "#e07ab3", "#a3afbf"],
  shadow: "rgba(0,0,0,0.5)",
};

const sepia: Theme = {
  id: "sepia",
  name: "Sepia",
  hint: "Warm paper, easier on the eyes in daylight",
  dark: false,
  colors: {
    ink: "#2b2418",
    "ink-soft": "#574c3b",
    "ink-faint": "#665b49",
    paper: "#f1ebdf",
    panel: "#fbf8f2",
    rule: "#e0d6c4",
    "rule-strong": "#9a8c74",
    select: "#e9dec7",
    "active-line": "#f5efe3",
    fresh: "#2a6a4e",
    "fresh-wash": "#e2efe5",
    stale: "#8a4f00",
    "stale-wash": "#f6e8cf",
    failed: "#9b2c22",
    "failed-wash": "#f5e1dc",
    running: "#3e4fa8",
    "running-wash": "#e4e5f3",
    review: "#74368a",
    "review-wash": "#f1e5f2",
  },
  chart: ["#2f6f9f", "#c27c1e", "#2f8a68", "#b5533a", "#5a5aa0", "#9a4877", "#7d7365"],
  shadow: "rgba(43,36,24,0.18)",
};

const contrast: Theme = {
  id: "contrast",
  name: "High contrast",
  hint: "Black on white, strong lines, for low vision or bright light",
  dark: false,
  colors: {
    ink: "#000000",
    "ink-soft": "#1f2733",
    "ink-faint": "#363f4d",
    paper: "#f2f4f7",
    panel: "#ffffff",
    rule: "#6b7685",
    "rule-strong": "#2b3440",
    select: "#c9d8f6",
    "active-line": "#eef1f6",
    fresh: "#065445",
    "fresh-wash": "#d9efe8",
    stale: "#5f3a00",
    "stale-wash": "#f7e6c7",
    failed: "#8a1610",
    "failed-wash": "#f6dcd9",
    running: "#202ca8",
    "running-wash": "#dfe2fb",
    review: "#56186b",
    "review-wash": "#eedff3",
  },
  chart: ["#0b5ea8", "#b35f00", "#077a5d", "#b0301b", "#3e40a8", "#8f2765", "#3d4652"],
  shadow: "rgba(0,0,0,0.35)",
};

export const THEMES: Theme[] = [paper, night, sepia, contrast];

export function themeById(id: ThemeId): Theme {
  return THEMES.find((t) => t.id === id) ?? paper;
}

const media = () => (typeof matchMedia === "function" ? matchMedia("(prefers-color-scheme: dark)") : null);

/** The theme a choice means now ("system" follows the computer's setting). */
export function resolve(choice: ThemeChoice): Theme {
  if (choice === "system") return media()?.matches ? night : paper;
  return themeById(choice);
}

export function loadChoice(): ThemeChoice {
  try {
    const v = localStorage.getItem("vda.theme");
    if (v === "system" || THEMES.some((t) => t.id === v)) return v as ThemeChoice;
  } catch {
    // Fall through.
  }
  return "system";
}

export function saveChoice(c: ThemeChoice) {
  try {
    localStorage.setItem("vda.theme", c);
  } catch {
    // A convenience only.
  }
}

/** Put a theme's tokens on the page. */
export function apply(t: Theme, root: HTMLElement = document.documentElement) {
  for (const [k, v] of Object.entries(t.colors)) root.style.setProperty(`--color-${k}`, v);
  root.style.setProperty("--shadow-pop", `0 12px 40px ${t.shadow}`);
  root.style.setProperty("--shadow-menu", `0 6px 24px ${t.shadow}`);
  root.style.colorScheme = t.dark ? "dark" : "light";
  root.dataset.theme = t.id;
}

/** The current theme's chart colours, read when a chart is drawn. */
export function current(): Theme {
  const id = typeof document !== "undefined" ? (document.documentElement.dataset.theme as ThemeId | undefined) : undefined;
  return themeById(id ?? "paper");
}

// --- Contrast (WCAG 2.1), for the tests and the picker's swatches.

function lum(hex: string): number {
  const h = hex.replace("#", "");
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16) / 255);
  const f = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  return 0.2126 * f(r!) + 0.7152 * f(g!) + 0.0722 * f(b!);
}

export function contrastRatio(a: string, b: string): number {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x! + 0.05) / (y! + 0.05);
}
