<script lang="ts">
  import type { Column } from "../api/types";
  import ColumnChips from "./ColumnChips.svelte";

  let {
    picked = $bindable(),
    name = $bindable(),
    value = $bindable(),
    columns,
  }: { picked: string[]; name: string; value: string; columns: Column[] | null } = $props();
</script>

<div class="space-y-5 text-sm">
  <fieldset>
    <legend class="mb-1.5 font-semibold">Turn these columns into rows</legend>
    <ColumnChips bind:value={picked} {columns} label="Columns to turn into rows" />
    <p class="mt-1 max-w-[60ch] text-ink-soft">
      Each row becomes {picked.length || "one"} row{picked.length === 1 ? "" : "s"}, one per column, keeping the other
      columns. The columns need one kind of value; whole numbers and decimals mix as decimals.
    </p>
  </fieldset>
  <div class="flex flex-wrap items-center gap-2">
    <label class="flex items-center gap-2">
      <span class="text-ink-soft">Column names go in</span>
      <input class="field w-36" aria-label="Column for the names" bind:value={name} />
    </label>
    <label class="flex items-center gap-2">
      <span class="text-ink-soft">and their values in</span>
      <input class="field w-36" aria-label="Column for the values" bind:value />
    </label>
  </div>
</div>
