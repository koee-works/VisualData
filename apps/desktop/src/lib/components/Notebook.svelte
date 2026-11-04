<script lang="ts">
  import { ArrowUp, ArrowDown, Plus, X, Download } from "@lucide/svelte";
  import { DropdownMenu } from "bits-ui";
  import type { Workspace } from "../workspace.svelte";
  import type { NotebookCell, Page } from "../api/types";
  import { columnDecimals, formatCell, formatCount, stepTitle } from "../format";
  import { renderMarkdown } from "../markdown";
  import StateMark from "./StateMark.svelte";
  import ChartView from "./ChartView.svelte";

  let { ws }: { ws: Workspace } = $props();

  let cells = $derived(ws.view?.cells ?? []);
  let editing = $state<number | null>(null);
  let draft = $state("");

  // First rows of each step cell's result.
  let previews = $state<Record<string, Page | null>>({});
  let charts = $state.raw<Record<string, unknown>>({});
  $effect(() => {
    void ws.revision;
    for (const c of cells) {
      if (c.type !== "step") continue;
      const s = ws.view?.steps.find((x) => x.id === c.step);
      if (!s || s.state === "not run") continue;
      if (s.mode === "chart") ws.backend.chart(s.id).then((spec) => (charts = { ...charts, [s.id]: spec }), () => {});
      else if (s.rows !== null) ws.backend.page(s.id, 0, 5).then((p) => (previews[s.id] = p), () => {});
    }
  });

  function update(next: NotebookCell[]) {
    ws.setCells(next);
  }
  function move(i: number, by: number) {
    const next = [...cells];
    const [c] = next.splice(i, 1);
    next.splice(i + by, 0, c!);
    update(next);
  }
  function insertText(at: number) {
    const next = [...cells];
    next.splice(at, 0, { type: "markdown", text: "Write what this shows and why it matters." });
    update(next);
    editing = at;
    draft = "";
  }
  function startEdit(i: number, text: string) {
    editing = i;
    draft = text;
  }
  function finishEdit(i: number) {
    const next = [...cells];
    next[i] = { type: "markdown", text: draft };
    editing = null;
    update(next);
  }
</script>

