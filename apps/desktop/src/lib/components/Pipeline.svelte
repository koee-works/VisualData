<script lang="ts">
  import { DropdownMenu } from "bits-ui";
  import { Plus, Database, Filter, Sigma, Braces, Code, ChartColumn, Combine, FilePlus2 } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { StepOp, StepView } from "../api/types";
  import { formatCount, stepTitle } from "../format";
  import StateMark from "./StateMark.svelte";

  let { ws }: { ws: Workspace } = $props();

  const icons = { filter: Filter, aggregate: Sigma, join: Combine, sql: Braces, python: Code, chart: ChartColumn };

  // Columns of whatever the new step would read.
  let inputColumns = $derived(ws.columnsOf(ws.selected) ?? []);
  let canAddFrom = $derived(!!ws.selectedSource || (ws.selectedStep?.mode ?? "chart") !== "chart");
  // What a new join would combine the selected node with: another source
  // first, since that is the usual reason to join.
  let joinPartner = $derived.by(() => {
    // A new step has no readers yet, so any other table can be its partner.
    const options = ws.readable(null).filter((o) => o.id !== ws.selected);
    return options.find((o) => ws.isSource(o.id)) ?? options[0] ?? null;
  });

  function starter(type: StepOp["type"]): StepOp {
    const cols = inputColumns;
    const first = cols[0]?.name ?? "";
    const numeric = cols.find((c) => c.type === "BIGINT" || c.type === "DOUBLE")?.name ?? first;
    const text = cols.find((c) => c.type === "VARCHAR")?.name ?? first;
    switch (type) {
      case "join": {
        // Prefer a column both sides share by name.
        const other = joinPartner ? (ws.columnsOf(joinPartner.id) ?? []) : [];
        const shared = cols.find((c) => other.some((o) => o.name === c.name))?.name;
        const left = shared ?? first;
        const right = shared ?? other[0]?.name ?? first;
        return { type, how: "left", on: [{ left, right }] };
      }
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
    { type: "join", name: "Join with another table", hint: "Visual" },
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
            Reads {ws.nameOf(ws.selected)}
          </p>
          {#each choices as c (c.type)}
            {@const Icon = icons[c.type]}
            {@const blocked = c.type === "join" && !joinPartner}
            <DropdownMenu.Item
              class="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 outline-none data-[disabled]:cursor-default data-[disabled]:opacity-50 data-[highlighted]:bg-select"
              disabled={blocked}
              onSelect={() => ws.addStep(starter(c.type), null, c.type === "join" && joinPartner ? [joinPartner.id] : [])}
            >
              <Icon size={15} class="text-ink-soft" />
              <span class="flex-1">
                {c.name}
                {#if blocked}<span class="block text-xs text-ink-faint">Add another source first</span>{/if}
              </span>
              <span class="text-xs text-ink-faint">{c.hint}</span>
            </DropdownMenu.Item>
          {/each}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  </div>

  <div class="flex-1 overflow-y-auto px-2 pb-3">
    {#each ws.tree as t (t.source.id)}
      <ol class="mb-2" aria-label="Steps from {t.source.path}">
        <li>
          <button class="node" aria-current={ws.selected === t.source.id} onclick={() => (ws.selected = t.source.id)}>
            <span class="mark-source"></span>
            <Database size={15} class="mt-0.5 shrink-0 text-ink-soft" />
            <span class="min-w-0 flex-1">
              <span class="block truncate font-semibold">{t.source.path}</span>
              <span class="block text-xs text-ink-faint">
                {formatCount(t.source.rows)} rows{#if (ws.view?.sources.length ?? 0) > 1}, as <code class="code">{t.source.id}</code>{/if}
              </span>
            </span>
          </button>
        </li>
        {#each t.steps as { step: s, depth } (s.id)}
          {@const Icon = icons[s.op.type]}
          {@const ev = ws.progress[s.id]}
          <li style:padding-left="{depth * 14}px" class="relative">
            <span class="branch" style:left="{depth * 14 - 7}px"></span>
            <button class="node" aria-current={ws.selected === s.id} onclick={() => (ws.selected = s.id)}>
              <StateMark state={s.state} running={isRunning(s)} />
              <Icon size={15} class="mt-0.5 shrink-0 text-ink-soft" />
              <span class="min-w-0 flex-1">
                <span class="block truncate">{stepTitle(s)}</span>
                {#if s.inputs.length > 1}
                  <span class="block truncate text-xs text-ink-soft" title="Also reads {s.inputs.slice(1).map((i) => ws.nameOf(i)).join(', ')}">
                    with {s.inputs.slice(1).map((i) => ws.nameOf(i)).join(", ")}
                  </span>
                {/if}
                <span class="block text-xs text-ink-faint">
                  {#if isRunning(s)}
                    Running…
                  {:else if s.state === "failed"}
                    <span class="text-failed">Failed</span>
                  {:else if s.state === "blocked"}
                    Waiting on a failed step
                  {:else if s.state === "needs review"}
                    <span class="text-review">Needs your review</span>
                  {:else if s.state === "waiting on review"}
                    Waiting on a review
                  {:else if s.rows !== null && s.mode !== "chart"}
                    {formatCount(s.rows)} rows{s.state === "stale"
                      ? ", outdated"
                      : ev?.cached === false
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
    {/each}
    <button class="btn btn-quiet mt-1 w-full justify-start !px-2" disabled={ws.running} onclick={() => ws.addSource()}>
      <FilePlus2 size={15} /> Add source
    </button>
  </div>
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
