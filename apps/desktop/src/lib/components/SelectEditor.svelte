<script lang="ts">
  import { ArrowDown, ArrowUp } from "@lucide/svelte";
  import type { Column } from "../api/types";
  import { typeLabel } from "../format";

  let {
    columns = $bindable(),
    inputColumns,
  }: { columns: { column: string; as: string | null }[]; inputColumns: Column[] | null } = $props();

  let kept = $derived(new Set(columns.map((c) => c.column)));
  // Columns of the input not kept, so they can be added back.
  let others = $derived((inputColumns ?? []).filter((c) => !kept.has(c.name)));

  function move(i: number, by: number) {
    const next = [...columns];
    const [c] = next.splice(i, 1);
    next.splice(i + by, 0, c!);
    columns = next;
  }
  const typeOf = (name: string) => inputColumns?.find((c) => c.name === name)?.type;
</script>

<div class="space-y-4 text-sm">
  <fieldset>
    <legend class="mb-1.5 font-semibold">Keep these columns, in this order</legend>
    <ol class="space-y-1.5">
      {#each columns as c, i (c.column)}
        {@const t = typeOf(c.column)}
        <li class="flex flex-wrap items-center gap-2">
          <span class="w-5 text-right text-ink-faint">{i + 1}.</span>
          <span class="w-44 truncate">{c.column}{#if t}<span class="text-ink-faint"> ({typeLabel[t]})</span>{/if}</span>
          <label class="flex items-center gap-1.5">
            <span class="text-ink-soft">as</span>
            <input
              class="field w-40"
              aria-label="New name for {c.column}"
              placeholder={c.column}
              value={c.as ?? ""}
              oninput={(e) => (c.as = e.currentTarget.value.trim() ? e.currentTarget.value : null)}
            />
          </label>
          <button class="btn btn-quiet !px-1.5" aria-label="Move {c.column} up" disabled={i === 0} onclick={() => move(i, -1)}>
            <ArrowUp size={14} />
          </button>
          <button
            class="btn btn-quiet !px-1.5"
            aria-label="Move {c.column} down"
            disabled={i === columns.length - 1}
            onclick={() => move(i, 1)}
          >
            <ArrowDown size={14} />
          </button>
          <button
            class="btn btn-quiet !px-2"
            disabled={columns.length === 1}
            onclick={() => (columns = columns.filter((x) => x.column !== c.column))}
          >
            Drop
          </button>
        </li>
      {/each}
    </ol>
  </fieldset>
  {#if others.length}
    <div>
      <p class="mb-1.5 text-ink-soft">Dropped</p>
      <div class="flex flex-wrap gap-1.5">
        {#each others as c (c.name)}
          <button class="btn !min-h-7" onclick={() => (columns = [...columns, { column: c.name, as: null }])}>+ {c.name}</button>
        {/each}
      </div>
    </div>
  {/if}
</div>
