<script lang="ts">
  import type { Column, ProfileRow } from "../api/types";
  import { formatCell, formatCount, typeLabel } from "../format";

  let { profile, columns }: { profile: ProfileRow[] | null; columns: Column[] } = $props();
</script>

<table class="w-full text-left text-sm">
  <thead class="sticky top-0 bg-paper text-ink-soft">
    <tr>
      <th class="px-3 py-1.5 font-semibold">Column</th>
      <th class="px-3 py-1.5 font-semibold">Type</th>
      {#if profile}
        <th class="px-3 py-1.5 text-right font-semibold">Missing</th>
        <th class="px-3 py-1.5 text-right font-semibold">Distinct</th>
        <th class="px-3 py-1.5 font-semibold">Smallest</th>
        <th class="px-3 py-1.5 font-semibold">Largest</th>
      {/if}
    </tr>
  </thead>
  <tbody class="tabular-nums">
    {#if profile}
      {#each profile as p (p.name)}
        <tr class="border-b border-rule/70">
          <td class="px-3 py-1.5 font-semibold">{p.name}</td>
          <td class="px-3 py-1.5 text-ink-soft">{typeLabel[p.type]}</td>
          <td class="px-3 py-1.5 text-right" class:text-stale={p.missing > 0}>
            {formatCount(p.missing)}{#if p.missing > 0}<span class="text-ink-faint">&nbsp;({((p.missing / Math.max(1, p.rows)) * 100).toFixed(p.missing * 1000 < p.rows ? 2 : 1)}%)</span
              >{/if}
          </td>
          <td class="px-3 py-1.5 text-right">{formatCount(p.distinct)}</td>
          <td class="max-w-48 truncate px-3 py-1.5">{formatCell(p.min)}</td>
          <td class="max-w-48 truncate px-3 py-1.5">{formatCell(p.max)}</td>
        </tr>
      {/each}
    {:else}
      {#each columns as c (c.name)}
        <tr class="border-b border-rule/70">
          <td class="px-3 py-1.5 font-semibold">{c.name}</td>
          <td class="px-3 py-1.5 text-ink-soft">{typeLabel[c.type]}</td>
        </tr>
      {/each}
    {/if}
  </tbody>
</table>
