<script lang="ts">
  import { CircleX, Database, Info, ScrollText, SlidersHorizontal, Sheet, SquareTerminal, TriangleAlert, Workflow } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { PaneName } from "../layout";
  import ThemePicker from "./ThemePicker.svelte";

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
