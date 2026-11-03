// The notebook as a self-contained HTML page: text, each step's first rows,
// charts drawn as SVG, and the code of code steps. No scripts, so it opens
// anywhere and can be printed or attached as it is.

import type { Workspace } from "./workspace.svelte";
import type { CheckReport, Page, StepView } from "./api/types";
import { chartConfig } from "./chartConfig";
import { formatCell, formatCount, literalText, stepTitle, typeLabel } from "./format";
import { renderMarkdown } from "./markdown";

/** Rows of each table shown in the report. */
export const REPORT_ROWS = 20;

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function table(p: Page): string {
  const head = p.columns.map((c) => `<th><span>${esc(c.name)}</span><small>${esc(typeLabel[c.type])}</small></th>`).join("");
  const body = p.rows
    .map((r) => `<tr>${r.map((c, i) => `<td class="${p.columns[i]?.type === "BIGINT" || p.columns[i]?.type === "DOUBLE" ? "num" : ""}${c.t === "null" ? " missing" : ""}">${c.t === "null" ? "missing" : esc(formatCell(c))}</td>`).join("")}</tr>`)
    .join("");
  const note =
    p.total_rows > p.rows.length
      ? `<p class="note">First ${formatCount(p.rows.length)} of ${formatCount(p.total_rows)} rows.</p>`
      : `<p class="note">${formatCount(p.total_rows)} row${p.total_rows === 1 ? "" : "s"}.</p>`;
  return `<div class="table"><table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table></div>${note}`;
}

/** A Vega-Lite specification drawn as an SVG string. */
export async function chartSvg(spec: unknown): Promise<string> {
  const [{ compile }, { View, parse }, { expressionInterpreter }] = await Promise.all([
    import("vega-lite"),
    import("vega"),
    import("vega-interpreter"),
  ]);
  const vl = { ...(spec as Record<string, unknown>), width: 640, height: 280, config: chartConfig };
  const vg = compile(vl as never).spec;
  const view = new View(parse(vg, undefined, { ast: true }), { renderer: "none", expr: expressionInterpreter as never });
  const svg = await view.toSVG();
  view.finalize();
  return svg;
}

const STYLE = `
:root { --ink: #1d2a3f; --soft: #4a5872; --faint: #7c8a99; --rule: #e2e7ed; --paper: #f7f8fa; --stale: #9a6412; }
* { box-sizing: border-box; }
body { margin: 0; background: #fff; color: var(--ink); font: 15px/1.55 "Atkinson Hyperlegible Next", "Atkinson Hyperlegible", system-ui, sans-serif; }
main { max-width: 46rem; margin: 0 auto; padding: 3rem 1.25rem 4rem; }
h1 { font-size: 1.75rem; line-height: 1.2; margin: 0 0 0.25rem; }
h2 { font-size: 1.25rem; margin: 2rem 0 0.5rem; }
h3 { font-size: 1.05rem; margin: 2rem 0 0.25rem; }
p { margin: 0.5rem 0; max-width: 68ch; }
.meta { color: var(--soft); margin-bottom: 2rem; }
.step { margin: 2.25rem 0; }
.step h3 { margin-top: 0; }
.reads { color: var(--soft); font-size: 0.875rem; margin-top: 0; }
.stale { border-left: 3px solid var(--stale); padding: 0.25rem 0.75rem; color: var(--stale); font-size: 0.875rem; }
.checks { padding-left: 1.1rem; }
.checks li { margin: 0.4rem 0; }
.checks li.failed strong { color: var(--stale); }
.checks .table { margin-top: 0.35rem; }
.table { overflow-x: auto; border: 1px solid var(--rule); border-radius: 6px; }
table { border-collapse: collapse; width: 100%; font-size: 0.8125rem; font-variant-numeric: tabular-nums; }
th { text-align: left; background: var(--paper); padding: 0.4rem 0.6rem; vertical-align: bottom; border-bottom: 1px solid var(--rule); }
th span { display: block; } th small { display: block; color: var(--faint); font-weight: 400; }
td { padding: 0.3rem 0.6rem; border-bottom: 1px solid var(--rule); white-space: nowrap; }
td.num { text-align: right; } td.missing { color: var(--faint); font-style: italic; }
tr:last-child td { border-bottom: 0; }
.note { color: var(--faint); font-size: 0.8125rem; margin-top: 0.35rem; }
svg { max-width: 100%; height: auto; }
details { margin-top: 0.75rem; } summary { cursor: pointer; color: var(--soft); font-size: 0.875rem; }
pre { background: var(--paper); border-radius: 6px; padding: 0.75rem; overflow-x: auto; font: 0.8125rem/1.5 "JetBrains Mono", ui-monospace, monospace; }
code { font-family: "JetBrains Mono", ui-monospace, monospace; font-size: 0.875em; }
footer { margin-top: 3rem; border-top: 1px solid var(--rule); padding-top: 0.75rem; color: var(--faint); font-size: 0.8125rem; }
@media print { main { padding-top: 0; } .step { break-inside: avoid; } details { display: none; } }
`;

