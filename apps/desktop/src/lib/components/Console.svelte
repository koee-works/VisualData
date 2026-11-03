<script lang="ts">
  import { ChevronDown, ChevronUp, CircleX, EyeOff, Info, TriangleAlert } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { Problem, RunView, VersionView } from "../api/types";

  // Docked under the step's result: what the linter finds, what steps
  // printed, and the history of runs.
  let { ws }: { ws: Workspace } = $props();

  const icon = { error: CircleX, warning: TriangleAlert, info: Info };
  const tabs = [
    { id: "problems", label: "Problems" },
    { id: "output", label: "Output" },
    { id: "log", label: "Run log" },
    { id: "history", label: "History" },
  ] as const;

  // Saved versions of the analysis; refetched after every change.
  let versions = $state<VersionView[]>([]);
  $effect(() => {
    void ws.revision;
    if (!ws.consoleOpen || ws.consoleTab !== "history") return;
    ws.backend.history(200).then((v) => (versions = v), () => (versions = []));
  });

  let runs = $state<RunView[]>([]);
  $effect(() => {
    void ws.runs;
    if (!ws.consoleOpen || ws.consoleTab === "problems") return;
    ws.backend.runLog(20).then((r) => (runs = r), () => (runs = []));
  });
  let expanded = $state<string | null>(null);

  // Output of the run in progress, else of the latest run.
  let printed = $derived.by(() => {
    const live = Object.values(ws.progress).filter((e) => e.output);
    if (ws.running || live.length) return live;
    return (runs[0]?.steps ?? []).filter((e) => e.output);
  });

  function where(p: Problem): string {
    const parts: string[] = [];
    const m = p.field?.match(/^columns\.(\d+)\.formula$/);
    if (m) parts.push(`formula ${Number(m[1]) + 1}`);
    else if (p.field === "sql") parts.push("SQL");
    else if (p.field === "code") parts.push("code");
    if (p.line) parts.push(`line ${p.line}`);
    if (p.column) parts.push(`character ${p.column}`);
    return parts.join(", ");
  }

  function summary(c: { error: number; warning: number; info: number }) {
    const out = [];
    if (c.error) out.push(`${c.error} error${c.error === 1 ? "" : "s"}`);
    if (c.warning) out.push(`${c.warning} warning${c.warning === 1 ? "" : "s"}`);
    if (c.info) out.push(`${c.info} note${c.info === 1 ? "" : "s"}`);
    return out.join(", ") || "No problems";
  }

  function when(ms: number) {
    return new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
  }

  function open(tab: (typeof tabs)[number]["id"]) {
    ws.consoleTab = tab;
    ws.consoleOpen = true;
  }
</script>

