import { describe, expect, it } from "vitest";
import { filter, score, type Command } from "./commands";

const cmd = (id: string, title: string, keywords?: string): Command => ({ id, group: "Actions", title, keywords, run: () => {} });
const all = [
  cmd("run", "Run analysis"),
  cmd("pipe", "Hide pipeline", "toggle panel"),
  cmd("theme", "Theme: Night", "dark"),
  cmd("sched", "Refresh on a schedule"),
  cmd("set", "Settings: Code editor", "vim"),
];

describe("command palette", () => {
  it("needs the letters in order", () => {
    expect(score("rna", "Run analysis")).not.toBeNull();
    expect(score("anr", "Run analysis")).toBeNull();
  });

  it("ranks word starts and prefixes first", () => {
    expect(filter(all, "run")[0]!.id).toBe("run");
    expect(filter(all, "hp")[0]!.id).toBe("pipe");
  });

  it("finds commands by their hidden words", () => {
    expect(filter(all, "vim").map((c) => c.id)).toEqual(["set"]);
    expect(filter(all, "dark")[0]!.id).toBe("theme");
  });

  it("puts recent commands first when nothing is typed", () => {
    expect(filter(all, "", ["sched", "theme"]).map((c) => c.id).slice(0, 3)).toEqual(["sched", "theme", "run"]);
  });

  it("matches nothing for letters that aren't there", () => {
    expect(filter(all, "zzz")).toEqual([]);
  });
});
