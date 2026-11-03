<script lang="ts">
  import { untrack } from "svelte";
  import { Trash2 } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { Column, StepOp, StepView } from "../api/types";
  import { SOURCE_ID } from "../api/types";
  import { describe, stepTitle } from "../format";
  import FilterEditor from "./FilterEditor.svelte";
  import AggregateEditor from "./AggregateEditor.svelte";
  import CodeEditor from "./CodeEditor.svelte";
  import ChartEditor from "./ChartEditor.svelte";

  let { ws, step }: { ws: Workspace; step: StepView } = $props();

  // A private copy of the definition; saved explicitly.
  const initial = untrack(() => ({
    op: structuredClone($state.snapshot(step.op)) as StepOp,
    label: step.label ?? "",
    id: step.id,
    json: JSON.stringify(step.op),
  }));
  let draft = $state<StepOp>(initial.op);
  let label = $state(initial.label);
  let invalid = $state(false);
  let saving = $state(false);
  let loadedFor = initial.id;
  let loadedOp = initial.json;

  // Reload when another step is selected or the saved definition changes.
  $effect.pre(() => {
    const opJson = JSON.stringify(step.op);
    if (step.id !== loadedFor || opJson !== loadedOp) {
      draft = structuredClone($state.snapshot(step.op)) as StepOp;
      label = step.label ?? "";
      invalid = false;
      loadedFor = step.id;
      loadedOp = opJson;
    }
  });

  let dirty = $derived(JSON.stringify(draft) !== JSON.stringify(step.op) || label !== (step.label ?? ""));

  let input = $derived(ws.view?.steps.find((s) => s.id === step.input) ?? null);
  let inputName = $derived(step.input === SOURCE_ID ? (ws.view?.source.path ?? "the source") : input ? stepTitle(input) : step.input);
  let inputColumns: Column[] | null = $derived(
    step.input === SOURCE_ID ? (ws.view?.source.columns ?? null) : (input?.columns ?? null),
  );

  async function save() {
    saving = true;
    await ws.saveStep(step.id, label.trim() || null, $state.snapshot(draft) as StepOp);
    saving = false;
  }

  function onkeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === "s") {
      e.preventDefault();
      if (dirty && !invalid) save();
    }
  }
</script>

<svelte:window {onkeydown} />

<section class="flex h-full flex-col" aria-label="Step editor">
  <header class="flex flex-wrap items-center gap-3 border-b border-rule px-5 py-3">
    <input
      class="field min-w-0 flex-1 text-lg font-semibold"
      aria-label="Step name"
      placeholder={describe(draft)}
      bind:value={label}
    />
    <button class="btn btn-primary" disabled={!dirty || invalid || saving} onclick={save}>
      {saving ? "Saving…" : "Save step"}
    </button>
    <button
      class="btn btn-quiet !px-2"
      disabled={step.dependents.length > 0 || ws.running}
      title={step.dependents.length ? "Other steps read this one; delete them first" : "Delete step"}
      aria-label="Delete step"
      onclick={() => ws.removeStep(step.id)}
    >
      <Trash2 size={16} />
    </button>
  </header>

  <div class="flex-1 overflow-y-auto px-5 py-4">
    <p class="mb-4 text-sm text-ink-soft">
      Reads <button class="underline decoration-rule-strong underline-offset-2 hover:text-ink" onclick={() => (ws.selected = step.input)}>{inputName}</button>{#if inputColumns === null}.
        Its columns appear after it runs, so type column names for now{/if}.
    </p>

    {#if step.state === "failed" && step.detail}
      <div class="mb-4 rounded-md border border-failed/40 bg-failed-wash px-3 py-2 text-sm" role="alert">
        <p class="font-semibold text-failed">This step failed on the last run</p>
        <p class="code mt-1 whitespace-pre-wrap">{step.detail}</p>
      </div>
    {:else if step.state === "blocked" && step.detail}
      <div class="mb-4 rounded-md border border-rule-strong bg-paper px-3 py-2 text-sm">
        Not run: {step.detail}. Fix that step and run again.
      </div>
    {/if}

    {#if draft.type === "filter"}
      <FilterEditor bind:predicates={draft.predicates} columns={inputColumns} bind:invalid />
    {:else if draft.type === "aggregate"}
      <AggregateEditor bind:groupBy={draft.group_by} bind:measures={draft.measures} columns={inputColumns} />
    {:else if draft.type === "sql"}
      <CodeEditor bind:code={draft.sql} language="sql" {inputName} />
    {:else if draft.type === "python"}
      <CodeEditor bind:code={draft.code} language="python" {inputName} />
      <label class="mt-3 flex items-center gap-2 text-sm text-ink-soft">
        Stop after
        <input
          class="field w-20"
          type="number"
          min="1"
          value={Math.round(draft.timeout_ms / 1000)}
          oninput={(e) => {
            if (draft.type === "python") draft.timeout_ms = Math.max(1, Number(e.currentTarget.value) || 60) * 1000;
          }}
        />
        seconds
      </label>
    {:else if draft.type === "chart"}
      <ChartEditor bind:mark={draft.mark} bind:x={draft.x} bind:y={draft.y} bind:title={draft.title} columns={inputColumns} />
    {/if}

    {#if dirty}
      <p class="mt-5 text-sm text-ink-soft">Unsaved changes. Saving marks this step and everything after it as outdated until the next run.</p>
    {/if}
  </div>
</section>
