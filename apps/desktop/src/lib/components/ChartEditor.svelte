<script lang="ts">
  import type { Column, Mark } from "../api/types";
  import ColumnPicker from "./ColumnPicker.svelte";

  let {
    mark = $bindable(),
    x = $bindable(),
    y = $bindable(),
    title = $bindable(),
    columns,
  }: { mark: Mark; x: string; y: string; title: string | null; columns: Column[] | null } = $props();

  const marks: { m: Mark; words: string }[] = [
    { m: "bar", words: "Bars" },
    { m: "line", words: "Line" },
    { m: "point", words: "Points" },
  ];
</script>

<div class="grid max-w-lg grid-cols-[8rem_1fr] items-center gap-x-3 gap-y-2.5">
  <label for="chart-mark" class="text-sm text-ink-soft">Draw as</label>
  <select id="chart-mark" class="field w-40" bind:value={mark}>
    {#each marks as m (m.m)}<option value={m.m}>{m.words}</option>{/each}
  </select>
  <span class="text-sm text-ink-soft">Horizontal axis</span>
  <ColumnPicker bind:value={() => x, (v) => (x = v ?? "")} {columns} label="Horizontal axis" />
  <span class="text-sm text-ink-soft">Vertical axis</span>
  <ColumnPicker bind:value={() => y, (v) => (y = v ?? "")} {columns} label="Vertical axis" />
  <label for="chart-title" class="text-sm text-ink-soft">Title</label>
  <input
    id="chart-title"
    class="field"
    placeholder="{y} by {x}"
    value={title ?? ""}
    oninput={(e) => (title = e.currentTarget.value || null)}
  />
</div>
