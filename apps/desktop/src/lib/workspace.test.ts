import { describe, expect, it } from "vitest";
import { flushSync } from "svelte";
import { MockBackend } from "./api/mock";
import { Workspace } from "./workspace.svelte";
import type { StepOp } from "./api/types";

const filter: StepOp = {
  type: "filter",
  predicates: [{ column: "region", op: "IS NOT NULL", value: null }],
};
const aggregate: StepOp = {
  type: "aggregate",
  group_by: ["region"],
  measures: [{ func: "COUNT", column: null, alias: "orders" }],
};

async function workspace() {
  const ws = new Workspace(new MockBackend());
  await ws.open("/projects/x");
  return ws;
}

describe("Workspace", () => {
  it("adds steps reading the selected node and selects the new step", async () => {
    const ws = await workspace();
    const f = await ws.addStep(filter);
    expect(f?.id).toBe("filter_1");
    expect(ws.selected).toBe("filter_1");
    const a = await ws.addStep(aggregate);
    expect(a?.inputs).toEqual(["filter_1"]);
    flushSync();
    expect(ws.ordered.map((s) => s.id)).toEqual(["filter_1", "aggregate_1"]);
    expect(ws.depth["aggregate_1"]).toBe(2);
  });

  it("runs with per-step progress, then marks downstream results outdated after an edit", async () => {
    const ws = await workspace();
    await ws.addStep(filter);
    await ws.addStep(aggregate);
    const seen: string[] = [];
    const run = ws.run();
    expect(ws.running).toBe(true);
    await run;
    for (const id of Object.keys(ws.progress)) seen.push(id);
    expect(seen).toEqual(["filter_1", "aggregate_1"]);
    expect(ws.running).toBe(false);
    expect(ws.view!.steps.map((s) => s.state)).toEqual(["fresh", "fresh"]);

    await ws.saveStep("filter_1", "Only East", {
      type: "filter",
      predicates: [{ column: "region", op: "=", value: "East" }],
    });
    expect(ws.view!.steps.map((s) => s.state)).toEqual(["stale", "stale"]);
    const page = await ws.backend.page("aggregate_1", 0, 10);
    expect(page.stale).toBe(true);

    await ws.run();
    expect(ws.view!.steps.map((s) => s.state)).toEqual(["fresh", "fresh"]);
    const after = await ws.backend.page("aggregate_1", 0, 10);
    expect(after.rows).toHaveLength(1);
  });

  it("reports failures with the action and keeps the analysis", async () => {
    const ws = await workspace();
    await ws.addStep(filter);
    await ws.addStep(aggregate);
    ws.selected = "filter_1";
    await ws.removeStep("filter_1");
    expect(ws.error?.action).toBe("Delete step");
    expect(ws.error?.message).toContain("aggregate_1");
    expect(ws.view!.steps).toHaveLength(2);

    ws.selected = "aggregate_1";
    await ws.addStep({ type: "python", code: "raise ValueError()", timeout_ms: 1000 });
    await ws.run();
    const py = ws.view!.steps.find((s) => s.id === "python_1")!;
    expect(py.state).toBe("failed");
    expect(py.detail).toContain("ValueError");
    expect(ws.progress["python_1"]?.status).toBe("failed");
  });

  it("moves selection to the input when a step is deleted", async () => {
    const ws = await workspace();
    await ws.addStep(filter);
    await ws.addStep(aggregate);
    await ws.removeStep("aggregate_1");
    expect(ws.selected).toBe("filter_1");
    expect(ws.error).toBeNull();
  });

  it("cancels a run in progress", async () => {
    const backend = new MockBackend();
    backend.stepDelay = 20;
    const ws = new Workspace(backend);
    await ws.open("/p");
    await ws.addStep(filter);
    await ws.addStep(aggregate);
    const run = ws.run();
    await ws.cancel();
    await run;
    expect(Object.values(ws.progress).every((e) => e.status === "skipped")).toBe(true);
    expect(ws.view!.steps.every((s) => s.state === "not run")).toBe(true);
  });
});

