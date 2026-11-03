<script lang="ts">
  import { X, Plus } from "@lucide/svelte";
  import type { AggFunc, Column, Measure } from "../api/types";
  import { funcWords, isNumericType } from "../format";
  import ColumnPicker from "./ColumnPicker.svelte";

  let {
    groupBy = $bindable(),
    measures = $bindable(),
    columns,
  }: { groupBy: string[]; measures: Measure[]; columns: Column[] | null } = $props();

  // "P" stands for any percentile; its number is chosen beside it.
  const funcs: { f: string; words: string }[] = [
    ...(["COUNT", "SUM", "AVG", "MIN", "MAX", "MEDIAN", "COUNT_DISTINCT", "STDDEV"] as const).map((f) => ({ f, words: funcWords(f) })),
    { f: "P", words: "Percentile" },
  ];
  const kind = (m: Measure) => (/^P\d+$/.test(m.func) ? "P" : m.func);
  const pct = (m: Measure) => (kind(m) === "P" ? Number(m.func.slice(1)) : 90);
  function setKind(m: Measure, k: string) {
    m.func = (k === "P" ? `P${pct(m)}` : k) as AggFunc;
    suggestAlias(m);
  }

  function toggle(name: string) {
    const i = groupBy.indexOf(name);
    if (i >= 0) groupBy.splice(i, 1);
    else groupBy.push(name);
  }

  function suggestAlias(m: Measure) {
    m.alias = m.func === "COUNT" && !m.column ? "rows" : `${m.func.toLowerCase()}_${m.column ?? ""}`;
  }

  // These need numbers; the core rejects anything else.
  const needsNumber = (m: Measure) => ["SUM", "AVG", "MEDIAN", "STDDEV", "P"].includes(kind(m));
</script>

<div class="space-y-4">
  <fieldset>
    <legend class="mb-2 text-sm text-ink-soft">Group by</legend>
    {#if columns}
      <div class="flex flex-wrap gap-1.5">
        {#each columns as c (c.name)}
          <button
            class="chip"
            aria-pressed={groupBy.includes(c.name)}
            onclick={() => toggle(c.name)}
          >
            {c.name}
          </button>
        {/each}
      </div>
      {#if !groupBy.length}<p class="mt-1 text-xs text-ink-faint">No groups: one summary row for everything.</p>{/if}
    {:else}
      <input
        class="field w-72"
        aria-label="Group by columns, separated by commas"
        value={groupBy.join(", ")}
        onchange={(e) => (groupBy = e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean))}
      />
    {/if}
  </fieldset>

  <fieldset class="space-y-2">
    <legend class="mb-2 text-sm text-ink-soft">Summaries</legend>
    {#each measures as m, i (i)}
      <div class="flex flex-wrap items-center gap-2">
        <select class="field" aria-label="Summary" value={kind(m)} onchange={(e) => setKind(m, e.currentTarget.value)}>
          {#each funcs as f (f.f)}<option value={f.f}>{f.words}</option>{/each}
        </select>
        {#if kind(m) === "P"}
          <input
            class="field w-16"
            type="number"
            min="1"
            max="99"
            aria-label="Percentile"
            value={pct(m)}
            onchange={(e) => {
              const n = Math.min(99, Math.max(1, Math.round(Number(e.currentTarget.value) || 90)));
              m.func = `P${n}` as AggFunc;
              suggestAlias(m);
            }}
          />
        {/if}
        <span class="text-ink-faint">of</span>
        <ColumnPicker
          bind:value={m.column}
          {columns}
          label="Summarized column"
          allowNone={m.func === "COUNT" ? "all rows" : null}
          filter={needsNumber(m) ? (c) => isNumericType(c.type) : undefined}
        />
        <span class="text-ink-faint">as</span>
        <input class="field w-36" aria-label="Result column name" bind:value={m.alias} />
        <button class="btn btn-quiet !px-1.5" aria-label="Remove summary" onclick={() => measures.splice(i, 1)}>
          <X size={15} />
        </button>
      </div>
    {/each}
    <button
      class="btn btn-quiet !px-2"
      onclick={() => measures.push({ func: "COUNT", column: null, alias: `rows_${measures.length + 1}` })}
    >
      <Plus size={15} /> Add summary
    </button>
  </fieldset>
</div>

<style>
  .chip {
    border: 1px solid var(--color-rule-strong);
    border-radius: 999px;
    padding: 0.15rem 0.65rem;
    background: var(--color-panel);
    cursor: pointer;
  }
  .chip[aria-pressed="true"] {
    background: var(--color-ink);
    border-color: var(--color-ink);
    color: white;
  }
</style>
