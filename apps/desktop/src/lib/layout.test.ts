import { describe, expect, it, beforeEach } from "vitest";
import { defaultSplit, paneKind, resultHeight, savedSidebar, saveSidebar, savedSplit, saveSplit } from "./layout";
import type { StepView } from "./api/types";

const step = (type: string) => ({ op: { type } }) as unknown as StepView;

describe("how the window is shared", () => {
  beforeEach(() => localStorage.clear());

  it("knows the kind of pane", () => {
    expect(paneKind(step("filter"), null)).toBe("visual");
    expect(paneKind(step("python"), null)).toBe("code");
    expect(paneKind(step("r"), null)).toBe("code");
    expect(paneKind(step("chart"), null)).toBe("chart");
    expect(paneKind(null, {} as never)).toBe("source");
  });

  it("fits small results and shares the space for large ones", () => {
    const four = resultHeight("visual", 4, false)!;
    expect(four).toBeGreaterThan(150);
    expect(resultHeight("visual", 4, true)).toBeGreaterThan(four); // the outdated banner
    expect(resultHeight("visual", 0, false)).toBe(resultHeight("visual", 2, false)); // room for "no rows"
    expect(resultHeight("visual", 500, false)).toBeNull();
    expect(resultHeight("chart", 4, false)).toBeNull();
    expect(resultHeight("visual", null, false)).toBeNull();
    expect(defaultSplit("code")).toBeGreaterThan(defaultSplit("visual"));
  });

  it("remembers what the user dragged to, per kind", () => {
    expect(savedSplit("code")).toBeNull();
    saveSplit("code", 0.7);
    expect(savedSplit("code")).toBe(0.7);
    expect(savedSplit("visual")).toBeNull();
    saveSplit("code", null);
    expect(savedSplit("code")).toBeNull();
    saveSidebar(320);
    expect(savedSidebar()).toBe(320);
  });
});