describe("Code review (ADR-0004)", () => {
  async function received() {
    const mock = new MockBackend();
    await mock.seedDemo({ run: false, editAfterRun: false, received: true });
    const ws = new Workspace(mock);
    await ws.open("/projects/from-dana");
    return { ws, mock };
  }

  it("opens the review dialog when a project has unapproved Python", async () => {
    const { ws } = await received();
    expect(ws.reviewOpen).toBe(true);
    expect(ws.pendingReview.map((r) => r.step_id)).toEqual(["python_1"]);
    expect(ws.pendingReview[0]!.code).toContain("share=");
    expect(ws.view!.steps.find((s) => s.id === "python_1")!.state).toBe("needs review");
    expect(ws.view!.steps.find((s) => s.id === "chart_1")!.state).toBe("waiting on review");
  });

  it("does not run unapproved code or anything after it", async () => {
    const { ws } = await received();
    ws.reviewOpen = false;
    await ws.run();
    expect(ws.progress["aggregate_1"]!.status).toBe("succeeded");
    expect(ws.progress["python_1"]!.status).toBe("needs review");
    expect(ws.progress["chart_1"]!.status).toBe("skipped");
    expect(ws.progress["chart_1"]!.detail).toContain("waiting for your review");
  });

  it("runs once approved and closes the dialog", async () => {
    const { ws } = await received();
    await ws.approve(["python_1"]);
    expect(ws.reviewOpen).toBe(false);
    expect(ws.pendingReview).toEqual([]);
    await ws.run();
    expect(Object.values(ws.progress).every((e) => e.status === "succeeded")).toBe(true);
  });

  it("treats code written in the app as approved, but a label change as not", async () => {
    const { ws } = await received();
    const step = ws.view!.steps.find((s) => s.id === "python_1")!;
    await ws.saveStep("python_1", "Renamed", step.op);
    expect(ws.view!.steps.find((s) => s.id === "python_1")!.state).toBe("needs review");
    await ws.saveStep("python_1", "Renamed", { type: "python", code: "result = df", timeout_ms: 60000 });
    expect(ws.pendingReview).toEqual([]);
  });

  it("trusts all Python for the session, and asks again when switched off", async () => {
    const { ws, mock } = await received();
    await ws.setTrustAll(true);
    expect(ws.reviewOpen).toBe(false);
    expect(ws.view!.trust_all).toBe(true);
    expect(ws.pendingReview).toEqual([]);
    await ws.run();
    expect(Object.values(ws.progress).every((e) => e.status === "succeeded")).toBe(true);
    expect(mock.approved.size).toBe(0);
    await ws.setTrustAll(false);
    expect(ws.pendingReview.map((r) => r.step_id)).toEqual(["python_1"]);
    expect(ws.reviewOpen).toBe(true);
  });

  it("does not open the dialog for the user's own project", async () => {
    const mock = new MockBackend();
    await mock.seedDemo({ run: false, editAfterRun: false });
    const ws = new Workspace(mock);
    await ws.open("/projects/mine");
    expect(ws.reviewOpen).toBe(false);
  });
});

