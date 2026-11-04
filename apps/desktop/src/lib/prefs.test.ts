import { describe, expect, it } from "vitest";
import { DEFAULTS, normalize, themeFor } from "./prefs.svelte";
import { buildCustom, contrastProblems, THEMES } from "./theme";

describe("preferences", () => {
  it("fills in defaults and drops values of the wrong kind", () => {
    const p = normalize({ textSize: "huge", maxDecimals: 40, runAfterSave: "yes", editorKeys: "vim", wrapCode: true });
    expect(p.textSize).toBe(DEFAULTS.textSize);
    expect(p.maxDecimals).toBe(DEFAULTS.maxDecimals);
    expect(p.runAfterSave).toBe(DEFAULTS.runAfterSave);
    expect(p.editorKeys).toBe("vim");
    expect(p.wrapCode).toBe(true);
    expect(normalize(null)).toEqual({ ...DEFAULTS, author: undefined });
  });

  it("keeps only well-formed custom themes", () => {
    const good = { id: "custom-1", name: "Mine", base: "night", colors: { paper: "#000000" } };
    const p = normalize({ customThemes: [good, { id: "x", name: "Bad", base: "night", colors: {} }, { ...good, id: "custom-2", colors: { paper: "red" } }] });
    expect(p.customThemes.map((t) => t.id)).toEqual(["custom-1"]);
    expect(themeFor("custom-1", p.customThemes).colors.paper).toBe("#000000");
  });
});

describe("custom theme contrast", () => {
  it("finds nothing wrong with an unchanged built-in theme", () => {
    for (const t of THEMES) expect(contrastProblems(buildCustom({ id: "custom-x", name: "x", base: t.id, colors: {} }).colors)).toEqual([]);
  });

  it("flags text that is too close to its background", () => {
    const t = buildCustom({ id: "custom-x", name: "x", base: "paper", colors: { "ink-faint": "#c8cdd4" } });
    const what = contrastProblems(t.colors).map((p) => p.what);
    expect(what).toContain("Hints on panels");
    expect(what).not.toContain("Text on panels");
  });

  it("treats a dark background as a dark theme", () => {
    expect(buildCustom({ id: "custom-x", name: "x", base: "paper", colors: { paper: "#101010", panel: "#181818", ink: "#f0f0f0" } }).dark).toBe(true);
  });
});
