<script lang="ts">
  import type { Column } from "../api/types";

  // Choose several columns by toggling chips; before the input has run its
  // columns are unknown, so names are typed, separated by commas.
  let {
    value = $bindable(),
    columns,
    label,
    filter,
  }: { value: string[]; columns: Column[] | null; label: string; filter?: (c: Column) => boolean } = $props();

  let shown = $derived((columns ?? []).filter((c) => !filter || filter(c)));

  function toggle(name: string) {
    value = value.includes(name) ? value.filter((v) => v !== name) : [...value, name];
  }
</script>

{#if columns}
  <div class="flex flex-wrap gap-1.5" role="group" aria-label={label}>
    {#each shown as c (c.name)}
      <button class="chip" aria-pressed={value.includes(c.name)} onclick={() => toggle(c.name)}>{c.name}</button>
    {/each}
    {#each value.filter((v) => !shown.some((c) => c.name === v)) as v (v)}
      <button class="chip" aria-pressed="true" title="Not in the input" onclick={() => toggle(v)}>{v} (not in input)</button>
    {/each}
  </div>
{:else}
  <input
    class="field w-72"
    aria-label="{label}, separated by commas"
    value={value.join(", ")}
    onchange={(e) => (value = e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean))}
  />
{/if}
