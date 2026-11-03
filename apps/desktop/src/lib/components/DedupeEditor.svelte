<script lang="ts">
  import type { Column, SortKey } from "../api/types";
  import ColumnChips from "./ColumnChips.svelte";
  import OrderByList from "./OrderByList.svelte";

  let { keys = $bindable(), orderBy = $bindable(), columns }: { keys: string[]; orderBy: SortKey[]; columns: Column[] | null } = $props();
</script>

<div class="space-y-5 text-sm">
  <fieldset>
    <legend class="mb-1.5 font-semibold">Keep one row for each</legend>
    <ColumnChips bind:value={keys} {columns} label="Key columns" />
    <p class="mt-1 max-w-[60ch] text-ink-soft">
      {keys.length
        ? `Rows with the same ${keys.join(", ")} count as repeats. Missing values match each other.`
        : "Nothing chosen: only rows identical in every column count as repeats."}
    </p>
  </fieldset>
  <OrderByList
    bind:by={orderBy}
    {columns}
    legend="Keep the first by"
    addLabel={["Choose which row to keep", "Then by"]}
    note={orderBy.length
      ? "Missing values go last; ties keep the earlier row. Kept rows stay in their order."
      : "Nothing chosen: the earliest row of each set is kept."}
  />
</div>
