<script lang="ts">
  import { X } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";

  let {
    ws,
    inputs = $bindable(),
    distinct = $bindable(),
    origin = $bindable(),
    choices,
  }: {
    ws: Workspace;
    /** Tables to stack, in order. */
    inputs: string[];
    distinct: boolean;
    origin: string | null;
    /** Tables that can be added without depending on this step. */
    choices: { id: string; name: string }[];
  } = $props();

  let addable = $derived(choices.filter((c) => !inputs.includes(c.id)));
  let tagging = $derived(origin !== null);

  // Columns each table contributes, to show what will line up.
  let columns = $derived.by(() => {
    const all: string[] = [];
    for (const id of inputs) for (const c of ws.columnsOf(id) ?? []) if (!all.includes(c.name)) all.push(c.name);
    return all;
  });
  let partial = $derived(
    columns.filter((name) => inputs.some((id) => ws.columnsOf(id) && !ws.columnsOf(id)!.some((c) => c.name === name))),
  );
</script>

<div class="space-y-5 text-sm">
  <fieldset>
    <legend class="mb-1.5 font-semibold">Stack the rows of</legend>
    <ol class="space-y-1.5">
      {#each inputs as id, i (id)}
        <li class="flex items-center gap-2">
          <span class="w-5 text-right text-ink-faint">{i + 1}.</span>
          <span class="min-w-0 flex-1 truncate">{ws.nameOf(id)} <code class="code text-ink-faint">{id}</code></span>
          <button
            class="btn btn-quiet !px-1.5"
            aria-label="Stop appending {ws.nameOf(id)}"
            disabled={inputs.length <= 2}
            title={inputs.length <= 2 ? "Appending needs at least two tables" : undefined}
            onclick={() => (inputs = inputs.filter((x) => x !== id))}
          >
            <X size={14} />
          </button>
        </li>
      {/each}
    </ol>
    {#if addable.length}
      <select
        class="field mt-2"
        aria-label="Add a table"
        value=""
        onchange={(e) => {
          const v = e.currentTarget.value;
          if (v) inputs = [...inputs, v];
          e.currentTarget.value = "";
        }}
      >
        <option value="">Add a table…</option>
        {#each addable as o (o.id)}<option value={o.id}>{o.name}</option>{/each}
      </select>
    {/if}
  </fieldset>

  <div class="space-y-3">
    <label class="flex items-start gap-2">
      <input type="checkbox" class="mt-0.5 accent-[var(--color-running)]" bind:checked={distinct} />
      <span>
        <span class="block font-semibold">Remove repeated rows</span>
        <span class="block text-ink-soft">A row identical in every column is kept once, where it first appears.</span>
      </span>
    </label>
    <label class="flex items-start gap-2">
      <input
        type="checkbox"
        class="mt-0.5 accent-[var(--color-running)]"
        checked={tagging}
        onchange={(e) => (origin = e.currentTarget.checked ? "source_table" : null)}
      />
      <span>
        <span class="block font-semibold">Record which table each row came from</span>
        <span class="block text-ink-soft">Adds a text column holding each table's name in this analysis.</span>
      </span>
    </label>
    {#if origin !== null}
      <label class="ml-6 flex items-center gap-2">
        <span class="text-ink-soft">Column name</span>
        <input class="field w-48" aria-label="Origin column name" bind:value={origin} />
      </label>
    {/if}
  </div>

  <p class="max-w-[64ch] text-ink-soft">
    Columns are matched by name, in the first table's order, then any new ones. Whole numbers and decimals combine as
    decimals, dates and times as times; other differences must be fixed first.
    {#if partial.length}
      Not every table has {partial.map((c) => `"${c}"`).join(", ")}; those rows show it as missing.
    {/if}
  </p>
</div>
