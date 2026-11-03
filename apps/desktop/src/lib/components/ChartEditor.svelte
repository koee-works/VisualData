<script lang="ts">
  import type { AggFunc, Column, Mark } from "../api/types";
  import { funcWords, isNumericType } from "../format";
  import ColumnPicker from "./ColumnPicker.svelte";

  let {
    mark = $bindable(),
    x = $bindable(),
    y = $bindable(),
    title = $bindable(),
    color = $bindable(),
    summary = $bindable(),
    columns,
  }: {
    mark: Mark;
    x: string;
    y: string;
    title: string | null;
    color: string | null | undefined;
    summary: AggFunc | null | undefined;
    columns: Column[] | null;
  } = $props();

  const marks: { m: Mark; words: string; hint: string }[] = [
    { m: "bar", words: "Bars", hint: "Compare groups" },
    { m: "line", words: "Line", hint: "A trend over time" },
    { m: "area", words: "Area", hint: "A trend, stacked by color" },
    { m: "point", words: "Points", hint: "How two numbers relate" },
    { m: "histogram", words: "Histogram", hint: "How one number is spread" },
    { m: "boxplot", words: "Box plot", hint: "Spread per group" },
  ];
  const summaries: AggFunc[] = ["SUM", "AVG", "MEDIAN", "COUNT", "MIN", "MAX", "COUNT_DISTINCT"];

  let kind = $derived(mark === "histogram" ? "histogram" : mark === "boxplot" ? "box" : "xy");
  const numeric = (c: Column) => isNumericType(c.type);

  function setMark(m: Mark) {
    mark = m;
    if (m === "histogram" || m === "boxplot") {
      color = null;
      summary = null;
    }
    if (m === "histogram" && !columns?.some((c) => c.name === x && numeric(c))) x = columns?.find(numeric)?.name ?? x;
    if (m === "boxplot" && !columns?.some((c) => c.name === y && numeric(c))) y = columns?.find(numeric)?.name ?? y;
  }
</script>

<div class="grid max-w-xl grid-cols-[9rem_1fr] items-center gap-x-3 gap-y-2.5 text-sm">
  <label for="chart-mark" class="text-ink-soft">Draw as</label>
  <div class="flex items-center gap-2">
    <select id="chart-mark" class="field w-40" value={mark} onchange={(e) => setMark(e.currentTarget.value as Mark)}>
      {#each marks as m (m.m)}<option value={m.m}>{m.words}</option>{/each}
    </select>
    <span class="text-ink-faint">{marks.find((m) => m.m === mark)?.hint}</span>
  </div>

  {#if kind === "histogram"}
    <span class="text-ink-soft">Count the values of</span>
    <ColumnPicker bind:value={() => x, (v) => (x = v ?? "")} {columns} label="Column to count" filter={numeric} />
    <span></span>
    <p class="text-ink-soft">Bars of a round width, about twenty across, counting every row.</p>
  {:else if kind === "box"}
    <span class="text-ink-soft">Spread of</span>
    <ColumnPicker bind:value={() => y, (v) => (y = v ?? "")} {columns} label="Column whose spread is shown" filter={numeric} />
    <span class="text-ink-soft">For each</span>
    <ColumnPicker bind:value={() => x || null, (v) => (x = v ?? "")} {columns} label="Group column" allowNone="Everything together" />
    <span></span>
    <p class="text-ink-soft">Lines run from the smallest to the largest value; the box holds the middle half; the tick is the median.</p>
  {:else}
    <span class="text-ink-soft">Along the bottom</span>
    <ColumnPicker bind:value={() => x, (v) => (x = v ?? "")} {columns} label="Horizontal axis" />
    <label for="chart-summary" class="text-ink-soft">Each {mark === "bar" ? "bar" : "point"} shows</label>
    <div class="flex flex-wrap items-center gap-2">
      <select
        id="chart-summary"
        class="field"
        aria-label="Summary"
        value={summary ?? ""}
        onchange={(e) => (summary = (e.currentTarget.value || null) as AggFunc | null)}
      >
        <option value="">each row's value</option>
        {#each summaries as f (f)}<option value={f}>{f === "COUNT" ? "the number of rows" : `the ${funcWords(f).toLowerCase()}`}</option>{/each}
      </select>
      {#if summary !== "COUNT"}
        <span class="text-ink-soft">of</span>
        <ColumnPicker
          bind:value={() => y, (v) => (y = v ?? "")}
          {columns}
          label="Vertical axis"
          filter={summary && summary !== "MIN" && summary !== "MAX" && summary !== "COUNT_DISTINCT" ? numeric : undefined}
        />
      {/if}
    </div>
    <span class="text-ink-soft">Color by</span>
    <ColumnPicker bind:value={() => color ?? null, (v) => (color = v)} {columns} label="Color" allowNone="One color" filter={(c) => c.name !== x} />
    {#if summary}
      <span></span>
      <p class="text-ink-soft">Computed over every row, then drawn.{#if color && mark === "bar"} Bars stack by {color}.{/if}</p>
    {:else}
      <span></span>
      <p class="text-ink-soft">Draws up to 5,000 rows. Choose a summary to draw totals of every row.</p>
    {/if}
  {/if}

  <label for="chart-title" class="text-ink-soft">Title</label>
  <input id="chart-title" class="field" placeholder="Made from the columns" value={title ?? ""} oninput={(e) => (title = e.currentTarget.value || null)} />
</div>