describe("Several sources and joins", () => {
  async function joined() {
    const mock = new MockBackend();
    await mock.seedDemo({ run: false, editAfterRun: false, join: true });
    const ws = new Workspace(mock);
    await ws.open("/projects/x");
    return { ws, mock };
  }

  it("shows one tree per source, with the join under its main input", async () => {
    const { ws } = await joined();
    flushSync();
    expect(ws.tree.map((t) => t.source.id)).toEqual(["source", "regions"]);
    expect(ws.tree[0]!.steps.map((r) => r.step.id)).toEqual(["filter_1", "aggregate_1", "python_1", "chart_1", "join_1"]);
    expect(ws.tree[1]!.steps).toEqual([]);
    expect(ws.nameOf("regions")).toBe("regions.csv");
    expect(ws.view!.steps.find((s) => s.id === "join_1")!.inputs).toEqual(["aggregate_1", "regions"]);
  });

  it("runs a full join: every region, including one without orders", async () => {
    const { ws } = await joined();
    await ws.run();
    expect(ws.progress["join_1"]!.status).toBe("succeeded");
    const page = await ws.backend.page("join_1", 0, 50);
    expect(page.columns.map((c) => c.name)).toEqual(["region", "orders", "revenue", "manager", "target"]);
    const regions = page.rows.map((r) => (r[0]!.t === "text" ? r[0]!.v : null));
    expect(regions).toContain("Central");
  });

  it("adds a source from a file and selects it", async () => {
    const ws = new Workspace(new MockBackend());
    await ws.open("/projects/x");
    await ws.addSource();
    expect(ws.view!.sources.map((s) => s.id)).toEqual(["source", "regions"]);
    expect(ws.selected).toBe("regions");
    expect(ws.selectedSource?.columns.map((c) => c.name)).toEqual(["region", "manager", "target"]);
  });

  it("offers inputs that would not create a cycle", async () => {
    const { ws } = await joined();
    flushSync();
    const forFilter = ws.readable("filter_1").map((o) => o.id);
    expect(forFilter).toEqual(["source", "regions"]);
    const forJoin = ws.readable("join_1").map((o) => o.id);
    expect(forJoin).toContain("aggregate_1");
    expect(forJoin).not.toContain("join_1");
  });

  it("changes a join's second input and keeps a source that is read", async () => {
    const { ws } = await joined();
    const j = ws.view!.steps.find((s) => s.id === "join_1")!;
    expect(await ws.saveStep("join_1", null, j.op, ["aggregate_1", "filter_1"])).toBe(true);
    expect(ws.view!.steps.find((s) => s.id === "join_1")!.inputs).toEqual(["aggregate_1", "filter_1"]);
    await ws.removeSource("regions");
    expect(ws.view!.sources.map((s) => s.id)).toEqual(["source"]);
  });

  it("explains why a source cannot be removed", async () => {
    const { ws } = await joined();
    await ws.removeSource("regions");
    expect(ws.error?.message).toContain("join_1");
    expect(ws.view!.sources).toHaveLength(2);
  });
});

describe("Append, comparison joins and source settings", () => {
  async function joined() {
    const mock = new MockBackend();
    await mock.seedDemo({ run: false, editAfterRun: false, join: true });
    const ws = new Workspace(mock);
    await ws.open("/projects/x");
    return { ws, mock };
  }

  it("appends tables by column name and records where each row came from", async () => {
    const { ws } = await joined();
    ws.selected = "regions";
    await ws.addStep({ type: "append", distinct: false, origin: "from" }, null, ["source"]);
    await ws.run();
    const page = await ws.backend.page("append_1", 0, 1000);
    expect(page.columns.map((c) => c.name)).toEqual(["region", "manager", "target", "order_id", "order_date", "revenue", "from"]);
    expect(page.total_rows).toBe(5 + 240);
    expect(page.rows[0]![6]).toEqual({ t: "text", v: "regions" });
    expect(page.rows[5]![1]).toEqual({ t: "null" });
  });

  it("joins on a comparison", async () => {
    const { ws } = await joined();
    const j = ws.view!.steps.find((s) => s.id === "join_1")!;
    await ws.saveStep("join_1", null, { type: "join", how: "inner", on: [{ left: "revenue", op: ">=", right: "target" }] }, j.inputs);
    await ws.run();
    expect(ws.progress["join_1"]!.status).toBe("succeeded");
    const page = await ws.backend.page("join_1", 0, 100);
    expect(page.columns.map((c) => c.name)).toContain("target");
  });

  it("renames a source everywhere it is read and keeps it selected", async () => {
    const { ws } = await joined();
    ws.selected = "regions";
    expect(await ws.updateSource("regions", "targets", null)).toBe(true);
    expect(ws.selected).toBe("targets");
    expect(ws.view!.steps.find((s) => s.id === "join_1")!.inputs).toEqual(["aggregate_1", "targets"]);
    expect(await ws.updateSource("targets", "aggregate_1", null)).toBe(false);
    expect(ws.error?.message).toContain("already used");
  });
});

