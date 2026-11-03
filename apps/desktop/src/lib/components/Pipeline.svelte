<script lang="ts">
  import { DropdownMenu } from "bits-ui";
  import { Plus, Database, Filter, Sigma, Braces, Code, ChartColumn } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { StepOp, StepView } from "../api/types";
  import { SOURCE_ID } from "../api/types";
  import { formatCount, stepTitle } from "../format";
  import StateMark from "./StateMark.svelte";

  let { ws }: { ws: Workspace } = $props();

  const icons = { filter: Filter, aggregate: Sigma, sql: Braces, python: Code, chart: ChartColumn };

  // Columns of whatever the new step would read.
  let inputColumns = $derived(
    ws.selected === SOURCE_ID ? (ws.view?.source.columns ?? []) : (ws.selectedStep?.columns ?? []),
  );
  let canAddFrom = $derived(ws.selected === SOURCE_ID || (ws.selectedStep?.mode ?? "chart") !== "chart");

  function starter(type: StepOp["type"]): StepOp {
    const cols = inputColumns;
    const first = cols[0]?.name ?? "";
    const numeric = cols.find((c) => c.type === "BIGINT" || c.type === "DOUBLE")?.name ?? first;
    const text = cols.find((c) => c.type === "VARCHAR")?.name ?? first;
    switch (type) {
      case "filter":
        return { type, predicates: [{ column: first, op: "IS NOT NULL", value: null }] };
      case "aggregate":
        return { type, group_by: text ? [text] : [], measures: [{ func: "COUNT", column: null, alias: "rows" }] };
      case "sql":
        return { type, sql: "SELECT *\nFROM input\nLIMIT 100" };
      case "python":
        return { type, code: "# `df` is the input as a pandas DataFrame.\nresult = df", timeout_ms: 60000 };
      case "chart":
        return { type, mark: "bar", x: text, y: numeric, title: null };
    }
  }

  const choices: { type: StepOp["type"]; name: string; hint: string }[] = [
    { type: "filter", name: "Filter rows", hint: "Visual" },
    { type: "aggregate", name: "Group and summarize", hint: "Visual" },
    { type: "chart", name: "Chart", hint: "Visual" },
    { type: "sql", name: "SQL", hint: "Code" },
    { type: "python", name: "Python", hint: "Code" },
  ];

  function isRunning(s: StepView) {
    return ws.running && !ws.progress[s.id];
  }
</script>

<nav class="flex h-full flex-col" aria-label="Pipeline">
  <div class="flex items-center justify-between px-3 pt-3 pb-2">
    <h2 class="text-sm font-semibold text-ink-soft">Pipeline</h2>
    <DropdownMenu.Root>
      <DropdownMenu.Trigger class="btn btn-quiet !px-2" disabled={!canAddFrom || ws.running} aria-label="Add a step">
        <Plus size={16} /> Add step
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          class="z-50 min-w-56 rounded-md border border-rule-strong bg-panel p-1 shadow-[0_6px_24px_rgba(29,42,63,0.14)]"
          align="end"
          sideOffset={4}
        >
          <p class="px-2 pt-1 pb-1.5 text-xs text-ink-faint">
            Reads {ws.selected === SOURCE_ID ? "the source" : stepTitle(ws.selectedStep!)}
          </p>
          {#each choices as c (c.type)}
            {@const Icon = icons[c.type]}
            <DropdownMenu.Item
              class="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 outline-none data-[highlighted]:bg-select"
              onSelect={() => ws.addStep(starter(c.type))}
            >
              <Icon size={15} class="text-ink-soft" />
              <span class="flex-1">{c.name}</span>
              <span class="text-xs text-ink-faint">{c.hint}</span>
            </DropdownMenu.Item>
          {/each}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  </div>

  <ol class="flex-1 overflow-y-auto px-2 pb-3">
    <li>
      <button
        class="node"
        aria-current={ws.selected === SOURCE_ID}
        onclick={() => (ws.selected = SOURCE_ID)}
      >
        <span class="mark-source"></span>
        <Database size={15} class="mt-0.5 shrink-0 text-ink-soft" />
        <span class="min-w-0 flex-1">
          <span class="block truncate font-semibold">{ws.view?.source.path}</span>
          <span class="block text-xs text-ink-faint">{formatCount(ws.view?.source.rows ?? 0)} rows</span>
        </span>
      </button>
    </li>
    {#each ws.ordered as s (s.id)}
      {@const Icon = icons[s.op.type]}
      {@const ev = ws.progress[s.id]}
      <li style:padding-left="{(ws.depth[s.id] ?? 1) * 14}px" class="relative">
        <span class="branch" style:left="{(ws.depth[s.id] ?? 1) * 14 - 7}px"></span>
        <button class="node" aria-current={ws.selected === s.id} onclick={() => (ws.selected = s.id)}>
          <StateMark state={s.state} running={isRunning(s)} />
          <Icon size={15} class="mt-0.5 shrink-0 text-ink-soft" />
          <span class="min-w-0 flex-1">
            <span class="block truncate">{stepTitle(s)}</span>
            <span class="block text-xs text-ink-faint">
              {#if isRunning(s)}
                Running…
              {:else if s.state === "failed"}
                <span class="text-failed">Failed</span>
              {:else if s.state === "blocked"}
                Waiting on a failed step
              {:else if s.rows !== null && s.mode !== "chart"}
                {formatCount(s.rows)} rows{s.state === "stale" ? ", outdated" : ""}{ev?.cached === false
                  ? `, ${ev.duration_ms} ms`
                  : ""}
              {:else if s.state === "stale"}
                Outdated
              {:else if s.mode === "chart" && s.state === "fresh"}
                Chart ready
              {:else}
                Not run
              {/if}
            </span>
          </span>
          {#if s.mode === "sql" || s.mode === "python"}
            <span class="tag" title="Edited as code, not with visual controls">{s.mode === "sql" ? "SQL" : "Py"}</span>
          {/if}
        </button>
      </li>
    {/each}
  </ol>
</nav>

<style>
  .node {
    display: flex;
    width: 100%;
    gap: 0.5rem;
    align-items: flex-start;
    text-align: left;
    padding: 0.4rem 0.5rem;
    border-radius: 6px;
    border: 1px solid transparent;
    cursor: pointer;
    background: none;
  }
  .node:hover {
    background: var(--color-paper);
  }
  .node[aria-current="true"] {
    background: var(--color-select);
    border-color: color-mix(in srgb, var(--color-running) 30%, transparent);
  }
  .mark-source {
    width: 6px;
    align-self: stretch;
    border-radius: 3px;
    background: var(--color-ink);
    flex: none;
  }
  .branch {
    position: absolute;
    top: 0;
    height: 1.2rem;
    width: 8px;
    border-left: 1px solid var(--color-rule-strong);
    border-bottom: 1px solid var(--color-rule-strong);
    border-bottom-left-radius: 4px;
  }
  .tag {
    font-family: var(--font-mono);
    font-size: 0.6875rem;
    padding: 0 0.3rem;
    border-radius: 3px;
    border: 1px solid var(--color-rule-strong);
    color: var(--color-ink-soft);
    align-self: center;
  }
</style>
