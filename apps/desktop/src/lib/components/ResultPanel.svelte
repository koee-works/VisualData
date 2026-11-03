<script lang="ts">
  import { DropdownMenu, Tabs } from "bits-ui";
  import { Download } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import { errorMessage } from "../api/backend";
  import DataGrid from "./DataGrid.svelte";
  import ProfileTable from "./ProfileTable.svelte";
  import ChartView from "./ChartView.svelte";
  import type { ColumnDetail } from "../api/types";

  let { ws }: { ws: Workspace } = $props();

  let step = $derived(ws.selectedStep);
  let isSource = $derived(!!ws.selectedSource);
  let isChart = $derived(step?.mode === "chart");
  let hasResult = $derived(isSource || (step !== null && (step.rows !== null || (isChart && step.state !== "not run"))));
  let stale = $derived(!!step && step.state === "stale");

  let tab = $state("data");
  $effect.pre(() => {
    void ws.selected;
    tab = isChart ? "chart" : "data";
  });


  let chartSpec = $state.raw<unknown>(null);
  let chartError = $state<string | null>(null);
  $effect(() => {
    if (!isChart || !step || !hasResult) return;
    const id = step.id;
    void ws.revision;
    ws.backend.chart(id).then(
      (s) => ((chartSpec = s), (chartError = null)),
      (e) => ((chartSpec = null), (chartError = errorMessage(e))),
    );
  });

  // Column details are computed when the Columns tab is first opened.
  let details = $state<ColumnDetail[] | null>(null);
  $effect(() => {
    void ws.revision;
    const id = ws.selected;
    details = null;
    if (tab !== "columns" || isSource || !step || step.rows === null) return;
    ws.backend.columnDetails(id).then(
      (d) => id === ws.selected && (details = d),
      () => {},
    );
  });

  let fetchPage = $derived(
    isSource
      ? (o: number, l: number) => ws.backend.sourcePage(ws.selected, o, l)
      : (o: number, l: number) => ws.backend.page(ws.selected, o, l),
  );
  let resetKey = $derived(`${ws.selected}:${ws.revision}`);

  let code = $derived.by(() => {
    if (!step) return null;
    if (step.op.type === "python") return { lang: "Python", text: step.op.code };
    if (step.sql) return { lang: "SQL", text: step.sql };
    return null;
  });
</script>

<section class="flex h-full min-h-0 flex-col bg-panel" aria-label="Result">
  {#if stale}
    <div class="hatch-stale flex items-center gap-3 border-b border-stale/40 bg-stale-wash px-4 py-1.5 text-sm" role="status">
      <span class="font-semibold text-stale">Outdated result</span>
      <span class="text-ink-soft">Produced before a change upstream. Run the analysis to update it.</span>
      <button class="btn ml-auto !min-h-7 !py-0.5" onclick={() => ws.run()} disabled={ws.running}>Run now</button>
    </div>
  {/if}

  {#if !hasResult}
    <div class="flex flex-1 items-center justify-center p-8 text-center text-ink-soft">
      <div>
        <p class="text-lg font-semibold text-ink">No result yet</p>
        <p class="mt-1">
          {step?.state === "failed" ? "The last run failed at this step." : step?.state === "blocked" ? "An earlier step failed." : step?.state === "needs review" ? "This step runs once you approve its code." : step?.state === "waiting on review" ? "An earlier Python step needs your review first." : "Run the analysis to see what this step produces."}
        </p>
        {#if step?.state === "needs review" || step?.state === "waiting on review"}
          <button class="btn mt-4" onclick={() => (ws.reviewOpen = true)}>Review code</button>
        {:else}
          <button class="btn btn-primary mt-4" onclick={() => ws.run()} disabled={ws.running}>Run analysis</button>
        {/if}
      </div>
    </div>
  {:else}
    <Tabs.Root bind:value={tab} class="flex min-h-0 flex-1 flex-col">
      <Tabs.List class="flex border-b border-rule px-2">
        {#if isChart}<Tabs.Trigger value="chart" class="tab">Chart</Tabs.Trigger>{:else}
          <Tabs.Trigger value="data" class="tab">Data</Tabs.Trigger>
          {#if !isSource}<Tabs.Trigger value="columns" class="tab">Columns</Tabs.Trigger>{/if}
        {/if}
        {#if code}<Tabs.Trigger value="code" class="tab">{code.lang}</Tabs.Trigger>{/if}
        {#if !isChart}
          <DropdownMenu.Root>
            <DropdownMenu.Trigger
              class="btn btn-quiet my-1 ml-auto !min-h-7 !px-2"
              disabled={stale || ws.running}
              title={stale ? "Run the analysis first: an export must match the step" : "Save this table as a file"}
            >
              <Download size={14} /> Export
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                class="z-50 min-w-48 rounded-md border border-rule-strong bg-panel p-1 shadow-[0_6px_24px_rgba(29,42,63,0.14)]"
                align="end"
                sideOffset={4}
              >
                {#each [["csv", "CSV", "Opens anywhere"], ["xlsx", "Excel workbook", "Up to 1,048,575 rows"], ["parquet", "Parquet", "Exact types, any size"]] as [ext, name, hint] (ext)}
                  <DropdownMenu.Item
                    class="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 outline-none data-[highlighted]:bg-select"
                    onSelect={() => ws.exportResult(ws.selected, ext as "csv" | "xlsx" | "parquet")}
                  >
                    <span class="flex-1">{name}</span>
                    <span class="text-xs text-ink-faint">{hint}</span>
                  </DropdownMenu.Item>
                {/each}
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        {/if}
      </Tabs.List>
      {#if !isChart}
        <Tabs.Content value="data" class="min-h-0 flex-1">
          <DataGrid {fetchPage} {resetKey} />
        </Tabs.Content>
        <Tabs.Content value="columns" class="min-h-0 flex-1 overflow-auto">
          <ProfileTable details={details} columns={step?.columns ?? []} loading={details === null && tab === "columns"} />
        </Tabs.Content>
      {:else}
        <Tabs.Content value="chart" class="min-h-0 flex-1 overflow-auto">
          {#if chartError}<p class="p-4 text-sm text-failed">{chartError}</p>{:else if chartSpec}<ChartView spec={chartSpec} />{/if}
        </Tabs.Content>
      {/if}
      {#if code}
        <Tabs.Content value="code" class="min-h-0 flex-1 overflow-auto">
          {#if code.lang === "SQL"}
            <p class="px-4 pt-3 text-sm text-ink-soft">
              {step?.mode === "visual" ? "What this visual step runs, back to the source." : "This step's query, with the steps before it."}
            </p>
          {/if}
          <pre class="code px-4 py-3 whitespace-pre-wrap">{code.text}</pre>
        </Tabs.Content>
      {/if}
    </Tabs.Root>
  {/if}
</section>