<article class="mx-auto max-w-[46rem] px-6 py-6" aria-label="Notebook">
  <div class="mb-1 flex items-start gap-3">
    <h1 class="flex-1 text-xl font-bold">{ws.view?.name}</h1>
    <DropdownMenu.Root>
      <DropdownMenu.Trigger class="btn btn-quiet !min-h-8 !px-2" disabled={ws.running}>
        <Download size={14} /> Export
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          class="z-50 min-w-64 rounded-md border border-rule-strong bg-panel p-1 shadow-[var(--shadow-menu)]"
          align="end"
          sideOffset={4}
        >
          {#each [["html", "Report", "A web page to share or print"], ["ipynb", "Jupyter notebook", "Reruns the analysis in Python"]] as [kind, name, hint] (kind)}
            <DropdownMenu.Item
              class="flex cursor-pointer flex-col rounded px-2 py-1.5 outline-none data-[highlighted]:bg-select"
              onSelect={() => ws.exportNotebook(kind as "html" | "ipynb")}
            >
              <span>{name}</span>
              <span class="text-xs text-ink-faint">{hint}</span>
            </DropdownMenu.Item>
          {/each}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  </div>
  <p class="mb-6 text-sm text-ink-soft">
    Notebook order is for reading. Steps still run in pipeline order, whatever their place here.
  </p>

  {#each cells as cell, i (i)}
    <div class="group relative mb-2">
      <div class="absolute -left-10 top-1 flex flex-col opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
        <button class="btn btn-quiet !min-h-6 !min-w-6 justify-center !px-1" aria-label="Move up" disabled={i === 0} onclick={() => move(i, -1)}><ArrowUp size={14} /></button>
        <button class="btn btn-quiet !min-h-6 !min-w-6 justify-center !px-1" aria-label="Move down" disabled={i === cells.length - 1} onclick={() => move(i, 1)}><ArrowDown size={14} /></button>
      </div>

      {#if cell.type === "markdown"}
        {#if editing === i}
          <textarea
            class="field min-h-28 w-full"
            aria-label="Notebook text"
            bind:value={draft}
            onblur={() => finishEdit(i)}
            onkeydown={(e) => e.key === "Escape" && finishEdit(i)}
          ></textarea>
          <p class="mt-1 text-xs text-ink-faint">Markdown: # headings, **bold**, *italics*, `code`, - lists. Click away to save.</p>
        {:else}
          <div class="prose-note flex items-start gap-2">
            <button class="flex-1 cursor-text text-left" onclick={() => startEdit(i, cell.text)} aria-label="Edit text">
              <!-- eslint-disable-next-line svelte/no-at-html-tags: escaped in renderMarkdown -->
              {@html renderMarkdown(cell.text)}
            </button>
            <button
              class="btn btn-quiet !min-h-6 !min-w-6 justify-center !px-1 opacity-0 group-hover:opacity-100"
              aria-label="Remove text"
              onclick={() => update(cells.filter((_, j) => j !== i))}><X size={14} /></button
            >
          </div>
        {/if}
      {:else}
        {@const s = ws.view?.steps.find((x) => x.id === cell.step)}
        {#if s}
          {@const p = previews[s.id]}
          <figure class="my-3 rounded-lg border border-rule bg-panel" class:hatch-border={s.state === "stale"}>
            <figcaption class="flex items-stretch gap-2.5 px-3 py-2">
              <StateMark state={s.state} />
              <button class="min-w-0 flex-1 text-left" onclick={() => ((ws.selected = s.id), (ws.centerTab = "step"))}>
                <span class="block font-semibold">{stepTitle(s)}</span>
                <span class="block text-xs text-ink-faint">
                  {s.mode === "visual" ? "Visual step" : s.mode === "chart" ? "Chart" : s.mode === "sql" ? "SQL step" : "Python step"}{s.rows !== null && s.mode !== "chart" ? `, ${formatCount(s.rows)} rows` : ""}{s.state === "stale" ? ", outdated" : s.state === "failed" ? ", failed" : s.state === "not run" ? ", not run yet" : ""}
                </span>
              </button>
            </figcaption>
            {#if s.mode === "chart" && charts[s.id] && s.state !== "not run"}
              <div class:opacity-60={s.state === "stale"}><ChartView spec={charts[s.id]} /></div>
            {:else if p && s.state !== "not run"}
              <div class="overflow-x-auto border-t border-rule" class:opacity-60={s.state === "stale"}>
                <table class="w-full text-sm tabular-nums">
                  <thead class="text-left text-ink-soft">
                    <tr>
                      {#each p.columns as c (c.name)}
                        <th class="px-3 py-1 font-semibold" class:text-right={c.type === "BIGINT" || c.type === "DOUBLE"}>{c.name}</th>
                      {/each}
                    </tr>
                  </thead>
                  <tbody>
                    {#each p.rows as r, ri (ri)}
                      <tr class="border-t border-rule/60">
                        {#each r as v, vi (vi)}
                          <td class="px-3 py-1" class:text-right={v.t === "int" || v.t === "float"} class:text-ink-faint={v.t === "null"}>
                            {v.t === "null" ? "missing" : formatCell(v, columnDecimals(p.rows.map((row) => row[vi]!)))}
                          </td>
                        {/each}
                      </tr>
                    {/each}
                  </tbody>
                </table>
                {#if p.total_rows > p.rows.length}
                  <p class="px-3 py-1 text-xs text-ink-faint">First {p.rows.length} of {formatCount(p.total_rows)} rows</p>
                {/if}
              </div>
            {/if}
          </figure>
        {/if}
      {/if}
    </div>
  {/each}

  <button class="btn btn-quiet mt-2 !px-2" onclick={() => insertText(cells.length)}><Plus size={15} /> Add text</button>
</article>

<style>
  .prose-note :global(h2) {
    font-size: var(--text-xl);
    font-weight: 700;
    margin: 1.2rem 0 0.4rem;
  }
  .prose-note :global(h3) {
    font-size: var(--text-lg);
    font-weight: 700;
    margin: 1rem 0 0.3rem;
  }
  .prose-note :global(p) {
    margin: 0.4rem 0;
    line-height: 1.6;
  }
  .prose-note :global(ul) {
    margin: 0.4rem 0 0.4rem 1.2rem;
    list-style: disc;
  }
  .prose-note :global(code) {
    font-family: var(--font-mono);
    font-size: 0.8125rem;
    background: var(--color-paper);
    padding: 0 0.25rem;
    border-radius: 3px;
  }
  .hatch-border {
    border-color: var(--color-stale);
  }
</style>
