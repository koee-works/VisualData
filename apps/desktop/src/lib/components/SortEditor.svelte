<script lang="ts">
  import type { Column } from "../api/types";
  import OrderByList from "./OrderByList.svelte";

  let {
    by = $bindable(),
    limit = $bindable(),
    columns,
  }: { by: { column: string; descending: boolean }[]; limit: number | null; columns: Column[] | null } = $props();
</script>

<div class="space-y-4 text-sm">
  <OrderByList
    bind:by
    {columns}
    legend="Sort by"
    addLabel={["Sort by a column", "Then sort by"]}
    note="Missing values go last. Rows that tie keep their order."
  />
  <label class="flex flex-wrap items-center gap-2">
    <input
      type="checkbox"
      class="accent-[var(--color-running)]"
      checked={limit !== null}
      onchange={(e) => (limit = e.currentTarget.checked ? 10 : null)}
    />
    <span>Keep only the first</span>
    <input
      class="field w-24"
      type="number"
      min="0"
      aria-label="Number of rows to keep"
      disabled={limit === null}
      value={limit ?? 10}
      onchange={(e) => (limit = Math.max(0, Math.round(Number(e.currentTarget.value) || 0)))}
    />
    <span>rows</span>
  </label>
</div>
