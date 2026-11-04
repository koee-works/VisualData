import { describe as group, expect, it } from "vitest";
import examples from "./describe-examples.json";
import { describe } from "./format";
import type { StepOp } from "./api/types";

// The same examples are checked in Rust (crates/vda-core/src/describe.rs),
// so history and the app name steps alike.
group("steps are described the same way everywhere", () => {
  for (const e of examples) {
    it(e.words, () => expect(describe(e.op as unknown as StepOp)).toBe(e.words));
  }
});
