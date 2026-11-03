import { describe, expect, it } from "vitest";
import { describe as describeStep, formatCell, parseLiteral } from "./format";
import { renderMarkdown } from "./markdown";

describe("formatCell", () => {
  it("matches the core's display rules", () => {
    expect(formatCell({ t: "float", v: 12 })).toBe("12.0");
    expect(formatCell({ t: "float", v: 2.75 })).toBe("2.75");
    expect(formatCell({ t: "int", v: "9007199254740993" })).toBe("9007199254740993");
    expect(formatCell({ t: "null" })).toBe("");
    expect(formatCell({ t: "bool", v: false })).toBe("false");
    expect(formatCell({ t: "float", v: 36.72222222222222 })).toBe("36.722222");
    expect(formatCell({ t: "float", v: 1e-9 })).toBe("1e-9");
  });
});

describe("parseLiteral", () => {
  it("types values by the column", () => {
    expect(parseLiteral("20", "DOUBLE")).toEqual({ int: "20" });
    expect(parseLiteral("19.5", "DOUBLE")).toEqual({ float: 19.5 });
    expect(parseLiteral("1.5", "BIGINT")).toEqual({ float: 1.5 });
    expect(parseLiteral("2026-07-03", "DATE")).toEqual({ date: "2026-07-03" });
    expect(parseLiteral("2026-07-03T10:00:00", "TIMESTAMP")).toEqual({ timestamp: "2026-07-03 10:00:00" });
    expect(parseLiteral("East", "VARCHAR")).toBe("East");
    expect(parseLiteral("true", "BOOLEAN")).toBe(true);
  });

  it("explains what to type when the value does not fit", () => {
    expect(parseLiteral("abc", "BIGINT")).toEqual({ error: "Enter a whole number, like 20" });
    expect(parseLiteral("07/03/2026", "DATE")).toEqual({ error: "Enter a date as YYYY-MM-DD" });
    expect(parseLiteral("", "VARCHAR")).toEqual({ error: "Enter some text" });
  });
});

describe("describe", () => {
  it("summarizes steps in words", () => {
    expect(
      describeStep({
        type: "filter",
        predicates: [
          { column: "region", op: "IS NOT NULL", value: null },
          { column: "revenue", op: ">=", value: { int: "20" } },
        ],
      }),
    ).toBe("Keep rows where region is present and revenue at least 20");
    expect(
      describeStep({ type: "aggregate", group_by: ["region"], measures: [{ func: "SUM", column: "revenue", alias: "r" }] }),
    ).toBe("sum of revenue by region");
  });
});

describe("renderMarkdown", () => {
  it("renders the supported subset", () => {
    expect(renderMarkdown("## Question\n\nWhy **this**?\n\n- a\n- `b`")).toBe(
      "<h3>Question</h3><p>Why <strong>this</strong>?</p><ul><li>a</li><li><code>b</code></li></ul>",
    );
  });

  it("escapes HTML so notebook text cannot inject markup", () => {
    const html = renderMarkdown('<img src=x onerror="alert(1)"> & <script>');
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script");
    expect(html).toContain("&lt;img");
  });
});
