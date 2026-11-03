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
    expect(a?.input).toBe("filter_1");
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
