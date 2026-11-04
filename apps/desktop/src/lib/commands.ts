// The command palette's commands and how typing finds them.

export interface Command {
  id: string;
  /** "Actions" or "Go to". */
  group: "Actions" | "Go to";
  title: string;
  /** Extra words that should find it (not shown). */
  keywords?: string;
  /** Shown on the right, e.g. "Ctrl+B". */
  shortcut?: string;
  /** Why it can't be used now; the command is shown dimmed. */
  disabled?: string;
  run: () => void;
}

/**
 * How well `query` matches `text`: the letters of the query must appear in
 * order. Higher is better; null when it doesn't match. Matches at the start
 * of words and runs of letters score more, so "tg" finds "Toggle grid" and
 * "run" ranks "Run analysis" above "Refresh un…".
 */
export function score(query: string, text: string): number | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return 0;
  const t = text.toLowerCase();
  let s = 0;
  let ti = 0;
  let run = 0;
  for (const ch of q) {
    const at = t.indexOf(ch, ti);
    if (at < 0) return null;
    const wordStart = at === 0 || /[\s\-_/(:,.]/.test(t[at - 1]!);
    if (at === ti && ti > 0) run += 1;
    else run = 0;
    s += 1 + (wordStart ? 6 : 0) + run * 3 - Math.min(at - ti, 8) * 0.25;
    ti = at + 1;
  }
  // A whole-word or prefix match beats letters spread across the text.
  if (t.startsWith(query.toLowerCase().trim())) s += 10;
  return s - t.length * 0.02;
}

/** The commands matching the query, best first; recent ones first when empty. */
export function filter(commands: Command[], query: string, recent: string[] = []): Command[] {
  if (!query.trim()) {
    const rank = (c: Command) => {
      const i = recent.indexOf(c.id);
      return i < 0 ? recent.length : i;
    };
    return [...commands].sort((a, b) => rank(a) - rank(b));
  }
  return commands
    .map((c) => {
      const title = score(query, c.title);
      const words = c.keywords ? score(query, c.keywords) : null;
      const best = Math.max(title ?? -Infinity, words === null ? -Infinity : words - 4);
      return { c, s: best + (recent.includes(c.id) ? 2 : 0) };
    })
    .filter((x) => x.s > -Infinity)
    .sort((a, b) => b.s - a.s)
    .map((x) => x.c);
}

const RECENT_KEY = "vda.palette.recent";

export function loadRecent(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string").slice(0, 8) : [];
  } catch {
    return [];
  }
}

export function remember(id: string, recent: string[]): string[] {
  const next = [id, ...recent.filter((r) => r !== id)].slice(0, 8);
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    // A convenience only.
  }
  return next;
}
