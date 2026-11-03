<script lang="ts">
  import type { Column } from "../api/types";
  import { typeLabel } from "../format";

  let {
    value = $bindable(),
    columns,
    label,
    allowNone = null,
    filter,
  }: {
    value: string | null;
    columns: Column[] | null;
    label: string;
    /** Text for an extra "no column" choice (null hides it). */
    allowNone?: string | null;
    filter?: (c: Column) => boolean;
  } = $props();

  let shown = $derived((columns ?? []).filter((c) => !filter || filter(c)));
</script>

{#if columns}
  <select class="field min-w-36" aria-label={label} bind:value>
    {#if allowNone !== null}<option value={null}>{allowNone}</option>{/if}
    {#if value && !shown.some((c) => c.name === value)}
      <option {value}>{value} (not in input)</option>
    {/if}
    {#each shown as c (c.name)}
      <option value={c.name}>{c.name} ({typeLabel[c.type]})</option>
    {/each}
  </select>
{:else}
  <!-- Input not run yet: columns unknown, so names are typed. -->
  <input class="field min-w-36" aria-label={label} bind:value placeholder="column name" />
{/if}
