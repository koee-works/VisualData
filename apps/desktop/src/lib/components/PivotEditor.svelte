<script lang="ts">
  import type { AggFunc, Column } from "../api/types";
  import { funcWords, isNumericType } from "../format";
  import ColumnChips from "./ColumnChips.svelte";
  import ColumnPicker from "./ColumnPicker.svelte";

  let {
    rows = $bindable(),
    column = $bindable(),
    value = $bindable(),
    func = $bindable(),
    columns,
  }: { rows: string[]; column: string; value: string | null; func: AggFunc; columns: Column[] | null } = $props();

  const funcs = ["COUNT", "SUM", "AVG", "MIN", "MAX", "MEDIAN", "COUNT_DISTINCT"] as const;
  const needsNumber = (f: string) => ["SUM", "AVG", "MEDIAN", "STDDEV"].includes(f) || /^P\d+$/.test(f);
  // Values become column names, so only types with one exact written form.
  const pivotable = (c: Column) => c.type === "VARCHAR" || c.type === "BIGINT" || c.type === "BOOLEAN" || c.type === "DATE";
</script>

<div class="space-y-5 text-sm">
  <label class="flex flex-wrap items-center gap-2">
    <span class="font-semibold">A column for each value of</span>
    <ColumnPicker bind:value={column} {columns} label="Column whose values become columns" filter={pivotable} />
  </label>
  <div class="flex flex-wrap items-center gap-2">
    <span class="font-semibold">Holding the</span>
    <select
      class="field"
      aria-label="Summary"
      value={func}
      onchange={(e) => {
        func = e.currentTarget.value as AggFunc;
        if (func === "COUNT") value = null;
        else if (!value) value = columns?.find((c) => isNumericType(c.type) && c.name !== column)?.name ?? "";
      }}
    >
      {#each funcs as f (f)}<option value={f}>{f === "COUNT" ? "Number of rows" : funcWords(f)}</option>{/each}
    </select>
    {#if func !== "COUNT"}
      <span class="text-ink-soft">of</span>
      <ColumnPicker
        bind:value
        {columns}
        label="Column to summarize"
        filter={needsNumber(func) ? (c) => isNumericType(c.type) : undefined}
      />
    {/if}
  </div>
  <fieldset>
    <legend class="mb-1.5 font-semibold">One row for each</legend>
    <ColumnChips bind:value={rows} {columns} label="Row columns" filter={(c) => c.name !== column && c.name !== value} />
    <p class="mt-1 max-w-[60ch] text-ink-soft">
      {rows.length ? "" : "Nothing chosen: one row for the whole table. "}The new columns are named after the values and
      appear once the step runs. Rows with no {column || "value"} are left out.
    </p>
  </fieldset>
</div>
