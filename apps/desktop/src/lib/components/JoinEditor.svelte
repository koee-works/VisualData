<script lang="ts">
  import { Plus, X } from "@lucide/svelte";
  import type { CmpOp, Column, JoinHow, JoinKey } from "../api/types";
  import { symbols, typeLabel } from "../format";
  import ColumnPicker from "./ColumnPicker.svelte";

  let {
    how = $bindable(),
    on = $bindable(),
    right = $bindable(),
    leftName,
    leftColumns,
    rightOptions,
    rightColumns,
    rightId,
  }: {
    how: JoinHow;
    on: JoinKey[];
    /** Id of the second input. */
    right: string;
    leftName: string;
    leftColumns: Column[] | null;
    rightOptions: { id: string; name: string }[];
    rightColumns: Column[] | null;
    /** The second input's id, used in renamed columns. */
    rightId: string;
  } = $props();

  let rightName = $derived(rightOptions.find((o) => o.id === right)?.name ?? right);

  const kinds: { value: JoinHow; label: (l: string, r: string) => string; hint: string }[] = [
    { value: "inner", label: () => "Only rows that match", hint: "Rows without a partner on the other side are left out." },
    { value: "left", label: (l) => `Every row of ${l}`, hint: "Where nothing matches, the other side's columns are missing." },
    { value: "full", label: () => "Every row of both", hint: "Unmatched rows from either side are kept, with the other side missing." },
  ];

  function typeOf(cols: Column[] | null, name: string) {
    return cols?.find((c) => c.name === name)?.type ?? null;
  }

  const ops: { value: CmpOp; words: string }[] = [
    { value: "=", words: "equals" },
    { value: "<>", words: "differs from" },
    { value: "<", words: "is before / less than" },
    { value: "<=", words: "is on or before / at most" },
    { value: ">", words: "is after / more than" },
    { value: ">=", words: "is on or after / at least" },
  ];

  // Mirrors the core's rule: same type, both numbers, or both dates/times.
  function mismatch(k: JoinKey): string | null {
    const a = typeOf(leftColumns, k.left);
    const b = typeOf(rightColumns, k.right);
    if (!a || !b || a === b) return null;
    const numeric = (t: string) => t === "BIGINT" || t === "DOUBLE";
    const temporal = (t: string) => t === "DATE" || t === "TIMESTAMP";
    if ((numeric(a) && numeric(b)) || (temporal(a) && temporal(b))) return null;
    return `A ${typeLabel[a]} column can't be compared with a ${typeLabel[b]} column.`;
  }

  // `=` is the default and is not stored, so an untouched join stays unchanged.
  function setOp(k: JoinKey, value: CmpOp) {
    if (value === "=") delete k.op;
    else k.op = value;
    on = [...on];
  }

  let clashes = $derived.by(() => {
    if (!leftColumns || !rightColumns) return [];
    const keys = new Set(on.filter((k) => (k.op ?? "=") === "=").map((k) => k.right));
    const left = new Set(leftColumns.map((c) => c.name));
    return rightColumns.filter((c) => !keys.has(c.name) && left.has(c.name)).map((c) => c.name);
  });
</script>

<div class="space-y-5 text-sm">
  <label class="flex flex-wrap items-center gap-2">
    <span class="text-ink-soft">Combine {leftName} with</span>
    <select class="field min-w-48" aria-label="Table to join with" bind:value={right}>
      {#each rightOptions as o (o.id)}
        <option value={o.id}>{o.name}</option>
      {/each}
    </select>
  </label>

  <fieldset>
    <legend class="mb-1.5 font-semibold">Keep</legend>
    <div class="grid gap-1.5 sm:grid-cols-3" role="radiogroup">
      {#each kinds as k (k.value)}
        <label
          class="flex cursor-pointer gap-2 rounded-md border px-3 py-2 has-[:checked]:border-running has-[:checked]:bg-running-wash"
          class:border-rule-strong={how !== k.value}
        >
          <input type="radio" class="mt-0.5 accent-[var(--color-running)]" name="join-how" value={k.value} bind:group={how} />
          <span>
            <span class="block font-semibold">{k.label(leftName, rightName)}</span>
            <span class="block text-xs text-ink-soft">{k.hint}</span>
          </span>
        </label>
      {/each}
    </div>
  </fieldset>

  <fieldset>
    <legend class="mb-1.5 font-semibold">Match rows where</legend>
    <p class="mb-2 max-w-[64ch] text-ink-soft">
      Every condition must hold. For a range, use two: a date on or after a start, and on or before an end.
    </p>
    <ul class="space-y-2">
      {#each on as key, i (i)}
        {@const problem = mismatch(key)}
        <li>
          <div class="flex flex-wrap items-center gap-2">
            <ColumnPicker bind:value={key.left} columns={leftColumns} label="Column of {leftName}" />
            <select
              class="field"
              aria-label="Comparison"
              value={key.op ?? "="}
              onchange={(e) => setOp(key, e.currentTarget.value as CmpOp)}
            >
              {#each ops as o (o.value)}<option value={o.value}>{symbols[o.value]} {o.words}</option>{/each}
            </select>
            <ColumnPicker bind:value={key.right} columns={rightColumns} label="Column of {rightName}" />
            <button
              class="btn btn-quiet !px-1.5"
              aria-label="Remove this pair"
              disabled={on.length === 1}
              onclick={() => (on = on.filter((_, j) => j !== i))}
            >
              <X size={14} />
            </button>
          </div>
          {#if problem}<p class="mt-1 text-failed" role="alert">{problem}</p>{/if}
        </li>
      {/each}
    </ul>
    <button
      class="btn btn-quiet mt-2 !px-2"
      onclick={() => (on = [...on, { left: leftColumns?.[0]?.name ?? "", right: rightColumns?.[0]?.name ?? "" }])}
    >
      <Plus size={14} /> Add another pair
    </button>
  </fieldset>

  <p class="max-w-[64ch] text-ink-soft">
    The result has {leftName}'s columns, then {rightName}'s other columns (columns matched with = appear once).
    Missing values never match.
    {#if clashes.length}
      Both have {clashes.map((c) => `"${c}"`).join(", ")}, so {rightName}'s
      {clashes.length === 1 ? "copy is" : "copies are"} renamed {clashes.map((c) => `"${c}_${rightId}"`).join(", ")}.
    {/if}
    {#if !rightColumns}
      Columns of {rightName} appear after it runs; type names for now.
    {/if}
  </p>
</div>
