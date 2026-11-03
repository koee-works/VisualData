<script lang="ts">
  import { X, Plus } from "@lucide/svelte";
  import type { CmpOp, Column, Parameter, Predicate } from "../api/types";
  import { literalText, parseLiteral } from "../format";
  import ColumnPicker from "./ColumnPicker.svelte";

  let {
    predicates = $bindable(),
    columns,
    invalid = $bindable(false),
    parameters = [],
  }: { predicates: Predicate[]; columns: Column[] | null; invalid?: boolean; parameters?: Parameter[] } = $props();

  const ops: { op: CmpOp; words: string }[] = [
    { op: "=", words: "is" },
    { op: "<>", words: "is not" },
    { op: ">", words: "is above" },
    { op: ">=", words: "is at least" },
    { op: "<", words: "is below" },
    { op: "<=", words: "is at most" },
    { op: "IS NOT NULL", words: "is present" },
    { op: "IS NULL", words: "is missing" },
  ];

  // What the user typed, per row; converted to typed literals on change.
  let raw = $state<string[]>([]);
  let errors = $state<(string | null)[]>([]);

  $effect.pre(() => {
    if (raw.length !== predicates.length) {
      raw = predicates.map((p) => literalText(p.value));
      errors = predicates.map(() => null);
    }
  });

  function typeOf(column: string) {
    return columns?.find((c) => c.name === column)?.type ?? "VARCHAR";
  }

  function commit(i: number) {
    const p = predicates[i]!;
    if (p.op === "IS NULL" || p.op === "IS NOT NULL") {
      p.value = null;
      errors[i] = null;
    } else {
      const parsed = parseLiteral(raw[i] ?? "", typeOf(p.column));
      if (parsed !== null && typeof parsed === "object" && "error" in parsed) {
        errors[i] = parsed.error;
      } else {
        errors[i] = null;
        p.value = parsed;
      }
    }
    invalid = errors.some((e) => e !== null);
  }

  function add() {
    predicates.push({ column: columns?.[0]?.name ?? "", op: "IS NOT NULL", value: null });
    raw.push("");
    errors.push(null);
  }

  function remove(i: number) {
    predicates.splice(i, 1);
    raw.splice(i, 1);
    errors.splice(i, 1);
    invalid = errors.some((e) => e !== null);
  }
</script>

<fieldset class="space-y-2">
  <legend class="mb-2 text-sm text-ink-soft">Keep rows where all of these hold</legend>
  {#each predicates as p, i (i)}
    <div class="flex flex-wrap items-start gap-2">
      <ColumnPicker bind:value={() => p.column, (v) => { p.column = v ?? ""; commit(i); }} {columns} label="Column" />
      <select class="field" aria-label="Condition" bind:value={p.op} onchange={() => commit(i)}>
        {#each ops as o (o.op)}<option value={o.op}>{o.words}</option>{/each}
      </select>
      {#if p.op !== "IS NULL" && p.op !== "IS NOT NULL" && (parameters.length || p.param)}
        <select
          class="field"
          aria-label="Compare with"
          value={p.param ?? ""}
          onchange={(e) => {
            p.param = e.currentTarget.value || null;
            if (p.param) {
              errors[i] = null;
              invalid = errors.some((x) => x !== null);
            } else commit(i);
          }}
        >
          <option value="">a value</option>
          {#each parameters as q (q.name)}<option value={q.name}>@{q.name} ({literalText(q.value)})</option>{/each}
          {#if p.param && !parameters.some((q) => q.name === p.param)}<option value={p.param}>@{p.param} (missing)</option>{/if}
        </select>
      {/if}
      {#if p.op !== "IS NULL" && p.op !== "IS NOT NULL" && !p.param}
        <div class="flex flex-col">
          <input
            class="field w-40"
            aria-label="Value"
            aria-invalid={errors[i] ? "true" : "false"}
            bind:value={raw[i]}
            oninput={() => commit(i)}
          />
          {#if errors[i]}<span class="mt-0.5 text-xs text-failed">{errors[i]}</span>{/if}
        </div>
      {/if}
      <button class="btn btn-quiet !px-1.5" aria-label="Remove condition" onclick={() => remove(i)}><X size={15} /></button>
    </div>
  {/each}
  <button class="btn btn-quiet !px-2" onclick={add}><Plus size={15} /> Add condition</button>
</fieldset>
