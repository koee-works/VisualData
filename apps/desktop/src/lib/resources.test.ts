import { afterEach, describe, expect, it, vi } from "vitest";
import { MockBackend } from "./api/mock";
import { resources } from "./resources.svelte";
import { formatBytes } from "./format";

describe("resources", () => {
  afterEach(() => vi.useRealTimers());

  it("reads at once, keeps reading, and stops when told", async () => {
    vi.useFakeTimers();
    const backend = new MockBackend();
    const spy = vi.spyOn(backend, "resources");
    const stop = resources.watch(backend, 1000);
    await vi.advanceTimersByTimeAsync(0);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(resources.value?.parts[0]?.name).toBe("App");
    await vi.advanceTimersByTimeAsync(2100);
    expect(spy).toHaveBeenCalledTimes(3);
    stop();
    await vi.advanceTimersByTimeAsync(5000);
    expect(spy).toHaveBeenCalledTimes(3);
  });

  it("marks a failed reading as stale and recovers", async () => {
    vi.useFakeTimers();
    const backend = new MockBackend();
    let fail = true;
    const real = backend.resources.bind(backend);
    vi.spyOn(backend, "resources").mockImplementation(() => (fail ? Promise.reject(new Error("busy")) : real()));
    const stop = resources.watch(backend, 1000);
    await vi.advanceTimersByTimeAsync(0);
    expect(resources.stale).toBe(true);
    fail = false;
    await vi.advanceTimersByTimeAsync(1100);
    expect(resources.stale).toBe(false);
    stop();
  });
});

describe("formatBytes", () => {
  it("writes sizes people read", () => {
    expect(formatBytes(100_000)).toBe("<1 MB");
    expect(formatBytes(820 * 1_048_576)).toBe("820 MB");
    expect(formatBytes(1.4 * 1024 * 1_048_576)).toBe("1.4 GB");
    expect(formatBytes(16 * 1024 * 1_048_576)).toBe("16 GB");
    expect(formatBytes(4 * 1024 * 1_048_576)).toBe("4 GB");
  });
});
