<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import type { Cell, Column, Page } from "../api/types";
  import { columnDecimals, exactCell, formatCell, formatCount, isNumericType, missingLabel, typeLabel } from "../format";
  import { prefs, ROW_HEIGHTS } from "../prefs.svelte";

  let {
    fetchPage,
    resetKey,
    empty,
  }: {
    fetchPage: (offset: number, limit: number) => Promise<Page>;
    /** Changing this discards cached pages (new result, new revision). */
    resetKey: string;
    /** Shown when there are no rows: says why, and what to do. */
    empty?: Snippet;
  } = $props();

  // Row height follows the density preference.
  let ROW_H = $derived(ROW_HEIGHTS[prefs.value.rowDensity]);
  const PAGE = 200;
  // Browsers cap element heights well below 50M rows × 26 px, so very long
  // results scroll through a scaled spacer.
  const MAX_PX = 6_000_000;

  let total = $state(0);
  let columns = $state<Column[]>([]);
  let error = $state<string | null>(null);
  let loaded = $state(false);
  // Fetched pages live outside reactive state (reading them while clearing
  // them would loop); `version` tells the view when one arrives.
  let pages = new Map<number, Cell[][]>();
  let version = $state(0);
  let decimals = $state<number[]>([]);
  let pending = new Set<number>();
  let scrollTop = $state(0);
  let viewportH = $state(320);
  let generation = 0;
  // The header scrolls sideways with the rows (it sits outside the scrolling body).
  let headEl = $state<HTMLDivElement>();

  async function load(page: number, gen: number) {
    if (pending.has(page) || pages.has(page)) return;
    pending.add(page);
    try {
      const p = await fetchPage(page * PAGE, PAGE);
      if (gen !== generation) return;
      total = p.total_rows;
      columns = p.columns;
      pages.set(page, p.rows);
      // Decimals per column from the first page loaded, so they stay put while scrolling.
      if (!decimals.length) decimals = p.columns.map((_, ci) => columnDecimals(p.rows.map((r) => r[ci]!)));
      version++;
      loaded = true;
      error = null;
    } catch (e) {
      if (gen === generation) error = (e as { message?: string }).message ?? String(e);
    } finally {
      pending.delete(page);
    }
  }

  $effect(() => {
    void resetKey;
    untrack(() => {
      generation++;
      pages = new Map();
      pending = new Set();
      decimals = [];
      loaded = false;
      scrollTop = 0;
      load(0, generation);
    });
  });

  let fullH = $derived(total * ROW_H);
  let spacerH = $derived(Math.min(fullH, MAX_PX));
  let scale = $derived(fullH > MAX_PX ? (fullH - viewportH) / Math.max(1, MAX_PX - viewportH) : 1);
  let virtualTop = $derived(scrollTop * scale);
  let first = $derived(Math.min(Math.floor(virtualTop / ROW_H), Math.max(0, total - 1)));
  let count = $derived(Math.ceil(viewportH / ROW_H) + 2);
  let visible = $derived.by(() => {
    void version;
    const out: { index: number; row: Cell[] | null }[] = [];
    for (let i = first; i < Math.min(total, first + count); i++) {
      out.push({ index: i, row: pages.get(Math.floor(i / PAGE))?.[i % PAGE] ?? null });
    }
    return out;
  });

  $effect(() => {
    const wanted = new Set(visible.map((v) => Math.floor(v.index / PAGE)));
    untrack(() => {
      for (const p of wanted) load(p, generation);
    });
  });

  // Rough widths by type keep columns stable while pages arrive.
  // Fixed widths (by type and name length) keep columns stable while pages
  // arrive; a filler column takes the leftover width instead of stretching
  // values away from their headers.
  let template = $derived(
    `4.5rem ${columns
      .map((c) => {
        const base = c.type === "VARCHAR" ? 11 : c.type === "TIMESTAMP" ? 12.5 : c.type === "DATE" ? 7.5 : 7.5;
        return `${Math.max(base, c.name.length * 0.6 + 2)}rem`;
      })
      .join(" ")} 1fr`,
  );
  let rowNumberDigits = $derived(String(total).length);
</script>

{#if error}
  <p class="p-4 text-sm text-failed" role="alert">{error}</p>
{:else if !loaded}
  <p class="p-4 text-sm text-ink-faint">Loading rows…</p>
{:else if total === 0}
  {#if empty}
    {@render empty()}
  {:else}
    <p class="p-4 text-sm text-ink-soft">No rows.</p>
  {/if}
{:else}
  <div class="grid-wrap flex h-full flex-col" role="table" aria-rowcount={total + 1} aria-colcount={columns.length}>
    <div class="row head" role="row" bind:this={headEl} style:grid-template-columns={template}>
      <span class="num" role="columnheader">#</span>
      {#each columns as c (c.name)}
        <span role="columnheader" class:right={isNumericType(c.type)}>
          <span class="block truncate font-semibold">{c.name}</span>
          <span class="block text-xs font-normal text-ink-faint">{typeLabel[c.type]}</span>
        </span>
      {/each}
    </div>
    <div
      class="relative flex-1 overflow-auto"
      bind:clientHeight={viewportH}
      onscroll={(e) => {
        scrollTop = e.currentTarget.scrollTop;
        if (headEl) headEl.scrollLeft = e.currentTarget.scrollLeft;
      }}
    >
      <div style:height="{spacerH}px" aria-hidden="true"></div>
      <div class="absolute inset-x-0" style:top="{scrollTop - (virtualTop % ROW_H)}px">
        {#each visible as v (v.index)}
          <div class="row" role="row" aria-rowindex={v.index + 2} style:grid-template-columns={template}>
            <span class="num" role="rowheader" style:min-width="{rowNumberDigits}ch">{formatCount(v.index + 1)}</span>
            {#if v.row}
              {#each v.row as cell, ci (ci)}
                <span
                  role="cell"
                  class:right={cell.t === "int" || cell.t === "float"}
                  class:missing={cell.t === "null"}
                  title={cell.t === "null" ? "Missing" : exactCell(cell)}
                >
                  {cell.t === "null" ? missingLabel() : formatCell(cell, decimals[ci])}
                </span>
              {/each}
            {:else}
              {#each columns as c (c.name)}<span class="loading" role="cell"></span>{/each}
            {/if}
          </div>
        {/each}
      </div>
    </div>
    <p class="border-t border-rule px-3 py-1 text-xs text-ink-faint">
      Rows {formatCount(first + 1)}–{formatCount(Math.min(total, first + count - 2))} of {formatCount(total)}
    </p>
  </div>
{/if}

<style>
  .row {
    display: grid;
    height: var(--row-height, 26px);
    align-items: center;
    border-bottom: 1px solid color-mix(in srgb, var(--color-rule) 60%, transparent);
    font-variant-numeric: tabular-nums;
  }
  .row > span {
    padding: 0 0.6rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .head {
    height: auto;
    padding: 0.35rem 0;
    background: var(--color-paper);
    border-bottom: 1px solid var(--color-rule-strong);
    overflow: hidden;
  }
  .head > span {
    line-height: 1.2;
  }
  .right {
    text-align: right;
  }
  .num {
    color: var(--color-ink-faint);
    text-align: right;
    font-size: var(--text-xs);
  }
  .missing {
    color: var(--color-ink-faint);
    font-style: italic;
  }
  .loading::after {
    content: "";
    display: block;
    height: 0.6rem;
    border-radius: 2px;
    background: var(--color-paper);
  }
</style>
