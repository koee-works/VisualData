<script lang="ts">
  import { DropdownMenu } from "bits-ui";
  import {
    Plus,
    Database,
    Filter,
    Sigma,
    Braces,
    Code,
    ChartColumn,
    Combine,
    FilePlus2,
    Rows3,
    Sheet,
    Calculator,
    Columns3,
    ArrowDownUp,
    TrendingUp,
    CopyMinus,
    Table2,
    ListTree,
    ShieldCheck,
    FlaskConical,
    MessageSquare,
  } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { StepOp, StepView } from "../api/types";
  import { formatCount, stepTitle } from "../format";
  import StateMark from "./StateMark.svelte";
  import ParametersPanel from "./ParametersPanel.svelte";

  let { ws }: { ws: Workspace } = $props();

  const icons = {
    filter: Filter,
    aggregate: Sigma,
    join: Combine,
    append: Rows3,
    derive: Calculator,
    select: Columns3,
    sort: ArrowDownUp,
    window: TrendingUp,
    dedupe: CopyMinus,
    pivot: Table2,
    unpivot: ListTree,
    check: ShieldCheck,
    test: FlaskConical,
    sql: Braces,
    python: Code,
    r: Code,
    chart: ChartColumn,
  };

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

  /** A column as a formula refers to it. */
  const ref = (name: string) => (/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) ? name : `[${name}]`);

  function starter(type: StepOp["type"]): StepOp {
    const cols = inputColumns;
    const first = cols[0]?.name ?? "";
    const numeric = cols.find((c) => c.type === "BIGINT" || c.type === "DOUBLE")?.name ?? first;
    const text = cols.find((c) => c.type === "VARCHAR")?.name ?? first;
    switch (type) {
      case "append":
        return { type, distinct: false, origin: null };
      case "derive":
        return numeric && cols.some((c) => c.name === numeric && (c.type === "BIGINT" || c.type === "DOUBLE"))
          ? { type, columns: [{ name: `${numeric}_rounded`, formula: `round(${ref(numeric)}, 2)` }] }
          : { type, columns: [{ name: "label", formula: text ? `upper(${ref(text)})` : '"…"' }] };
      case "select":
        return { type, columns: cols.map((c) => ({ column: c.name, as: null })) };
      case "sort":
        return { type, by: [{ column: numeric, descending: true }], limit: null };
      case "window": {
        const date = cols.find((c) => c.type === "DATE" || c.type === "TIMESTAMP")?.name;
        return {
          type,
          partition_by: [],
          order_by: date ? [{ column: date, descending: false }] : [],
          columns: [{ name: `running_${numeric}`, func: "running_sum", column: numeric, n: null }],
        };
      }
      case "dedupe":
        return { type, keys: text ? [text] : [], order_by: [] };
      case "pivot": {
        const pivotable = cols.filter((c) => c.type === "VARCHAR" || c.type === "DATE" || c.type === "BOOLEAN");
        const on = pivotable[1]?.name ?? pivotable[0]?.name ?? first;
        const rows = pivotable[0] && pivotable[0].name !== on ? [pivotable[0].name] : [];
        return numeric && numeric !== on
          ? { type, rows, column: on, value: numeric, func: "SUM" }
          : { type, rows, column: on, value: null, func: "COUNT" };
      }
      case "unpivot": {
        const nums = cols.filter((c) => c.type === "BIGINT" || c.type === "DOUBLE").map((c) => c.name);
        return { type, columns: nums.slice(0, 2), name: "column", value: "value" };
      }
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
      case "r":
        return { type, code: "# `df` is the input as a data frame.\nresult <- df", timeout_ms: 60000 };
      case "chart":
        return { type, mark: "bar", x: text, y: numeric, title: null, color: null, summary: "SUM" };
      case "test": {
        // Measures, not identifiers.
        const nums = cols.filter((c) => (c.type === "BIGINT" || c.type === "DOUBLE") && !/(^|_)id$/i.test(c.name)).map((c) => c.name);
        const cats = cols.filter((c) => c.type === "VARCHAR" || c.type === "BOOLEAN").map((c) => c.name);
        return cats[0] && nums[0]
          ? { type, test: "compare_groups", a: nums[0], b: cats[0] }
          : { type, test: "correlation", a: nums[0] ?? first, b: nums[1] ?? first };
      }
      case "check": {
        const key = cols.find((c) => /(^|_)id$/i.test(c.name))?.name ?? first;
        return {
          type,
          stop: false,
          rules: [
            { rule: "expect", formula: numeric ? `${ref(numeric)} is not missing` : "true" },
            { rule: "unique", columns: key ? [key] : [] },
          ],
        };
      }
    }
  }

  const choices: { type: StepOp["type"]; name: string; hint: string }[] = [
    { type: "filter", name: "Filter rows", hint: "Visual" },
    { type: "derive", name: "Calculate columns", hint: "Visual" },
    { type: "select", name: "Choose and rename columns", hint: "Visual" },
    { type: "sort", name: "Sort rows, keep the top", hint: "Visual" },
    { type: "aggregate", name: "Group and summarize", hint: "Visual" },
    { type: "window", name: "Running totals, ranks, previous values", hint: "Visual" },
    { type: "dedupe", name: "Remove duplicates", hint: "Visual" },
    { type: "pivot", name: "Pivot: values to columns", hint: "Visual" },
    { type: "unpivot", name: "Unpivot: columns to rows", hint: "Visual" },
    { type: "join", name: "Join with another table", hint: "Visual" },
    { type: "append", name: "Append another table's rows", hint: "Visual" },
    { type: "check", name: "Check data quality", hint: "Visual" },
    { type: "test", name: "Statistical test", hint: "Visual" },
    { type: "chart", name: "Chart", hint: "Visual" },
    { type: "sql", name: "SQL", hint: "Code" },
    { type: "python", name: "Python", hint: "Code" },
    { type: "r", name: "R", hint: "Code" },
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
            {@const needsTwo = c.type === "join" || c.type === "append"}
            {@const blocked = needsTwo && !joinPartner}
            <DropdownMenu.Item
              class="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 outline-none data-[disabled]:cursor-default data-[disabled]:opacity-50 data-[highlighted]:bg-select"
              disabled={blocked}
              onSelect={() => ws.addStep(starter(c.type), null, needsTwo && joinPartner ? [joinPartner.id] : [])}
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
            {#if t.source.format === "excel"}
              <Sheet size={15} class="mt-0.5 shrink-0 text-ink-soft" />
            {:else}
              <Database size={15} class="mt-0.5 shrink-0 text-ink-soft" />
            {/if}
            <span class="min-w-0 flex-1">
              <span class="block truncate font-semibold">{t.source.path}</span>
              <span class="block text-xs text-ink-faint">
                {#if t.source.query}{"query, "}{:else if t.source.sheet}{`${t.source.sheet}, `}{/if}{formatCount(t.source.rows)} {t.source.rows === 1 ? "row" : "rows"}{#if (ws.view?.sources.length ?? 0) > 1}, as <code class="code">{t.source.id}</code>{/if}
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
                  {:else if ws.problemsFor(s.id).some((p) => p.severity === "error")}
                    <span class="text-failed">Has a problem</span>
                  {:else if s.state === "blocked"}
                    Waiting on a failed step
                  {:else if s.state === "needs review"}
                    <span class="text-review">Needs your review</span>
                  {:else if s.state === "waiting on review"}
                    Waiting on a review
                  {:else if s.rows !== null && s.mode !== "chart"}
                    {formatCount(s.rows)} {s.rows === 1 ? "row" : "rows"}{s.state === "stale"
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
              {#if ws.openComments(`step:${s.id}`)}
                <span class="flex items-center gap-0.5 text-xs text-ink-soft" title="Open comments">
                  <MessageSquare size={12} aria-hidden="true" />{ws.openComments(`step:${s.id}`)}
                  <span class="sr-only">open comments</span>
                </span>
              {/if}
              {#if s.mode === "sql" || s.mode === "python" || s.mode === "r"}
                <span class="tag" title="Edited as code, not with visual controls">{{ sql: "SQL", python: "Py", r: "R" }[s.mode]}</span>
              {/if}
            </button>
          </li>
        {/each}
      </ol>
    {/each}
    <button class="btn btn-quiet mt-1 w-full justify-start !px-2" disabled={ws.running} onclick={() => ws.addSource()}>
      <FilePlus2 size={15} /> Add source
    </button>
    <ParametersPanel {ws} />
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
