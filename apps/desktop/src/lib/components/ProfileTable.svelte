<script lang="ts">
  import type { Column, ColumnDetail } from "../api/types";
  import { formatCount, typeLabel } from "../format";

  // Columns with their profile once it is computed; until then, names and types.
  let { details, columns, loading = false }: { details: ColumnDetail[] | null; columns: Column[]; loading?: boolean } = $props();

  function num(x: number | null): string {
    if (x === null) return "";
    if (Number.isInteger(x)) return x.toLocaleString("en-US");
    const a = Math.abs(x);
    return x.toLocaleString("en-US", { maximumFractionDigits: a >= 100 ? 1 : a >= 1 ? 2 : 4 });
  }
  const pct = (n: number, of: number) => {
    const p = (n / Math.max(1, of)) * 100;
    return p < 0.1 ? "<0.1%" : `${p.toFixed(p < 10 ? 1 : 0)}%`;
  };

  function notes(d: ColumnDetail): string[] {
    const out: string[] = [];
    if (d.zeros) out.push(`${formatCount(d.zeros)} zero${d.zeros === 1 ? "" : "s"}`);
    if (d.negatives) out.push(`${formatCount(d.negatives)} below zero`);
    if (d.blank) out.push(`${formatCount(d.blank)} blank`);
    if (d.min_length !== null && d.max_length !== null)
      out.push(d.min_length === d.max_length ? `${d.min_length} characters` : `${d.min_length} to ${d.max_length} characters`);
    if (d.distinct === d.rows - d.missing && d.rows > 1 && d.rows - d.missing > 1) out.push("all different");
    return out;
  }
</script>

<table class="w-full text-left text-sm">
  <thead class="sticky top-0 z-10 bg-paper text-ink-soft">
    <tr>
      <th class="px-3 py-1.5 font-semibold">Column</th>
      <th class="px-3 py-1.5 text-right font-semibold">Missing</th>
      <th class="px-3 py-1.5 text-right font-semibold">Distinct</th>
      <th class="px-3 py-1.5 font-semibold">Range</th>
      <th class="px-3 py-1.5 font-semibold">Typical values</th>
      <th class="px-3 py-1.5 font-semibold">Distribution</th>
      <th class="px-3 py-1.5 font-semibold">Notes</th>
    </tr>
  </thead>
  <tbody class="tabular-nums">
    {#if details}
      {#each details as d (d.name)}
        {@const peak = Math.max(1, ...d.histogram)}
        <tr class="border-b border-rule/70 align-top">
          <td class="px-3 py-1.5">
            <span class="block font-semibold">{d.name}</span>
            <span class="block text-xs text-ink-faint">{typeLabel[d.type]}</span>
          </td>
          <td class="px-3 py-1.5 text-right whitespace-nowrap" class:text-stale={d.missing > 0}>
            {formatCount(d.missing)}{#if d.missing > 0}<span class="block text-xs text-ink-faint">{pct(d.missing, d.rows)}</span>{/if}
          </td>
          <td class="px-3 py-1.5 text-right">{formatCount(d.distinct)}</td>
          <td class="max-w-56 px-3 py-1.5">
            {#if d.min !== null}<span class="block truncate" title="{d.min} to {d.max}">{d.min === d.max ? d.min : `${d.min} to ${d.max}`}</span>{/if}
          </td>
          <td class="max-w-72 px-3 py-1.5">
            {#if d.median !== null}
              <span class="block" title={d.mean !== null ? `Average ${num(d.mean)}${d.stddev !== null ? `, standard deviation ${num(d.stddev)}` : ""}` : ""}>
                Median {num(d.median)}
              </span>
              <span class="block text-xs text-ink-soft">Middle half {num(d.q1)} to {num(d.q3)}; average {num(d.mean)}</span>
            {:else if d.top.length}
              <span class="block truncate" title={d.top.map((t) => `${t.value}: ${t.count}`).join("\n")}>
                {#each d.top.slice(0, 3) as t, i (i)}{#if i > 0}{", "}{/if}{t.value}<span class="text-ink-faint">&nbsp;{formatCount(t.count)}</span>{/each}
              </span>
            {/if}
          </td>
          <td class="px-3 py-1.5">
            {#if d.histogram.length}
              <svg width="80" height="22" viewBox="0 0 80 22" role="img" aria-label="Distribution of {d.name}: {d.histogram.join(', ')}">
                {#each d.histogram as n, i (i)}
                  {@const h = n ? Math.max(1.5, (n / peak) * 20) : 0}
                  <rect x={i * 8} y={22 - h} width="6.5" height={h} fill="var(--color-ink)" opacity="0.75" />
                {/each}
                <line x1="0" y1="21.5" x2="80" y2="21.5" stroke="var(--color-rule-strong)" />
              </svg>
            {/if}
          </td>
          <td class="px-3 py-1.5 text-xs text-ink-soft">{notes(d).join("; ")}</td>
        </tr>
      {/each}
    {:else}
      {#each columns as c (c.name)}
        <tr class="border-b border-rule/70">
          <td class="px-3 py-1.5">
            <span class="block font-semibold">{c.name}</span>
            <span class="block text-xs text-ink-faint">{typeLabel[c.type]}</span>
          </td>
          <td colspan="6" class="px-3 py-1.5 text-ink-faint">{loading ? "Profiling…" : ""}</td>
        </tr>
      {/each}
    {/if}
  </tbody>
</table>
