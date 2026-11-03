<script lang="ts">
  import { Plus, X } from "@lucide/svelte";
  import type { Column } from "../api/types";
  import ColumnPicker from "./ColumnPicker.svelte";

  let {
    by = $bindable(),
    limit = $bindable(),
    columns,
  }: { by: { column: string; descending: boolean }[]; limit: number | null; columns: Column[] | null } = $props();
</script>

<div class="space-y-4 text-sm">
  <fieldset>
    <legend class="mb-1.5 font-semibold">Sort by</legend>
    <ol class="space-y-2">
      {#each by as k, i (i)}
        <li class="flex flex-wrap items-center gap-2">
          <span class="w-12 text-ink-soft">{i === 0 ? "First" : "Then"}</span>
          <ColumnPicker bind:value={k.column} {columns} label="Sort column" />
          <select
            class="field"
            aria-label="Direction"
            value={k.descending ? "desc" : "asc"}
            onchange={(e) => (k.descending = e.currentTarget.value === "desc")}
          >
            <option value="asc">smallest / earliest / A first</option>
            <option value="desc">largest / latest / Z first</option>
          </select>
          <button class="btn btn-quiet !px-1.5" aria-label="Remove this sort" onclick={() => (by = by.filter((_, j) => j !== i))}>
            <X size={14} />
          </button>
        </li>
      {/each}
    </ol>
    <button
      class="btn btn-quiet mt-2 !px-2"
      onclick={() => (by = [...by, { column: columns?.[0]?.name ?? "", descending: false }])}
    >
      <Plus size={14} /> {by.length ? "Then sort by" : "Sort by a column"}
    </button>
    <p class="mt-1 max-w-[60ch] text-ink-soft">Missing values go last. Rows that tie keep their order.</p>
  </fieldset>
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
