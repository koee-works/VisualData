<script lang="ts">
  import type { Column, TestKind } from "../api/types";
  import { isNumericType, testWords } from "../format";
  import ColumnPicker from "./ColumnPicker.svelte";

  let {
    test = $bindable(),
    a = $bindable(),
    b = $bindable(),
    columns,
  }: { test: TestKind; a: string; b: string; columns: Column[] | null } = $props();

  const kinds = Object.keys(testWords) as TestKind[];
  const numeric = (c: Column) => isNumericType(c.type);
  // Categories: anything but decimals and timestamps (group those first).
  const category = (c: Column) => c.type !== "DOUBLE" && c.type !== "TIMESTAMP";
  let aNumeric = $derived(test !== "independence");
  let bNumeric = $derived(test === "correlation");

  const explain: Record<TestKind, string> = {
    compare_means:
      "Whether the average differs between two groups more than chance would explain. The groups may have different spreads (Welch's t-test). Filter to two groups first if there are more.",
    compare_groups: "Whether the average differs across two or more groups more than chance would explain (one-way analysis of variance).",
    independence:
      "Whether the mix of one category depends on the other, from how often each pair occurs (chi-square test). Counts below 5 make the answer rough.",
    correlation: "How closely two numbers move together in a straight line, from −1 to 1 (Pearson's r), and whether that could be chance.",
  };
</script>

<div class="space-y-4 text-sm">
  <fieldset>
    <legend class="mb-1.5 font-semibold">Test</legend>
    <div class="grid gap-1.5 sm:grid-cols-2">
      {#each kinds as k (k)}
        <label class="flex cursor-pointer items-start gap-2 rounded-md border px-3 py-2 {test === k ? 'border-ink bg-select' : 'border-rule'}">
          <input type="radio" class="mt-1 accent-[var(--color-running)]" name="test-kind" value={k} bind:group={test} />
          <span>
            <span class="block font-semibold">{testWords[k].name}</span>
            <span class="block text-xs text-ink-soft">{testWords[k].hint}</span>
          </span>
        </label>
      {/each}
    </div>
  </fieldset>
  <div class="flex flex-wrap items-center gap-2">
    <span class="text-ink-soft">{testWords[test].a}</span>
    <ColumnPicker bind:value={() => a, (v) => (a = v ?? "")} {columns} label="First column" filter={aNumeric ? numeric : category} />
    <span class="text-ink-soft">{testWords[test].b}</span>
    <ColumnPicker bind:value={() => b, (v) => (b = v ?? "")} {columns} label="Second column" filter={bNumeric ? numeric : category} />
  </div>
  <p class="max-w-[64ch] text-ink-soft">
    {explain[test]} Rows missing either value are left out. The result is one row: the statistic, the p-value and a sentence
    reading it at the usual 5% level.
  </p>
</div>
