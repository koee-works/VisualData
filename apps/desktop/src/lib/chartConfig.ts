/** Ink-on-paper chart styling, shared by the app and exported reports. */
export const chartConfig = {
  font: "Atkinson Hyperlegible Next",
  background: null,
  view: { stroke: null },
  axis: {
    labelColor: "#4a5872",
    titleColor: "#1d2a3f",
    gridColor: "#e2e7ed",
    domainColor: "#b6c1cd",
    tickColor: "#b6c1cd",
    labelFontSize: 12,
    titleFontSize: 12,
  },
  axisX: { labelAngle: 0, labelOverlap: true },
  scale: { bandPaddingInner: 0.35, bandPaddingOuter: 0.2 },
  title: { color: "#1d2a3f", fontSize: 14, anchor: "start" },
  // Series colors: ink first, then hues distinct in print and for common color blindness.
  range: { category: ["#1d2a3f", "#2f7fb8", "#d08a2c", "#2b9a7a", "#a44a7c", "#7c8a99", "#c45a3b", "#5b5fb0"] },
  mark: { color: "#1d2a3f" },
  // No rounded bar ends: Vega-Lite 6 draws ranged bars (histograms) with no
  // width when cornerRadiusEnd is set.
  bar: { color: "#1d2a3f" },
  line: { color: "#1d2a3f", strokeWidth: 2 },
  point: { color: "#1d2a3f", filled: true },
  area: { color: "#1d2a3f" },
  rule: { color: "#1d2a3f" },
};
