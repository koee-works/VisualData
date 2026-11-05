<script lang="ts">
  import { Popover } from "bits-ui";
  import { Cpu, CircleX, Database, Info, ScrollText, SlidersHorizontal, Sheet, SquareTerminal, TriangleAlert, Workflow } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { PaneName } from "../layout";
  import ThemePicker from "./ThemePicker.svelte";
  import { prefs } from "../prefs.svelte";
  import { resources } from "../resources.svelte";
  import { formatBytes } from "../format";

  // Along the bottom, as in Zed: each dock's button on the side where the
  // dock sits, what the linter found, and how the analysis stands.
  let { ws }: { ws: Workspace } = $props();

  const mac = typeof navigator !== "undefined" && /Mac/.test(navigator.platform);
  const key = mac ? "⌘" : "Ctrl+";

  let steps = $derived(ws.view?.steps ?? []);
  let outdated = $derived(steps.filter((s) => s.state !== "fresh").length);
  let done = $derived(Object.keys(ws.progress).length);
  let lastFailed = $derived(Object.values(ws.progress).filter((e) => e.status === "failed").length);
  let summary = $derived.by(() => {
    if (ws.running) return `Running step ${Math.min(done + 1, steps.length)} of ${steps.length}`;
    if (!steps.length) return "Add a step to start";
    if (lastFailed) return `${lastFailed} step${lastFailed === 1 ? "" : "s"} failed on the last run`;
    if (!outdated) return "All results current";
    return `${outdated} of ${steps.length} results need a run`;
  });
  let codeStep = $derived(ws.centerTab === "step" && (ws.selectedStep?.op.type === "python" || ws.selectedStep?.op.type === "r"));
  let onStep = $derived(ws.centerTab === "step");

  // Read what the app uses only while it is shown.
  $effect(() => {
    if (!prefs.value.showResources) return;
    return resources.watch(ws.backend);
  });
  let usage = $derived(resources.value);
  let limitText = $derived.by(() => {
    const mb = prefs.value.codeMemoryMb;
    if (mb === 0) return "Python and R programs have no memory limit.";
    return `Python and R programs stop at ${formatBytes((mb ?? 4096) * 1_048_576)} each. Change it in Settings → Running.`;
  });
  const percent = (n: number) => (n < 10 ? n.toFixed(1) : String(Math.round(n)));

  // The icons say what a dock holds; the tooltip says where it opens.
  function title(shown: boolean, what: string, where: string, keys = "") {
    return `${shown ? "Hide" : "Show"} ${what}, ${where}${keys ? ` (${keys})` : ""}`;
  }
</script>

