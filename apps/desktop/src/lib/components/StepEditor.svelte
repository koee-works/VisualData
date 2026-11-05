<script lang="ts">
  import { tick, untrack } from "svelte";
  import { prefs } from "../prefs.svelte";
  import { ExternalLink, Trash2, X } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { Column, Problem, StepOp, StepView } from "../api/types";
  import { CircleX, TriangleAlert, Info } from "@lucide/svelte";
  import { describe, kindWords, stepTitle } from "../format";
  import FilterEditor from "./FilterEditor.svelte";
  import AggregateEditor from "./AggregateEditor.svelte";
  import CodeEditor from "./CodeEditor.svelte";
  import type { RunRequest } from "../editor/prefs";
  import { editorParameters } from "../editor/complete";
  import ChartEditor from "./ChartEditor.svelte";
  import JoinEditor from "./JoinEditor.svelte";
  import AppendEditor from "./AppendEditor.svelte";
  import DeriveEditor from "./DeriveEditor.svelte";
  import SelectEditor from "./SelectEditor.svelte";
  import SortEditor from "./SortEditor.svelte";
  import WindowEditor from "./WindowEditor.svelte";
  import DedupeEditor from "./DedupeEditor.svelte";
  import PivotEditor from "./PivotEditor.svelte";
  import UnpivotEditor from "./UnpivotEditor.svelte";
  import TestEditor from "./TestEditor.svelte";
  import Comments from "./Comments.svelte";
  import CheckEditor from "./CheckEditor.svelte";
  import CleanEditor from "./CleanEditor.svelte";
  import type { CheckReport, CleanReport, ColumnDetail } from "../api/types";

  let { ws, step }: { ws: Workspace; step: StepView } = $props();

  // A private copy of the definition; saved explicitly.
  const initial = untrack(() => ({
    op: structuredClone($state.snapshot(step.op)) as StepOp,
    label: step.label ?? "",
    id: step.id,
    json: JSON.stringify(step.op),
    inputs: [...step.inputs],
  }));
  let draft = $state<StepOp>(initial.op);
  let label = $state(initial.label);
  let inputs = $state<string[]>(initial.inputs);
  let loadedInputs = JSON.stringify(initial.inputs);
  let invalid = $state(false);
  let saving = $state(false);
  let loadedFor = initial.id;
  let loadedOp = initial.json;

  // Reload when another step is selected or the saved definition changes.
  $effect.pre(() => {
    const opJson = JSON.stringify(step.op);
    const inputsJson = JSON.stringify(step.inputs);
    if (step.id !== loadedFor || opJson !== loadedOp || inputsJson !== loadedInputs) {
      draft = structuredClone($state.snapshot(step.op)) as StepOp;
      label = step.label ?? "";
      inputs = [...step.inputs];
      invalid = false;
      loadedFor = step.id;
      loadedOp = opJson;
      loadedInputs = inputsJson;
    }
  });

  let inputsChanged = $derived(JSON.stringify(inputs) !== JSON.stringify(step.inputs));
  let dirty = $derived(JSON.stringify(draft) !== JSON.stringify(step.op) || label !== (step.label ?? "") || inputsChanged);

  let inputName = $derived(ws.nameOf(inputs[0]!));
  let inputColumns: Column[] | null = $derived(ws.columnsOf(inputs[0]!));
  // Tables this step may read without depending on itself.
  let readable = $derived(ws.readable(step.id));
  let extraChoices = $derived(readable.filter((o) => !inputs.includes(o.id)));

  // Checked as you type: a draft is sent to the linter (not saved) a moment
  // after the last change; the saved step's problems show otherwise.
  let draftProblems = $state<Problem[] | null>(null);
  $effect(() => {
    const op = JSON.stringify(draft);
    const ins = JSON.stringify(inputs);
    const id = step.id;
    if (!dirty) {
      draftProblems = null;
      return;
    }
    const timer = setTimeout(() => {
      ws.backend.checkDraft(id, JSON.parse(ins), JSON.parse(op)).then(
        (p) => {
          if (id === step.id && op === JSON.stringify(draft)) draftProblems = p;
        },
        () => {},
      );
    }, 300);
    return () => clearTimeout(timer);
  });
  let shown = $derived(
    // Notes (unused, outdated) belong in the console, not in the editor.
    (draftProblems ?? ws.problemsFor(step.id)).filter((p) => p.severity !== "info" && p.code !== "failed"),
  );
  let general = $derived(
    shown.filter((p) => !p.field?.startsWith("columns.") && !p.field?.startsWith("rules.") && p.field !== "code" && p.field !== "sql"),
  );

  // A check step's last report, shown beside its rules while they are unchanged.
  let report = $state<CheckReport | null>(null);
  $effect(() => {
    void ws.revision;
    const id = step.id;
    if (step.op.type !== "check" || step.state === "not run") {
      report = null;
      return;
    }
    ws.backend.checkReport(id).then((r) => id === step.id && (report = r), () => (report = null));
  });
  // A Clean step's last report, and its input's columns with missing counts.
  let cleanReport = $state<CleanReport | null>(null);
  let inputDetails = $state<ColumnDetail[] | null>(null);
  $effect(() => {
    void ws.revision;
    const id = step.id;
    const from = inputs[0]!;
    if (step.op.type !== "clean") {
      cleanReport = null;
      inputDetails = null;
      return;
    }
    if (step.state === "not run") cleanReport = null;
    else ws.backend.cleanReport(id).then((r) => id === step.id && (cleanReport = r), () => (cleanReport = null));
    ws.backend.columnDetails(from).then((d) => id === step.id && (inputDetails = d), () => (inputDetails = null));
  });
  let byField = $derived.by(() => {
    const m: Record<string, Problem[]> = {};
    for (const p of shown) if (p.field) (m[p.field] ??= []).push(p);
    return m;
  });
  const icons = { error: CircleX, warning: TriangleAlert, info: Info };

  async function save() {
    saving = true;
    const saved = await ws.saveStep(step.id, label.trim() || null, $state.snapshot(draft) as StepOp, inputsChanged ? [...inputs] : undefined);
    saving = false;
    // Settings → General: run after each save.
    if (saved && prefs.value.runAfterSave) await ws.run();
  }

  let codeInputs = $derived(inputs.map((id) => ({ id, name: ws.nameOf(id), columns: ws.columnsOf(id) })));

  let codeEditor: CodeEditor | undefined = $state();
  // The console lives in the right dock; it registers itself on `ws`.
  $effect(() => {
    ws.focusCodeLine = (n) => codeEditor?.focusLine(n);
    return () => (ws.focusCodeLine = null);
  });

  // Shift+Enter (and SQL's Ctrl+Enter): save and run the step. In Python and
  // R, Ctrl+Enter runs the line or selection in the console instead.
  async function runFromEditor(r: RunRequest) {
    const code = draft.type === "python" || draft.type === "r";
    if (code && r.what !== "step") {
      // The console was hidden: show it, then run there.
      if (!ws.panes.console) {
        ws.togglePane("console");
        await tick();
      }
      await ws.codeConsole?.run(r.code, r.fromLine);
      return;
    }
    if (saving || invalid) return;
    const wasDirty = dirty;
    if (wasDirty) await save();
    // Saving already ran it when the user asked for that in Settings.
    if (!(wasDirty && prefs.value.runAfterSave)) await ws.run();
    await ws.codeConsole?.compare();
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
    <span class="kind-tag" title="Kind of step">{kindWords[draft.type]}</span>
    <input
      class="field title-field min-w-0 flex-1 text-lg font-semibold"
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

  <div class="@container flex-1 overflow-y-auto px-5 py-4">
    <p class="mb-4 text-sm text-ink-soft">
      Reads
      {#each inputs as id, i (id)}{#if i > 0}{i === inputs.length - 1 ? " and " : ", "}{/if}<button
          class="underline decoration-rule-strong underline-offset-2 hover:text-ink"
          onclick={() => (ws.selected = id)}>{ws.nameOf(id)}</button
        >{/each}{#if inputColumns === null && draft.type !== "join"}.
        Its columns appear after it runs, so type column names for now{/if}.
    </p>

    {#if general.length}
      <ul class="mb-4 space-y-1 text-sm" aria-label="Problems in this step">
        {#each general as p, i (i)}
          {@const Icon = icons[p.severity]}
          <li class="flex items-start gap-2">
            <Icon size={15} class="mt-0.5 shrink-0 {p.severity === 'error' ? 'text-failed' : p.severity === 'warning' ? 'text-stale' : 'text-ink-faint'}" />
            <span>{p.message}{#if p.from_last_run}<span class="text-ink-faint"> (last run)</span>{/if}</span>
          </li>
        {/each}
      </ul>
    {/if}

    {#if step.state === "failed" && step.detail}
      <div class="mb-4 rounded-md border border-failed/40 bg-failed-wash px-3 py-2 text-sm" role="alert">
        <p class="font-semibold text-failed">This step failed on the last run</p>
        <p class="code mt-1 whitespace-pre-wrap">{step.detail}</p>
      </div>
    {:else if step.state === "needs review"}
      <div class="mb-4 flex flex-wrap items-center gap-3 rounded-md border border-review/40 bg-review-wash px-3 py-2 text-sm">
        <p class="max-w-[64ch] flex-1">
          <span class="font-semibold text-review">You haven't approved this code.</span> It came with the project and
          will run as you, with access to your files and the network. Read it below; it won't run until you approve it.
          Editing the code counts as writing it yourself.
        </p>
        <button class="btn" disabled={dirty} onclick={() => ws.approve([step.id])}>Approve</button>
      </div>
    {:else if step.state === "blocked" && step.detail}
      <div class="mb-4 rounded-md border border-rule-strong bg-paper px-3 py-2 text-sm">
        Not run: {step.detail}. Fix that step and run again.
      </div>
    {/if}

    {#if draft.type === "filter"}
      <FilterEditor bind:predicates={draft.predicates} columns={inputColumns} parameters={ws.view?.parameters ?? []} bind:invalid />
    {:else if draft.type === "aggregate"}
      <AggregateEditor bind:groupBy={draft.group_by} bind:measures={draft.measures} columns={inputColumns} />
    {:else if draft.type === "join"}
      <JoinEditor
        bind:how={draft.how}
        bind:on={draft.on}
        bind:right={inputs[1]!}
        leftName={inputName}
        leftColumns={inputColumns}
        rightOptions={readable.filter((o) => o.id !== inputs[0])}
        rightColumns={ws.columnsOf(inputs[1]!)}
        rightId={inputs[1]!}
      />
    {:else if draft.type === "derive"}
      <DeriveEditor bind:columns={draft.columns} {inputColumns} parameters={ws.view?.parameters ?? []} problems={byField} />
    {:else if draft.type === "select"}
      <SelectEditor bind:columns={draft.columns} {inputColumns} />
    {:else if draft.type === "sort"}
      <SortEditor bind:by={draft.by} bind:limit={draft.limit} columns={inputColumns} />
    {:else if draft.type === "window"}
      <WindowEditor bind:partitionBy={draft.partition_by} bind:orderBy={draft.order_by} bind:calcs={draft.columns} columns={inputColumns} />
    {:else if draft.type === "dedupe"}
      <DedupeEditor bind:keys={draft.keys} bind:orderBy={draft.order_by} columns={inputColumns} />
    {:else if draft.type === "pivot"}
      <PivotEditor bind:rows={draft.rows} bind:column={draft.column} bind:value={draft.value} bind:func={draft.func} columns={inputColumns} />
    {:else if draft.type === "unpivot"}
      <UnpivotEditor bind:picked={draft.columns} bind:name={draft.name} bind:value={draft.value} columns={inputColumns} />
    {:else if draft.type === "test"}
      <TestEditor bind:test={draft.test} bind:a={draft.a} bind:b={draft.b} columns={inputColumns} />
    {:else if draft.type === "clean"}
      <CleanEditor
        bind:markers={draft.markers}
        bind:columns={draft.columns}
        bind:flag={draft.flag}
        bind:seed={draft.seed}
        {inputColumns}
        details={inputDetails}
        parameters={ws.view?.parameters ?? []}
        report={dirty ? null : cleanReport}
        compare={(column) => ws.backend.compareFills(step.id, $state.snapshot(draft) as StepOp, column)}
      />
    {:else if draft.type === "check"}
      <CheckEditor bind:rules={draft.rules} bind:stop={draft.stop} columns={inputColumns} parameters={ws.view?.parameters ?? []} problems={byField} report={dirty ? null : report} />
    {:else if draft.type === "append"}
      <AppendEditor {ws} bind:inputs bind:distinct={draft.distinct} bind:origin={draft.origin} choices={readable} />
    {:else if draft.type === "sql" || draft.type === "python" || draft.type === "r"}
      <div class="code-layout">
        <div class="min-w-0">
        {#if draft.type === "sql"}
          <CodeEditor bind:code={draft.sql} language="sql" inputs={codeInputs} parameters={ws.view?.parameters ?? []} problems={byField["sql"] ?? []} onrun={runFromEditor} />
        {:else}
          <CodeEditor bind:this={codeEditor} bind:code={draft.code} language={draft.type} inputs={codeInputs} parameters={ws.view?.parameters ?? []} problems={byField["code"] ?? []} onrun={runFromEditor} />
        {/if}
        <div class="mt-3 flex flex-wrap items-center gap-2 text-sm" aria-label="More inputs">
          <span class="text-ink-soft">Also reads</span>
          {#each inputs.slice(1) as id (id)}
            <span class="inline-flex items-center gap-1 rounded border border-rule-strong py-0.5 pr-0.5 pl-2">
              <code class="code">{id}</code>
              <button class="btn btn-quiet !min-h-6 !min-w-6 justify-center !px-1" aria-label="Stop reading {ws.nameOf(id)}" onclick={() => (inputs = inputs.filter((x) => x !== id))}>
                <X size={12} />
              </button>
            </span>
          {:else}
            <span class="text-ink-faint">nothing else</span>
          {/each}
          {#if extraChoices.length}
            <select
              class="field"
              aria-label="Add an input"
              value=""
              onchange={(e) => {
                const v = e.currentTarget.value;
                if (v) inputs = [...inputs, v];
                e.currentTarget.value = "";
              }}
            >
              <option value="">Add an input…</option>
              {#each extraChoices as o (o.id)}<option value={o.id}>{o.name}</option>{/each}
            </select>
          {/if}
        </div>
        {#if draft.type === "python" || draft.type === "r"}
          <label class="mt-3 flex items-center gap-2 text-sm text-ink-soft">
            Stop after
            <input
              class="field w-20"
              type="number"
              min="1"
              value={Math.round(draft.timeout_ms / 1000)}
              oninput={(e) => {
                if (draft.type === "python" || draft.type === "r") draft.timeout_ms = Math.max(1, Number(e.currentTarget.value) || 60) * 1000;
              }}
            />
            seconds
          </label>
        {/if}
        {#if draft.type === "python" || draft.type === "r"}
          <div class="mt-3 flex flex-wrap items-center gap-2 text-sm">
            <button class="btn !min-h-7" disabled={dirty} title={dirty ? "Save first: the file starts from the saved code" : undefined} onclick={() => ws.editExternally(step.id)}>
              <ExternalLink size={14} /> {draft.type === "r" ? "Open in RStudio" : "Open in Jupyter"}
            </button>
            <span class="text-xs text-ink-faint">
              {#if ws.external[step.id]}Open as <code class="code">{ws.external[step.id]?.split(/[\\/]/).pop()}</code>; saving it there updates this step.{:else}Opens {draft.type === "r" ? "an .R file" : "a notebook"} with the step's inputs loaded, in the app your computer uses for it. Saving there updates this step.{/if}
            </span>
          </div>

        {/if}
        </div>
      </div>
    {:else if draft.type === "chart"}
      <ChartEditor
        bind:mark={draft.mark}
        bind:x={draft.x}
        bind:y={draft.y}
        bind:title={draft.title}
        bind:color={draft.color}
        bind:summary={draft.summary}
        columns={inputColumns}
      />
    {/if}

    {#if dirty}
      <p class="mt-5 text-sm text-ink-soft">Unsaved changes. Saving marks this step and everything after it as outdated until the next run.</p>
    {/if}

    <Comments {ws} target="step:{step.id}" />
  </div>
</section>
