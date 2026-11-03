<script lang="ts">
  import { Plus, X } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { Literal, Parameter } from "../api/types";
  import { literalText, parseLiteral } from "../format";

  // Named values steps share: @name in formulas and filters, $name in SQL,
  // params["name"] in Python. A new value outdates the steps that use it.
  let { ws }: { ws: Workspace } = $props();

  let params = $derived(ws.view?.parameters ?? []);
  type Kind = "number" | "text" | "bool" | "date";
  const kindOf = (v: Literal): Kind =>
    typeof v === "boolean" ? "bool" : typeof v === "string" ? "text" : v && "date" in v ? "date" : "number";
  const defaults: Record<Kind, Literal> = { number: { int: "10" }, text: "East", bool: true, date: { date: new Date().toISOString().slice(0, 10) } };

  let adding = $state(false);
  let newName = $state("");
  let newKind = $state<Kind>("number");
  let errors = $state<Record<string, string>>({});

  function parse(kind: Kind, raw: string): Literal | { error: string } {
    if (kind === "number") return parseLiteral(raw, "DOUBLE");
    if (kind === "date") return parseLiteral(raw, "DATE");
    if (kind === "bool") return raw === "true";
    return raw;
  }

  async function setValue(p: Parameter, raw: string) {
    const v = parse(kindOf(p.value), raw);
    if (v !== null && typeof v === "object" && "error" in v) {
      errors = { ...errors, [p.name]: v.error };
      return;
    }
    errors = { ...errors, [p.name]: "" };
    if (JSON.stringify(v) === JSON.stringify(p.value)) return;
    await ws.setParameters(params.map((q) => (q.name === p.name ? { name: q.name, value: v } : q)));
  }

  async function add() {
    const name = newName.trim();
    if (await ws.setParameters([...params, { name, value: defaults[newKind] }])) {
      adding = false;
      newName = "";
    }
  }

  async function rename(p: Parameter, name: string) {
    if (name && name !== p.name) await ws.renameParameter(p.name, name);
  }
</script>

<section class="mt-4 border-t border-rule px-1 pt-3" aria-label="Parameters">
  <div class="flex items-center justify-between px-1">
    <h2 class="text-sm font-semibold text-ink-soft">Parameters</h2>
    <button class="btn btn-quiet !min-h-7 !px-1.5" aria-label="Add a parameter" disabled={ws.running} onclick={() => (adding = !adding)}>
      <Plus size={15} />
    </button>
  </div>
  {#if !params.length && !adding}
    <p class="px-1 pt-1 text-xs text-ink-faint">
      Values several steps share, like a start date or a threshold. Use them as @name in formulas and filters, $name in SQL.
    </p>
  {/if}
  <ul class="space-y-1.5 pt-1.5">
    {#each params as p (p.name)}
      {@const kind = kindOf(p.value)}
      <li class="px-1">
        <div class="flex items-center gap-1.5">
          <input
            class="field code !min-h-7 w-24 shrink-0 !px-1.5 text-xs"
            aria-label="Name of parameter {p.name}"
            value={p.name}
            onchange={(e) => rename(p, e.currentTarget.value.trim())}
          />
          {#if kind === "bool"}
            <select class="field !min-h-7 min-w-0 flex-1 !px-1.5 text-xs" aria-label="Value of {p.name}" value={String(p.value)} onchange={(e) => setValue(p, e.currentTarget.value)}>
              <option value="true">true</option>
              <option value="false">false</option>
            </select>
          {:else}
            <input
              class="field !min-h-7 min-w-0 flex-1 !px-1.5 text-xs"
              type={kind === "date" ? "date" : "text"}
              aria-label="Value of {p.name}"
              value={literalText(p.value)}
              onchange={(e) => setValue(p, e.currentTarget.value)}
            />
          {/if}
          <button
            class="btn btn-quiet !min-h-7 !px-1"
            aria-label="Remove parameter {p.name}"
            onclick={() => ws.setParameters(params.filter((q) => q.name !== p.name))}
          >
            <X size={13} />
          </button>
        </div>
        {#if errors[p.name]}<p class="mt-0.5 text-xs text-failed">{errors[p.name]}</p>{/if}
      </li>
    {/each}
  </ul>
  {#if adding}
    <form
      class="mt-2 flex items-center gap-1.5 px-1"
      onsubmit={(e) => {
        e.preventDefault();
        add();
      }}
    >
      <input class="field code !min-h-7 w-24 !px-1.5 text-xs" aria-label="New parameter name" placeholder="start_date" bind:value={newName} />
      <select class="field !min-h-7 min-w-0 flex-1 !px-1.5 text-xs" aria-label="Kind of value" bind:value={newKind}>
        <option value="number">number</option>
        <option value="text">text</option>
        <option value="date">date</option>
        <option value="bool">true/false</option>
      </select>
      <button class="btn !min-h-7 !px-2 text-xs" disabled={!newName.trim()}>Add</button>
    </form>
  {/if}
</section>
