<script lang="ts">
  import { CircleCheck, CircleX, Plus, X } from "@lucide/svelte";
  import type { CheckReport, Column, Problem, Rule } from "../api/types";
  import ColumnChips from "./ColumnChips.svelte";

  let {
    rules = $bindable(),
    stop = $bindable(),
    columns,
    problems,
    report,
  }: {
    rules: Rule[];
    stop: boolean;
    columns: Column[] | null;
    problems: Record<string, Problem[]>;
    /** The last run's results, when they match these rules. */
    report: CheckReport | null;
  } = $props();

  const kinds: { k: Rule["rule"]; words: string }[] = [
    { k: "expect", words: "Every row meets" },
    { k: "unique", words: "No two rows share" },
    { k: "rows", words: "The number of rows is" },
  ];

  function setKind(i: number, k: Rule["rule"]) {
    const first = columns?.[0]?.name;
    rules[i] = k === "expect" ? { rule: k, formula: "" } : k === "unique" ? { rule: k, columns: first ? [first] : [] } : { rule: k, min: 1, max: null };
  }

  function bound(v: string): number | null {
    const n = Math.round(Number(v));
    return v.trim() === "" || !Number.isFinite(n) || n < 0 ? null : n;
  }
</script>

<div class="space-y-4 text-sm">
  <ol class="space-y-3" aria-label="Rules">
    {#each rules as r, i (i)}
      {@const result = report?.results.find((x) => x.rule === i)}
      <li class="rounded-md border border-rule px-3 py-2">
        <div class="flex flex-wrap items-start gap-2">
          <select class="field" aria-label="Kind of rule" value={r.rule} onchange={(e) => setKind(i, e.currentTarget.value as Rule["rule"])}>
            {#each kinds as k (k.k)}<option value={k.k}>{k.words}</option>{/each}
          </select>
          {#if r.rule === "expect"}
            <div class="min-w-64 flex-1">
              <input class="field code w-full" aria-label="Rule formula" placeholder="units > 0 and region is not missing" bind:value={r.formula} />
              <p class="mt-0.5 text-xs text-ink-faint">A formula that is true for a good row. A missing result counts as not met.</p>
            </div>
          {:else if r.rule === "unique"}
            <div class="flex-1"><ColumnChips bind:value={r.columns} {columns} label="Columns that should not repeat" /></div>
          {:else}
            <label class="flex items-center gap-1.5">
              <span class="text-ink-soft">at least</span>
              <input class="field w-24" type="number" min="0" aria-label="Fewest rows" value={r.min ?? ""} onchange={(e) => (r.min = bound(e.currentTarget.value))} />
            </label>
            <label class="flex items-center gap-1.5">
              <span class="text-ink-soft">and at most</span>
              <input class="field w-24" type="number" min="0" aria-label="Most rows" placeholder="any" value={r.max ?? ""} onchange={(e) => (r.max = bound(e.currentTarget.value))} />
            </label>
          {/if}
          <button class="btn btn-quiet ml-auto !px-1.5" aria-label="Remove this rule" onclick={() => (rules = rules.filter((_, j) => j !== i))}>
            <X size={14} />
          </button>
        </div>
        {#each problems[`rules.${i}`] ?? [] as p, k (k)}
          {#if p.code !== "check-failed"}<p class="mt-1 text-xs text-failed" role="alert">{p.message}</p>{/if}
        {/each}
        {#if result}
          <div class="mt-2 flex items-start gap-1.5 {result.passed ? 'text-fresh' : 'text-failed'}">
            {#if result.passed}<CircleCheck size={15} class="mt-0.5 shrink-0" />{:else}<CircleX size={15} class="mt-0.5 shrink-0" />{/if}
            <span>{result.message}</span>
          </div>
          {#if result.examples.length}
            <div class="mt-1.5 overflow-x-auto">
              <table class="text-xs tabular-nums" aria-label="Rows that break rule {i + 1}">
                <thead class="text-ink-soft">
                  <tr>{#each result.columns as c (c)}<th class="pr-4 text-left font-semibold">{c}</th>{/each}</tr>
                </thead>
                <tbody>
                  {#each result.examples as row, k (k)}
                    <tr>{#each row as v, j (j)}<td class="pr-4" class:text-ink-faint={v === "missing"}>{v}</td>{/each}</tr>
                  {/each}
                </tbody>
              </table>
              {#if result.failing > result.examples.length}<p class="mt-0.5 text-xs text-ink-faint">First {result.examples.length} of {result.failing.toLocaleString("en-US")}.</p>{/if}
            </div>
          {/if}
        {/if}
      </li>
    {/each}
  </ol>
  <button class="btn btn-quiet !px-2" onclick={() => (rules = [...rules, { rule: "expect", formula: "" }])}><Plus size={14} /> Add a rule</button>

  <fieldset>
    <legend class="mb-1.5 font-semibold">When a rule is not met</legend>
    <label class="flex items-center gap-2">
      <input type="radio" name="check-stop" checked={!stop} onchange={() => (stop = false)} />
      Warn in the console, and pass the rows on
    </label>
    <label class="mt-1 flex items-center gap-2">
      <input type="radio" name="check-stop" checked={stop} onchange={() => (stop = true)} />
      Stop: this step fails, and the steps after it wait
    </label>
  </fieldset>
</div>
