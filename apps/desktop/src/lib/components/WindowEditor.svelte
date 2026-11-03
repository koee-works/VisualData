<script lang="ts">
  import { Plus, X } from "@lucide/svelte";
  import type { Column, SortKey, WindowColumn, WindowFunc } from "../api/types";
  import { isNumericType, windowWords } from "../format";
  import ColumnChips from "./ColumnChips.svelte";
  import ColumnPicker from "./ColumnPicker.svelte";
  import OrderByList from "./OrderByList.svelte";

  let {
    partitionBy = $bindable(),
    orderBy = $bindable(),
    calcs = $bindable(),
    columns,
  }: { partitionBy: string[]; orderBy: SortKey[]; calcs: WindowColumn[]; columns: Column[] | null } = $props();

  const funcs = Object.keys(windowWords) as WindowFunc[];
  const usesColumn = (f: WindowFunc) => f !== "row_number" && f !== "rank";
  const needsNumber = (f: WindowFunc) => !["row_number", "rank", "previous", "next"].includes(f);
  const usesN = (f: WindowFunc) => f === "previous" || f === "next" || f === "moving_average";
  const nWords: Partial<Record<WindowFunc, [string, string]>> = {
    previous: ["", "rows back"],
    next: ["", "rows ahead"],
    moving_average: ["over", "rows"],
  };
  const short: Record<WindowFunc, string> = {
    running_sum: "running",
    row_number: "row",
    rank: "rank",
    previous: "previous",
    next: "next",
    moving_average: "average",
    change: "change",
    percent_change: "pct_change",
    share_of_total: "share",
  };
  const firstNumber = () => columns?.find((c) => isNumericType(c.type))?.name ?? columns?.[0]?.name ?? "";

  function suggest(c: WindowColumn) {
    c.name = c.column && usesColumn(c.func) ? `${short[c.func]}_${c.column}` : short[c.func];
  }
  function setFunc(c: WindowColumn, f: WindowFunc) {
    c.func = f;
    if (!usesColumn(f)) c.column = null;
    else if (!c.column) c.column = firstNumber();
    c.n = usesN(f) ? (c.n ?? (f === "moving_average" ? 3 : 1)) : null;
    suggest(c);
  }
</script>

<div class="space-y-5 text-sm">
  <fieldset class="space-y-2">
    <legend class="mb-1.5 font-semibold">Calculate</legend>
    {#each calcs as c, i (i)}
      <div class="flex flex-wrap items-center gap-2">
        <select class="field" aria-label="Calculation" value={c.func} onchange={(e) => setFunc(c, e.currentTarget.value as WindowFunc)}>
          {#each funcs as f (f)}<option value={f}>{windowWords[f]}</option>{/each}
        </select>
        {#if usesColumn(c.func)}
          <span class="text-ink-soft">of</span>
          <ColumnPicker
            bind:value={c.column}
            {columns}
            label="Column for {windowWords[c.func].toLowerCase()}"
            filter={needsNumber(c.func) ? (col) => isNumericType(col.type) : undefined}
          />
        {/if}
        {#if usesN(c.func)}
          {#if nWords[c.func]![0]}<span class="text-ink-soft">{nWords[c.func]![0]}</span>{/if}
          <input
            class="field w-20"
            type="number"
            min="1"
            aria-label="Number of rows"
            value={c.n ?? 1}
            onchange={(e) => (c.n = Math.max(1, Math.round(Number(e.currentTarget.value) || 1)))}
          />
          <span class="text-ink-soft">{nWords[c.func]![1]}</span>
        {/if}
        <span class="text-ink-soft">as</span>
        <input class="field w-40" aria-label="New column name" bind:value={c.name} />
        <button class="btn btn-quiet !px-1.5" aria-label="Remove this calculation" onclick={() => (calcs = calcs.filter((_, j) => j !== i))}>
          <X size={14} />
        </button>
      </div>
    {/each}
    <button
      class="btn btn-quiet !px-2"
      onclick={() => {
        const c: WindowColumn = { name: "", func: "running_sum", column: firstNumber(), n: null };
        suggest(c);
        calcs = [...calcs, c];
      }}
    >
      <Plus size={14} /> Add a calculation
    </button>
  </fieldset>

  <fieldset>
    <legend class="mb-1.5 font-semibold">Separately for each</legend>
    <ColumnChips bind:value={partitionBy} {columns} label="Group columns" />
    <p class="mt-1 max-w-[60ch] text-ink-soft">
      {partitionBy.length ? "Totals, ranks and previous values restart for each group." : "Nothing chosen: the whole table is one group."}
    </p>
  </fieldset>

  <OrderByList
    bind:by={orderBy}
    {columns}
    legend="In order of"
    addLabel={["Choose an order", "Then by"]}
    note="Without an order, rows count in their current order. Missing values go last; ties keep their order, and share the same rank. Rows stay where they are in the result."
  />
</div>