describe("Core transforms and export", () => {
  async function seeded() {
    const mock = new MockBackend();
    await mock.seedDemo({ run: false, editAfterRun: false });
    const ws = new Workspace(mock);
    await ws.open("/projects/x");
    return { ws, mock };
  }

  it("selects, renames and reorders columns", async () => {
    const { ws } = await seeded();
    ws.selected = "source";
    await ws.addStep({ type: "select", columns: [{ column: "revenue", as: "amount" }, { column: "region", as: null }] });
    await ws.run();
    const page = await ws.backend.page("select_1", 0, 5);
    expect(page.columns.map((c) => c.name)).toEqual(["amount", "region"]);
  });

  it("sorts with missing values last and keeps the top rows", async () => {
    const { ws } = await seeded();
    ws.selected = "source";
    await ws.addStep({ type: "sort", by: [{ column: "revenue", descending: true }], limit: 3 });
    await ws.run();
    const page = await ws.backend.page("sort_1", 0, 10);
    expect(page.total_rows).toBe(3);
    const vals = page.rows.map((r) => (r[3]!.t === "float" ? r[3]!.v : null));
    expect(vals).toEqual([...vals].sort((a, b) => b! - a!));
  });

  it("describes the new steps in words", async () => {
    const { describe: d } = await import("./format");
    expect(d({ type: "sort", by: [{ column: "revenue", descending: true }], limit: 10 })).toBe("Sort by revenue (highest first), first 10");
    expect(d({ type: "derive", columns: [{ name: "total", formula: "a * b" }] })).toBe("Calculate total");
    expect(d({ type: "aggregate", group_by: ["region"], measures: [{ func: "P90", column: "revenue", alias: "p" }] })).toBe(
      "90th percentile of revenue by region",
    );
  });

  it("exports a current result and refuses an outdated one", async () => {
    const { ws, mock } = await seeded();
    await ws.run();
    await ws.exportResult("aggregate_1", "xlsx");
    expect(mock.exports).toEqual([["aggregate_1", "/exports/Revenue by region.xlsx"]]);
    expect(ws.notice).toContain("Exported 4 rows");
    const f = ws.view!.steps.find((s) => s.id === "filter_1")!;
    if (f.op.type === "filter") await ws.saveStep("filter_1", f.label, { ...f.op, predicates: [] });
    await ws.exportResult("aggregate_1", "csv");
    expect(ws.error?.message).toContain("outdated");
  });
});

describe("Reshaping and window calculations", () => {
  async function fromSource() {
    const mock = new MockBackend();
    await mock.seedDemo({ run: false, editAfterRun: false });
    const ws = new Workspace(mock);
    await ws.open("/projects/x");
    ws.selected = "source";
    return ws;
  }

  it("adds a running total per region that keeps rows in place", async () => {
    const ws = await fromSource();
    await ws.addStep({
      type: "window",
      partition_by: ["region"],
      order_by: [{ column: "order_date", descending: false }],
      columns: [{ name: "running_revenue", func: "running_sum", column: "revenue", n: null }],
    });
    await ws.run();
    const page = await ws.backend.page("window_1", 0, 3);
    expect(page.columns.map((c) => c.name)).toEqual(["order_id", "order_date", "region", "revenue", "running_revenue"]);
    expect(page.rows.map((r) => r[0]!.t === "int" && r[0]!.v)).toEqual(["1", "2", "3"]);
  });

  it("keeps one row per key, in input order", async () => {
    const ws = await fromSource();
    await ws.addStep({ type: "dedupe", keys: ["region"], order_by: [{ column: "revenue", descending: true }] });
    await ws.run();
    const page = await ws.backend.page("dedupe_1", 0, 10);
    expect(page.total_rows).toBe(5); // four regions and the missing one
  });

  it("pivots values into columns and back into rows", async () => {
    const ws = await fromSource();
    const p = await ws.addStep({ type: "pivot", rows: [], column: "region", value: null, func: "COUNT" });
    await ws.run();
    const wide = await ws.backend.page(p!.id, 0, 5);
    expect(wide.total_rows).toBe(1);
    expect(wide.columns.map((c) => c.name)).toEqual(["East", "North", "South", "West"]);
    ws.selected = p!.id;
    await ws.addStep({ type: "unpivot", columns: ["East", "West"], name: "region", value: "orders" });
    await ws.run();
    const long = await ws.backend.page("unpivot_1", 0, 5);
    expect(long.columns.map((c) => c.name)).toEqual(["North", "South", "region", "orders"]);
    expect(long.total_rows).toBe(2);
  });

  it("describes the steps in words", async () => {
    const { describe: d } = await import("./format");
    expect(
      d({ type: "window", partition_by: ["region"], order_by: [], columns: [{ name: "r", func: "running_sum", column: "revenue", n: null }] }),
    ).toBe("Running total of revenue per region");
    expect(d({ type: "window", partition_by: [], order_by: [], columns: [{ name: "r", func: "rank", column: null, n: null }] })).toBe("Rank");
    expect(d({ type: "dedupe", keys: [], order_by: [] })).toBe("Remove repeated rows");
    expect(d({ type: "dedupe", keys: ["customer"], order_by: [] })).toBe("One row per customer");
    expect(d({ type: "pivot", rows: ["region"], column: "month", value: "revenue", func: "SUM" })).toBe("Columns for each month, rows by region");
    expect(d({ type: "unpivot", columns: ["q1", "q2"], name: "quarter", value: "v" })).toBe("Turn q1, q2 into rows");
  });
});

