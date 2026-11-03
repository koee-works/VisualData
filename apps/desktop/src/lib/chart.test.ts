import { describe, expect, it } from "vitest";
import { chartSvg } from "./report";

// Histograms are ranged bars (x to x2). A style setting once drew them with
// no width; every bar here must be as wide as its bin.
describe("Chart drawing", () => {
  it("draws histogram bars with their bin's width", async () => {
    const spec = {
      data: { values: [{ start: 0, end: 20, rows: 3 }, { start: 20, end: 40, rows: 8 }] },
      mark: { type: "bar", tooltip: true },
      encoding: {
        x: { field: "start", type: "quantitative", bin: { binned: true, step: 20 }, title: "revenue" },
        x2: { field: "end" },
        y: { field: "rows", type: "quantitative" },
      },
    };
    const svg = await chartSvg(spec);
    const bars = [...svg.matchAll(/aria-roledescription="bar" d="M[\d.]+,[\d.]+h([\d.]+)/g)].map((m) => Number(m[1]));
    expect(bars).toHaveLength(2);
    expect(Math.min(...bars)).toBeGreaterThan(100);
  });
});
