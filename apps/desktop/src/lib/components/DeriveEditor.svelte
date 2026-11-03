<script lang="ts">
  import { Plus, X } from "@lucide/svelte";
  import type { Column } from "../api/types";

  let {
    columns = $bindable(),
    inputColumns,
  }: { columns: { name: string; formula: string }[]; inputColumns: Column[] | null } = $props();

  // Grouped the way people look for them; mirrors crates/vda-core/src/formula.rs.
  const reference: { group: string; items: [string, string][] }[] = [
    {
      group: "Numbers",
      items: [
        ["+ - * / %", "arithmetic; dividing by zero gives a missing value"],
        ["round(x, 2)", "round half away from zero; floor(x), ceil(x), abs(x)"],
        ["sqrt(x), power(x, y)", "missing when there is no real answer"],
      ],
    },
    {
      group: "Logic and missing values",
      items: [
        ["if(cond, then, else)", "a missing condition takes the else branch"],
        ["coalesce(a, b, …)", "the first value that is not missing"],
        ["x is missing", "or x is not missing; and, or, not; = <> < <= > >="],
      ],
    },
    {
      group: "Text",
      items: [
        ['a & " " & b', "join text (missing if either is); concat(a, b) skips missing"],
        ["upper, lower, trim, length", "trim removes spaces at both ends"],
        ["contains, starts_with, ends_with", "true/false"],
        ["replace(s, from, to), substr(s, start, len)", "positions count from 1"],
      ],
    },
    {
      group: "Changing type",
      items: [
        ["text(x), format(x, 2)", "format writes a number with fixed decimals"],
        ["number(x), integer(x), date(x)", "missing when the text is not a number or ISO date"],
      ],
    },
    {
      group: "Dates",
      items: [
        ['date_trunc("month", d)', "start of the year, quarter, month, week (Monday), day, hour, minute"],
        ["year, quarter, month, day, weekday", "weekday: Monday is 1; hour and minute for times"],
        ["days_between(a, b), add_days(d, n)", ""],
      ],
    },
  ];
  let showHelp = $state(false);

  function names(i: number) {
    // Columns a formula can use: the input's, then earlier ones here.
    const seen = (inputColumns ?? []).map((c) => c.name);
    for (const c of columns.slice(0, i)) if (c.name && !seen.includes(c.name)) seen.push(c.name);
    return seen;
  }
</script>

<div class="space-y-4 text-sm">
  <ol class="space-y-3">
    {#each columns as c, i (i)}
      {@const replaces = (inputColumns ?? []).some((x) => x.name === c.name)}
      <li class="rounded-md border border-rule px-3 py-2.5">
        <div class="flex flex-wrap items-center gap-2">
          <label class="flex items-center gap-2">
            <span class="text-ink-soft">Column</span>
            <input class="field w-48" aria-label="New column name" bind:value={c.name} placeholder="name" />
          </label>
          {#if replaces}<span class="text-xs text-stale">replaces the existing column</span>{/if}
          <button
            class="btn btn-quiet ml-auto !px-1.5"
            aria-label="Remove this column"
            disabled={columns.length === 1}
            onclick={() => (columns = columns.filter((_, j) => j !== i))}
          >
            <X size={14} />
          </button>
        </div>
        <label class="mt-2 block">
          <span class="sr-only">Formula</span>
          <textarea
            class="field code h-16 w-full resize-y"
            spellcheck="false"
            aria-label="Formula"
            placeholder="units * unit_price"
            bind:value={c.formula}
          ></textarea>
        </label>
        {#if names(i).length}
          <p class="mt-1 text-xs text-ink-faint">
            Can use: {names(i).map((n) => (/^[A-Za-z_][A-Za-z0-9_]*$/.test(n) ? n : `[${n}]`)).join(", ")}
          </p>
        {/if}
      </li>
    {/each}
  </ol>
  <div class="flex flex-wrap items-center gap-2">
    <button class="btn btn-quiet !px-2" onclick={() => (columns = [...columns, { name: "", formula: "" }])}>
      <Plus size={14} /> Add another column
    </button>
    <button class="btn btn-quiet !px-2" aria-expanded={showHelp} onclick={() => (showHelp = !showHelp)}>
      {showHelp ? "Hide" : "Show"} what formulas can do
    </button>
  </div>
  {#if showHelp}
    <div class="grid gap-x-6 gap-y-3 rounded-md bg-paper px-4 py-3 md:grid-cols-2">
      {#each reference as r (r.group)}
        <section>
          <h3 class="mb-1 font-semibold">{r.group}</h3>
          <dl class="space-y-1">
            {#each r.items as [code, words] (code)}
              <div>
                <dt class="code inline">{code}</dt>
                {#if words}<dd class="inline text-ink-soft"> — {words}</dd>{/if}
              </div>
            {/each}
          </dl>
        </section>
      {/each}
      <p class="text-ink-soft md:col-span-2">
        Write column names with spaces in brackets: <span class="code">[Unit price]</span>. Text goes in quotes. A column
        here can replace one with the same name, for example <span class="code">coalesce(region, "Unknown")</span>.
      </p>
    </div>
  {/if}
</div>
