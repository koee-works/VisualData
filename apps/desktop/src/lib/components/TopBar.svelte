<script lang="ts">
  import { Play, Square } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import { formatCount } from "../format";

  let { ws }: { ws: Workspace } = $props();

  let name = $state("");
  $effect.pre(() => {
    name = ws.view?.name ?? "";
  });

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
</script>

<header class="flex items-center gap-4 border-b border-rule bg-panel px-4 py-2">
  <input
    class="min-w-0 flex-1 rounded px-1 py-0.5 text-lg font-bold hover:bg-paper focus:bg-paper focus:outline-none"
    aria-label="Analysis name"
    bind:value={name}
    onchange={() => name.trim() && name !== ws.view?.name && ws.rename(name.trim())}
  />
  <p class="hidden text-sm text-ink-soft md:block" title={ws.view?.project_dir}>
    {ws.view?.source.path}, {formatCount(ws.view?.source.rows ?? 0)} rows
  </p>
  <p
    class="text-sm"
    class:text-failed={lastFailed > 0 && !ws.running}
    class:text-running={ws.running}
    class:text-ink-soft={!ws.running && !lastFailed}
    role="status"
    aria-live="polite"
  >
    {summary}
  </p>
  {#if ws.running}
    <button class="btn" onclick={() => ws.cancel()}><Square size={14} /> Cancel</button>
  {:else}
    <button class="btn btn-primary" onclick={() => ws.run()} disabled={!steps.length}>
      <Play size={14} /> Run analysis
    </button>
  {/if}
</header>