<section class="flex min-h-0 flex-col border-t border-rule-strong bg-panel" aria-label="Console" class:h-56={ws.consoleOpen}>
  <div class="flex items-center gap-1 px-2">
    {#each tabs as t (t.id)}
      <button
        class="tab-button"
        aria-pressed={ws.consoleOpen && ws.consoleTab === t.id}
        onclick={() => (ws.consoleOpen && ws.consoleTab === t.id ? (ws.consoleOpen = false) : open(t.id))}
      >
        {t.label}
        {#if t.id === "problems" && (ws.counts.error || ws.counts.warning)}
          <span class="ml-1 inline-flex items-center gap-1.5 text-xs">
            {#if ws.counts.error}<span class="flex items-center gap-0.5 text-failed"><CircleX size={12} />{ws.counts.error}</span>{/if}
            {#if ws.counts.warning}<span class="flex items-center gap-0.5 text-stale"><TriangleAlert size={12} />{ws.counts.warning}</span>{/if}
          </span>
        {/if}
      </button>
    {/each}
    <span class="ml-auto truncate px-2 text-xs text-ink-faint" aria-live="polite">{summary(ws.counts)}</span>
    <button
      class="btn btn-quiet !min-h-7 !px-1.5"
      aria-label={ws.consoleOpen ? "Close the console" : "Open the console"}
      onclick={() => (ws.consoleOpen = !ws.consoleOpen)}
    >
      {#if ws.consoleOpen}<ChevronDown size={15} />{:else}<ChevronUp size={15} />{/if}
    </button>
  </div>

  {#if ws.consoleOpen}
    <div class="min-h-0 flex-1 overflow-y-auto border-t border-rule">
      {#if ws.consoleTab === "problems"}
        {#if !ws.problems.items.length}
          <p class="px-4 py-3 text-sm text-ink-soft">
            Nothing to report. Steps are checked as you edit, and results after each run.
          </p>
        {:else}
          <ul class="divide-y divide-rule text-sm" aria-label="Problems">
            {#each ws.problems.items as p, i (i)}
              {@const Icon = icon[p.severity]}
              <li class="group flex items-start gap-2 px-3 py-1.5">
                <Icon
                  size={15}
                  class="mt-0.5 shrink-0 {p.severity === 'error' ? 'text-failed' : p.severity === 'warning' ? 'text-stale' : 'text-ink-faint'}"
                  aria-label={p.severity}
                />
                <div class="min-w-0 flex-1">
                  <p>
                    {#if p.step}
                      <button class="font-semibold underline decoration-rule-strong underline-offset-2 hover:text-ink" onclick={() => ws.reveal(p)}>
                        {ws.nameOf(p.step)}</button
                      >{#if where(p)}{" "}<span class="text-ink-faint">({where(p)})</span>{/if}:
                    {/if}
                    {p.message}
                  </p>
                  {#if p.from_last_run}<p class="text-xs text-ink-faint">From the last run's results</p>{/if}
                </div>
                {#if p.dismissable}
                  <button
                    class="btn btn-quiet !min-h-6 !px-1.5 opacity-0 group-focus-within:opacity-100 group-hover:opacity-100"
                    title="Dismiss for this step; saved with the project"
                    aria-label="Dismiss"
                    onclick={() => ws.dismiss(p)}
                  >
                    <EyeOff size={14} />
                  </button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
        {#if ws.problems.dismissed}
          <p class="px-4 py-2 text-xs text-ink-faint">
            {ws.problems.dismissed} dismissed.
            <button class="underline underline-offset-2 hover:text-ink" onclick={() => ws.restoreDismissed()}>Show them again</button>
          </p>
        {/if}
      {:else if ws.consoleTab === "output"}
        {#if !printed.length}
          <p class="px-4 py-3 text-sm text-ink-soft">
            Nothing was printed in the last run. Text a Python step prints, and its warnings, appear here.
          </p>
        {:else}
          {#each printed as e (e.step_id)}
            <div class="border-b border-rule px-3 py-2">
              <button class="text-sm font-semibold underline decoration-rule-strong underline-offset-2" onclick={() => ((ws.selected = e.step_id), (ws.centerTab = "step"))}>
                {ws.nameOf(e.step_id)}
              </button>
              <pre class="code mt-1 max-h-40 overflow-auto whitespace-pre-wrap text-ink-soft">{e.output}</pre>
            </div>
          {/each}
        {/if}
      {:else if ws.consoleTab === "history"}
        {#if !versions.length}
          <p class="px-4 py-3 text-sm text-ink-soft">No saved versions yet. Every change you save is kept here, and earlier versions can be restored.</p>
        {:else}
          <ol class="text-sm" aria-label="Saved versions">
            {#each versions as v (v.version)}
              <li class="flex items-center gap-3 border-b border-rule px-3 py-1.5">
                <span class="w-40 shrink-0 text-ink-soft">{when(v.time_unix_ms)}</span>
                <span class="min-w-0 flex-1 truncate" title={v.what}>{v.what}</span>
                {#if v.current}
                  <span class="text-xs text-ink-faint">Current</span>
                {:else}
                  <button class="btn btn-quiet !min-h-6 !px-2 text-xs" disabled={ws.running} onclick={() => ws.restoreVersion(v.version)}>Restore</button>
                {/if}
              </li>
            {/each}
          </ol>
          <p class="px-4 py-2 text-xs text-ink-faint">Kept in the project's history folder, so they travel with it. Restoring keeps results that still match, and can be undone.</p>
        {/if}
      {:else}
        {#if !runs.length}
          <p class="px-4 py-3 text-sm text-ink-soft">No runs yet.</p>
        {:else}
          <ul class="text-sm" aria-label="Runs">
            {#each runs as r (r.run_id)}
              {@const failed = r.steps.filter((x) => x.status === "failed").length}
              {@const ran = r.steps.filter((x) => x.status === "succeeded" && !x.cached).length}
              {@const ms = r.steps.reduce((a, x) => a + x.duration_ms, 0)}
              <li class="border-b border-rule">
                <button
                  class="flex w-full items-center gap-3 px-3 py-1.5 text-left hover:bg-paper"
                  aria-expanded={expanded === r.run_id}
                  onclick={() => (expanded = expanded === r.run_id ? null : r.run_id)}
                >
                  <span class="w-40 shrink-0">{when(r.started_unix_ms)}</span>
                  <span class={r.succeeded ? "text-fresh" : "text-failed"}>{r.succeeded ? "Succeeded" : failed ? `${failed} failed` : "Did not finish"}</span>
                  {#if r.trigger === "scheduled"}<span class="tag" title="Run by vda-run, without the app">Scheduled</span>{/if}
                  <span class="text-ink-soft">
                    {ran} ran, {r.steps.length - ran - failed} reused or skipped, {(ms / 1000).toFixed(1)} s
                  </span>
                </button>
                {#if expanded === r.run_id}
                  <table class="mb-2 ml-3 text-xs">
                    <tbody>
                      {#each r.steps as x (x.step_id)}
                        <tr>
                          <td class="py-0.5 pr-4">{ws.nameOf(x.step_id)}</td>
                          <td class="pr-4 {x.status === 'failed' ? 'text-failed' : 'text-ink-soft'}">
                            {x.status === "succeeded" ? (x.cached ? "reused" : "ran") : x.status}
                          </td>
                          <td class="pr-4 text-right text-ink-soft">{x.rows !== null ? `${x.rows.toLocaleString("en-US")} rows` : ""}</td>
                          <td class="pr-4 text-right text-ink-soft">{x.duration_ms} ms</td>
                          <td class="text-ink-soft">{x.detail ?? ""}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </div>
  {/if}
</section>

<style>
  .tab-button {
    display: inline-flex;
    align-items: center;
    padding: 0.35rem 0.6rem;
    font-size: 0.8125rem;
    color: var(--color-ink-soft);
    border-bottom: 2px solid transparent;
    cursor: pointer;
  }
  .tab-button[aria-pressed="true"] {
    color: var(--color-ink);
    font-weight: 600;
    border-bottom-color: var(--color-ink);
  }
</style>
