import { themeById, type Theme } from "./theme";

/** Chart styling from a theme's tokens. Exported reports use Paper. */
export function chartConfigFor(t: Theme) {
  const c = t.colors;
  const ink = c.ink!;
  return {
    font: "Atkinson Hyperlegible Next",
    background: null,
    view: { stroke: null },
    axis: {
      labelColor: c["ink-soft"],
      titleColor: ink,
      gridColor: c.rule,
      domainColor: c["rule-strong"],
      tickColor: c["rule-strong"],
      labelFontSize: 12,
      titleFontSize: 12,
    },
    axisX: { labelAngle: 0, labelOverlap: true },
    scale: { bandPaddingInner: 0.35, bandPaddingOuter: 0.2 },
    title: { color: ink, fontSize: 14, anchor: "start" },
    legend: { labelColor: c["ink-soft"], titleColor: ink },
    // Series colors: ink first, then hues distinct in print and for common color blindness.
    range: { category: [ink, ...t.chart] },
    mark: { color: ink },
    // No rounded bar ends: Vega-Lite 6 draws ranged bars (histograms) with no
    // width when cornerRadiusEnd is set.
    bar: { color: ink },
    line: { color: ink, strokeWidth: 2 },
    point: { color: ink, filled: true },
    area: { color: ink },
    rule: { color: ink },
  };
}

/** Ink-on-paper chart styling, shared by the app and exported reports. */
export const chartConfig = chartConfigFor(themeById("paper"));