{#snippet toggle(name: PaneName, label: string, where: string, Icon: typeof Workflow, keys = "", disabled = false)}
  <button class="pane-toggle" aria-pressed={ws.panes[name]} aria-label={label} title={title(ws.panes[name], label.toLowerCase(), where, keys)} {disabled} onclick={() => ws.togglePane(name)}>
    <Icon size={15} />
  </button>
{/snippet}

<footer class="status-bar" aria-label="Status bar">
  <div class="flex items-center gap-0.5">
    {@render toggle("pipeline", "Pipeline", "on the left", Workflow, `${key}B`)}
  </div>
  <button
    class="status-item"
    aria-label="Problems"
    title="Show what the linter found"
    onclick={() => {
      ws.consoleTab = "problems";
      if (!ws.panes.activity) ws.togglePane("activity");
    }}
  >
    <span class="flex items-center gap-1" class:text-failed={ws.counts.error > 0}><CircleX size={13} />{ws.counts.error}</span>
    <span class="flex items-center gap-1" class:text-stale={ws.counts.warning > 0}><TriangleAlert size={13} />{ws.counts.warning}</span>
    {#if ws.counts.info}<span class="flex items-center gap-1"><Info size={13} />{ws.counts.info}</span>{/if}
  </button>
  <p
    class="min-w-0 truncate text-xs"
    class:text-failed={lastFailed > 0 && !ws.running}
    class:text-running={ws.running}
    class:text-ink-soft={!ws.running && !lastFailed}
    role="status"
    aria-live="polite"
  >
    {summary}
  </p>
  <div class="ml-auto flex items-center gap-0.5">
    {#if prefs.value.showResources && usage}
      <Popover.Root>
        <Popover.Trigger
          class="inline-flex items-center gap-1.5 rounded px-1.5 py-0.5 text-xs tabular-nums text-ink-soft hover:bg-select hover:text-ink"
          aria-label="Resources used by this app: {percent(usage.cpu)} percent of the processor, {formatBytes(usage.memory_bytes)} of memory"
          title="What this app and its Python and R programs use"
        >
          <Cpu size={13} />
          <span class:text-stale={resources.stale}>{percent(usage.cpu)}%</span>
          <span class:text-stale={resources.stale}>{formatBytes(usage.memory_bytes)}</span>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content class="z-50 w-80 rounded-md border border-rule-strong bg-panel p-3 text-sm shadow-[var(--shadow-menu)]" side="top" align="end" sideOffset={6}>
            <p class="font-semibold">Used by this app</p>
            <table class="mt-2 w-full tabular-nums">
              <thead>
                <tr class="text-left text-xs text-ink-soft">
                  <th class="pb-1 font-normal">Part</th>
                  <th class="pb-1 text-right font-normal">Processor</th>
                  <th class="pb-1 text-right font-normal">Memory</th>
                </tr>
              </thead>
              <tbody>
                {#each usage.parts as p (p.name)}
                  <tr>
                    <td class="py-0.5">{p.name}{#if p.count > 1}<span class="ml-1 text-ink-soft">×{p.count}</span>{/if}</td>
                    <td class="py-0.5 text-right">{percent(p.cpu)}%</td>
                    <td class="py-0.5 text-right">{formatBytes(p.memory_bytes)}</td>
                  </tr>
                {/each}
              </tbody>
              <tfoot>
                <tr class="border-t border-rule font-semibold">
                  <td class="pt-1">Total</td>
                  <td class="pt-1 text-right">{percent(usage.cpu)}%</td>
                  <td class="pt-1 text-right">{formatBytes(usage.memory_bytes)}</td>
                </tr>
              </tfoot>
            </table>
            <p class="mt-2 text-xs text-ink-soft">
              Processor is a share of the whole computer ({usage.cores} cores). Memory is {Math.round((usage.memory_bytes / Math.max(1, usage.total_memory_bytes)) * 100)}% of the computer's {formatBytes(usage.total_memory_bytes)}. The App row includes the DuckDB engine.
            </p>
            <p class="mt-1 text-xs text-ink-soft">{limitText}</p>
            {#if resources.stale}<p class="mt-1 text-xs text-stale">The last reading failed; these numbers are from earlier.</p>{/if}
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
      <span class="mx-1 h-4 border-l border-rule" aria-hidden="true"></span>
    {/if}
    <ThemePicker onedit={() => ws.openSettings("appearance")} />
    <span class="mx-1 h-4 border-l border-rule" aria-hidden="true"></span>
    {@render toggle("editor", ws.selectedSource ? "Source details" : "Step editor", "in the middle, above the result", ws.selectedSource ? Database : SlidersHorizontal, "", !onStep)}
    {@render toggle("result", "Result", "in the middle, below the editor", Sheet, "", !onStep)}
    {@render toggle("activity", "Activity", "along the bottom: problems, output, run log, history", ScrollText, `${key}J`)}
    {@render toggle("console", "Code console", "on the right", SquareTerminal, `${key}\``, !codeStep)}
  </div>
</footer>

<style>
  .status-bar {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    min-height: 1.85rem;
    padding: 0 0.5rem;
    border-top: 1px solid var(--color-rule);
    background: var(--color-paper);
  }
  .status-item {
    display: inline-flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.15rem 0.4rem;
    border-radius: 4px;
    font-size: var(--text-xs);
    color: var(--color-ink-soft);
  }
  .status-item:hover {
    background: var(--color-select);
    color: var(--color-ink);
  }
</style>
