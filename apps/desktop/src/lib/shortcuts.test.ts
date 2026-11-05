import { describe, expect, it } from "vitest";
import { combo, shortcutGroups } from "./shortcuts";

describe("shortcuts", () => {
  it("writes combinations the way each computer does", () => {
    expect(combo({ key: "K", mod: true }, false)).toBe("Ctrl+K");
    expect(combo({ key: "P", mod: true, shift: true }, false)).toBe("Ctrl+Shift+P");
    expect(combo({ key: "P", mod: true, shift: true }, true)).toBe("⇧⌘P");
    expect(combo({ key: "-", alt: true }, true)).toBe("⌥-");
    expect(combo({ key: "Enter", shift: true }, false)).toBe("Shift+Enter");
  });

  it("lists the same actions on every computer, with its own keys", () => {
    const win = shortcutGroups(false);
    const mac = shortcutGroups(true);
    expect(mac.map((g) => g.name)).toEqual(win.map((g) => g.name));
    const keys = (gs: typeof win) => gs.flatMap((g) => g.items.map((i) => i.keys)).join(" | ");
    expect(keys(win)).toContain("Ctrl+K or Ctrl+Shift+P");
    expect(keys(mac)).toContain("⌘K or ⇧⌘P");
    expect(keys(win)).toContain("Ctrl+Y or Ctrl+Shift+Z");
    expect(keys(mac)).not.toContain("Ctrl+");
  });

  it("describes every shortcut once", () => {
    const all = shortcutGroups(false).flatMap((g) => g.items);
    expect(all.length).toBeGreaterThan(10);
    expect(new Set(all.map((i) => i.what)).size).toBe(all.length);
    expect(all.every((i) => i.keys && i.what)).toBe(true);
  });
});
