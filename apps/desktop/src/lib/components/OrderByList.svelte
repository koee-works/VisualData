<script lang="ts">
  import { Plus, X } from "@lucide/svelte";
  import type { Column } from "../api/types";
  import ColumnPicker from "./ColumnPicker.svelte";

  let {
    by = $bindable(),
    columns,
    legend,
    addLabel,
    note,
  }: { by: { column: string; descending: boolean }[]; columns: Column[] | null; legend: string; addLabel: [string, string]; note: string } = $props();
</script>

<fieldset>
  <legend class="mb-1.5 font-semibold">{legend}</legend>
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
  <button class="btn btn-quiet mt-2 !px-2" onclick={() => (by = [...by, { column: columns?.[0]?.name ?? "", descending: false }])}>
    <Plus size={14} /> {by.length ? addLabel[1] : addLabel[0]}
  </button>
  <p class="mt-1 max-w-[60ch] text-ink-soft">{note}</p>
</fieldset>