/** A check's rules: passed or not, with a few rows that break each. */
function checkResults(r: CheckReport): string {
  const items = r.results.map((x) => {
    const examples = x.examples.length
      ? `<div class="table"><table><thead><tr>${x.columns.map((c) => `<th><span>${esc(c)}</span></th>`).join("")}</tr></thead><tbody>${x.examples
          .map((e) => `<tr>${e.map((v) => `<td>${esc(v)}</td>`).join("")}</tr>`)
          .join("")}</tbody></table></div>`
      : "";
    return `<li class="${x.passed ? "passed" : "failed"}"><strong>${x.passed ? "Passed" : "Not met"}:</strong> ${esc(x.message)}${examples}</li>`;
  });
  const broken = r.results.filter((x) => !x.passed).length;
  const summary = broken ? `${broken} of ${r.results.length} checks not met` : `All ${r.results.length} checks passed`;
  return `<p>${summary}, over ${formatCount(r.rows)} rows.</p><ul class="checks">${items.join("")}</ul>`;
}

async function stepSection(ws: Workspace, s: StepView): Promise<string> {
  const parts = [`<section class="step"><h3>${esc(stepTitle(s))}</h3>`];
  parts.push(`<p class="reads">Reads ${s.inputs.map((i) => esc(ws.nameOf(i))).join(", ")}</p>`);
  if (s.state === "stale") parts.push(`<p class="stale">Outdated: produced before a change upstream.</p>`);
  if (s.state === "failed") parts.push(`<p class="stale">This step failed on the last run${s.detail ? `: ${esc(s.detail)}` : ""}.</p>`);
  if (s.mode === "chart" && s.state !== "not run") {
    try {
      parts.push(`<figure>${await chartSvg(await ws.backend.chart(s.id))}</figure>`);
    } catch {
      parts.push(`<p class="note">The chart could not be drawn.</p>`);
    }
  } else if (s.op.type === "check" && s.rows !== null) {
    // What the rules found matters here, not the rows passed on.
    const report = await ws.backend.checkReport(s.id).catch(() => null);
    if (report) parts.push(checkResults(report));
  } else if (s.rows !== null) {
    try {
      parts.push(table(await ws.backend.page(s.id, 0, REPORT_ROWS)));
    } catch {
      parts.push(`<p class="note">No result.</p>`);
    }
  } else if (s.state === "not run") {
    parts.push(`<p class="note">Not run yet.</p>`);
  }
  const code = s.op.type === "python" ? { lang: "Python", text: s.op.code } : s.op.type === "sql" ? { lang: "SQL", text: s.op.sql } : null;
  if (code) parts.push(`<details><summary>${code.lang}</summary><pre>${esc(code.text)}</pre></details>`);
  parts.push("</section>");
  return parts.join("\n");
}

export async function buildReport(ws: Workspace, now = new Date()): Promise<string> {
  const v = ws.view;
  if (!v) throw new Error("no project is open");
  const body: string[] = [`<h1>${esc(v.name)}</h1>`];
  const sources = v.sources.map((s) => `${esc(s.path)} (${formatCount(s.rows)} rows)`).join(", ");
  body.push(`<p class="meta">From ${sources}. Exported ${esc(now.toLocaleDateString("en-US", { dateStyle: "long" }))}.</p>`);
  if (v.parameters.length) {
    body.push(
      `<p class="meta">With ${v.parameters.map((p) => `<code>${esc(p.name)}</code> = ${esc(literalText(p.value))}`).join(", ")}.</p>`,
    );
  }
  for (const c of v.cells) {
    if (c.type === "markdown") body.push(renderMarkdown(c.text));
    else {
      const s = v.steps.find((x) => x.id === c.step);
      if (s) body.push(await stepSection(ws, s));
    }
  }
  body.push(`<footer>Made with the visual data analysis app. Tables show their first ${REPORT_ROWS} rows.</footer>`);
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(v.name)}</title>
<style>${STYLE}</style>
</head>
<body><main>
${body.join("\n")}
</main></body>
</html>
`;
}
