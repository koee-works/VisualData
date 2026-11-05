import { describe, expect, it } from "vitest";
import { cellDisplay, describe as describeStep, exactCell, formatCell, formatDateText, missingLabel, parseLiteral } from "./format";
import { renderMarkdown } from "./markdown";

describe("numbers in a column line up", () => {
  const f = (v: number) => ({ t: "float" as const, v });
  it("uses the decimals the most precise value needs, capped", async () => {
    const { columnDecimals, formatCell } = await import("./format");
    const col = [f(1615), f(30.6), f(27.605263157)];
    const d = columnDecimals(col);
    expect(d).toBe(4);
    expect(col.map((c) => formatCell(c, d))).toEqual(["1,615.0000", "30.6000", "27.6053"]);
    expect(columnDecimals([f(11656), f(11003.5)])).toBe(1);
    expect(formatCell(f(11656), 1)).toBe("11,656.0");
    expect(columnDecimals([f(0.0012345), f(0.5)])).toBe(6);
    expect(columnDecimals([{ t: "null" }, { t: "int", v: "3" }])).toBe(1);
    expect(formatCell(f(1e-9), 4)).toBe("1e-9");
  });
});

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

describe("dates and missing values follow the settings", () => {
  it("writes dates in the chosen format and leaves anything else alone", () => {
    expect(formatDateText("2026-07-02", "iso")).toBe("2026-07-02");
    expect(formatDateText("2026-07-02", "us")).toBe("7/2/2026");
    expect(formatDateText("2026-07-02", "eu")).toBe("02.07.2026");
    expect(formatDateText("2026-07-02", "long")).toBe("2 Jul 2026");
    expect(formatDateText("2026-07-02 13:45:00", "long")).toBe("2 Jul 2026 13:45:00");
    expect(formatDateText("2026-07-02T13:45:00", "us")).toBe("7/2/2026 13:45:00");
    expect(formatDateText("not a date", "us")).toBe("not a date");
    expect(formatDateText("2026-13-02", "us")).toBe("2026-13-02");
  });

  it("applies the format to date cells but keeps the exact value for tooltips", () => {
    const before = cellDisplay.dates;
    cellDisplay.dates = "us";
    expect(formatCell({ t: "date", v: "2026-07-02" })).toBe("7/2/2026");
    expect(formatCell({ t: "timestamp", v: "2026-07-02 09:30:00" })).toBe("7/2/2026 09:30:00");
    expect(exactCell({ t: "date", v: "2026-07-02" })).toBe("2026-07-02");
    expect(formatCell({ t: "text", v: "2026-07-02" })).toBe("2026-07-02");
    cellDisplay.dates = before;
  });

  it("shows missing cells as chosen", () => {
    const before = cellDisplay.missing;
    for (const [choice, shown] of [["missing", "missing"], ["dash", "—"], ["blank", ""], ["null", "null"]] as const) {
      cellDisplay.missing = choice;
      expect(missingLabel()).toBe(shown);
    }
    cellDisplay.missing = before;
  });
});
