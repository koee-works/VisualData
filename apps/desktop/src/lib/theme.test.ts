import { describe, expect, it } from "vitest";
// @ts-expect-error: the app is typed for the browser; tests run in Node.
import { readFileSync } from "node:fs";
import { contrastRatio, THEMES } from "./theme";

// Text pairs as the app uses them: [text, background, minimum].
const TEXT: [string, string, number][] = [
  ["ink", "panel", 7],
  ["ink", "paper", 7],
  ["ink", "select", 4.5],
  ["ink-soft", "panel", 4.5],
  ["ink-soft", "paper", 4.5],
  ["ink-faint", "panel", 4.5],
  ["ink-faint", "paper", 4.5],
  ["ink-faint", "select", 4.5],
  ["fresh", "panel", 4.5],
  ["fresh", "fresh-wash", 4.5],
  ["stale", "panel", 4.5],
  ["stale", "stale-wash", 4.5],
  ["failed", "panel", 4.5],
  ["failed", "failed-wash", 4.5],
  ["running", "panel", 4.5],
  ["running", "running-wash", 4.5],
  ["review", "panel", 4.5],
  ["review", "review-wash", 4.5],
  ["ink", "fresh-wash", 4.5],
  ["ink", "stale-wash", 4.5],
];

describe("every theme is readable", () => {
  for (const t of THEMES) {
    it(`${t.name}: text meets WCAG AA`, () => {
      for (const [fg, bg, min] of TEXT) {
        const r = contrastRatio(t.colors[fg]!, t.colors[bg]!);
        expect(r, `${fg} on ${bg}: ${r.toFixed(2)}`).toBeGreaterThanOrEqual(min);
      }
    });
    it(`${t.name}: control edges and chart colours stand out`, () => {
      // Field borders against what they sit on (WCAG 1.4.11).
      expect(contrastRatio(t.colors["rule-strong"]!, t.colors.panel!)).toBeGreaterThanOrEqual(3);
      for (const c of t.chart) expect(contrastRatio(c, t.colors.panel!), c).toBeGreaterThanOrEqual(3);
    });
  }

  it("has the same tokens in every theme, and Paper is what app.css declares", () => {
    const keys = Object.keys(THEMES[0]!.colors).sort();
    for (const t of THEMES) expect(Object.keys(t.colors).sort()).toEqual(keys);
    const css: string = readFileSync("src/app.css", "utf8");
    for (const [k, v] of Object.entries(THEMES[0]!.colors)) expect(css, `--color-${k}`).toContain(`--color-${k}: ${v};`);
  });
});