describe("Console", () => {
  async function seeded() {
    const mock = new MockBackend();
    await mock.seedDemo({ run: true, editAfterRun: false });
    const ws = new Workspace(mock);
    await ws.open("/projects/x");
    await ws.refreshProblems();
    return { ws, mock };
  }

  it("lists problems after edits, errors first, and finds them per step", async () => {
    const { ws } = await seeded();
    expect(ws.problems.items).toEqual([]);
    ws.selected = "aggregate_1";
    await ws.addStep({ type: "python", code: "for _, r in df.iterrows():\n    pass", timeout_ms: 60000 });
    await ws.refreshProblems();
    expect(ws.problems.items.map((p) => [p.severity, p.code])).toEqual([
      ["error", "python-result"],
      ["warning", "row-loop"],
    ]);
    expect(ws.counts.error).toBe(1);
    expect(ws.problemsFor("python_2").map((p) => p.code)).toContain("row-loop");
  });

  it("dismisses a warning and shows it again", async () => {
    const { ws } = await seeded();
    ws.selected = "aggregate_1";
    await ws.addStep({ type: "python", code: "for _, r in df.iterrows():\n    pass\nresult = df", timeout_ms: 60000 });
    await ws.refreshProblems();
    const loop = ws.problems.items.find((p) => p.code === "row-loop")!;
    await ws.dismiss(loop);
    expect(ws.problems.items.some((p) => p.code === "row-loop")).toBe(false);
    expect(ws.problems.dismissed).toBe(1);
    await ws.restoreDismissed();
    expect(ws.problems.items.some((p) => p.code === "row-loop")).toBe(true);
  });

  it("reveals the step a problem is about", async () => {
    const { ws } = await seeded();
    ws.selected = "source";
    ws.centerTab = "notebook";
    ws.reveal({ severity: "error", code: "formula", step: "filter_1", field: null, line: null, column: null, message: "", from_last_run: false, dismissable: false });
    expect([ws.selected, ws.centerTab]).toEqual(["filter_1", "step"]);
  });

  it("keeps runs in the log, newest first, with what steps printed", async () => {
    const { ws, mock } = await seeded();
    const py = ws.view!.steps.find((s) => s.id === "python_1")!;
    if (py.op.type === "python") await ws.saveStep("python_1", py.label, { ...py.op, code: `print("hi")\n${py.op.code}` });
    await ws.run();
    const log = await mock.runLog(5);
    expect(log.length).toBe(2);
    expect(log[0]!.steps.find((s) => s.step_id === "python_1")!.output).toContain("printed by python_1");
    expect(ws.runs).toBe(1);
  });
});
