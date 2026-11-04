import { describe, expect, it } from "vitest";
import { EditorState, Text } from "@codemirror/state";
import { schemaFor, suggest, type EditorContext } from "./complete";
import { diagnostics } from "./setup";
import type { Problem } from "../api/types";

const ctx: EditorContext = {
  inputs: [
    { id: "sales", name: "sales.csv", columns: [{ name: "region", type: "text" }, { name: "units", type: "whole number" }, { name: "Unit price", type: "decimal" }] },
    { id: "targets", name: "regions.csv", columns: [{ name: "target", type: "decimal" }] },
  ],
  parameters: [{ name: "goal", kind: "float" }],
};

const labels = (before: string, lang: Parameters<typeof suggest>[0]) => suggest(lang, ctx, before)?.options.map((o) => o.label) ?? [];

describe("Python completion", () => {
  it("offers the first input's columns inside df[...]", () => {
    const c = suggest("python", ctx, 'x = df["re')!;
    expect(c.options.map((o) => o.label)).toEqual(["region", "units", "Unit price"]);
    expect(c.from).toBe('x = df["'.length);
  });
  it("offers inputs in dfs[...] and their columns after", () => {
    expect(labels('dfs["', "python")).toEqual(["sales", "targets"]);
    expect(labels('dfs["targets"]["t', "python")).toEqual(["target"]);
  });
  it("offers parameters in params[...]", () => {
    expect(labels("params['", "python")).toEqual(["goal"]);
  });
  it("offers identifier columns and pandas methods after df.", () => {
    const l = labels("df.", "python");
    expect(l).toContain("units");
    expect(l).not.toContain("Unit price");
    expect(l).toContain("groupby");
  });
  it("offers the step's names, not after a dot", () => {
    expect(labels("r", "python")).toEqual(expect.arrayContaining(["df", "dfs", "params", "result", "pd"]));
    expect(labels("np.r", "python")).toEqual([]);
  });
});

describe("R completion", () => {
  it("offers columns after df$ and in df[[...]]", () => {
    expect(labels("df$", "r")).toEqual(["region", "units", "Unit price"]);
    expect(labels('df[["u', "r")).toEqual(["region", "units", "Unit price"]);
  });
  it("offers inputs and their columns", () => {
    expect(labels("dfs$", "r")).toEqual(["sales", "targets"]);
    expect(labels("dfs$targets$", "r")).toEqual(["target"]);
    expect(labels('dfs[["targets"]]$', "r")).toEqual(["target"]);
  });
  it("offers bare columns and dplyr verbs in a pipeline", () => {
    const c = suggest("r", ctx, "df |> filter(un")!;
    expect(c.options.map((o) => o.label)).toEqual(expect.arrayContaining(["units", "filter", "mutate"]));
    expect(c.options.find((o) => o.label === "mutate")!.apply).toBe("mutate(");
  });
});

describe("SQL and formulas", () => {
  it("gives SQL a schema named input and by id", () => {
    expect(schemaFor(ctx)).toEqual({ input: ["region", "units", "Unit price"], sales: ["region", "units", "Unit price"], targets: ["target"] });
    expect(labels("WHERE share > $g", "sql")).toEqual(["goal"]);
  });
  it("offers parameters, bracketed columns, and functions in formulas", () => {
    expect(labels("units * @", "formula")).toEqual(["goal"]);
    expect(labels("[Un", "formula")).toEqual(["region", "units", "Unit price"]);
    const l = labels("rou", "formula");
    expect(l).toContain("round");
    expect(l).toContain("units");
    expect(l).not.toContain("Unit price");
  });
});

describe("problems as underlines", () => {
  const p = (line: number | null, column: number | null, severity: Problem["severity"] = "error"): Problem => ({
    severity, code: "x", step: "s", field: "code", line, column, message: "m", from_last_run: false, dismissable: false,
  });
  const doc = Text.of(["result = df", "x = undefined_name + 1"]);
  it("underlines the word at a line and column", () => {
    const [d] = diagnostics(doc, [p(2, 5)]);
    expect(doc.sliceString(d!.from, d!.to)).toBe("undefined_name");
  });
  it("underlines a whole line without a column, and skips problems with no place", () => {
    const ds = diagnostics(doc, [p(1, null, "warning"), p(null, null)]);
    expect(ds).toHaveLength(1);
    expect(doc.sliceString(ds[0]!.from, ds[0]!.to)).toBe("result = df");
    expect(ds[0]!.severity).toBe("warning");
  });
  it("keeps lines and columns past the end inside the text", () => {
    const [d] = diagnostics(doc, [p(9, 99)]);
    expect(d!.from).toBeLessThanOrEqual(EditorState.create({ doc }).doc.length);
  });
});
