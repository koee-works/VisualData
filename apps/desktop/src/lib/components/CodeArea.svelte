<script lang="ts">
  import { onMount } from "svelte";
  import type { Problem } from "../api/types";
  import type { EditorContext, Lang } from "../editor/complete";
  import type { RunRequest } from "../editor/prefs";
  import { prefs } from "../prefs.svelte";
  // CodeMirror loads with the first editor shown, not with the app.
  type CM = typeof import("../editor/setup");
  let cm: CM | undefined = $state();

  let {
    value = $bindable(),
    lang,
    label,
    context = { inputs: [], parameters: [] },
    problems = [],
    placeholder,
    singleLine = false,
    minLines = 10,
    onrun,
    onenter,
    onhistory,
  }: {
    value: string;
    lang: Lang;
    label: string;
    context?: EditorContext;
    problems?: Problem[];
    placeholder?: string;
    singleLine?: boolean;
    minLines?: number;
    onrun?: (r: RunRequest) => void;
    onenter?: () => void;
    onhistory?: (step: -1 | 1) => string | null;
  } = $props();
  // From the user's preferences, so a change applies to every open editor.
  let keys = $derived(prefs.value.editorKeys);
  let wrap = $derived(prefs.value.wrapCode && !singleLine);

  let host: HTMLDivElement;
  let view: import("@codemirror/view").EditorView | undefined = $state();
  let keysC: import("@codemirror/state").Compartment;
  let langC: import("@codemirror/state").Compartment;
  let wrapC: import("@codemirror/state").Compartment;

  onMount(() => {
    let gone = false;
    let cleanup = () => {};
    import("../editor/setup").then((m) => {
      if (gone) return;
      cm = m;
      cleanup = mountEditor(m);
    });
    return () => {
      gone = true;
      cleanup();
    };
  });

  function mountEditor(m: CM) {
    const { Compartment, EditorState, EditorView, extensions } = m;
    keysC = new Compartment();
    langC = new Compartment();
    wrapC = new Compartment();
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: value,
        extensions: extensions({
          lang,
          label,
          placeholder,
          singleLine,
          keys,
          keysCompartment: keysC,
          languageCompartment: langC,
          context: () => context,
          onrun: onrun ? (r) => onrun?.(r) : undefined,
          onenter: onenter ? () => onenter?.() : undefined,
          onhistory: onhistory ? (s) => onhistory?.(s) ?? null : undefined,
          onchange: (code) => {
            if (code !== value) value = code;
          },
        }).concat(wrapC.of(wrap ? EditorView.lineWrapping : [])),
      }),
    });
    // Replace the whole text from outside the editor (tests, tools), as if typed.
    const set = (e: Event) => {
      const text = (e as CustomEvent<string>).detail;
      view?.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text }, userEvent: "input" });
    };
    host.addEventListener("vda-set-code", set);
    return () => {
      host.removeEventListener("vda-set-code", set);
      view?.destroy();
    };
  }

  // Text set from outside (another step selected, undo) replaces the document.
  $effect(() => {
    const v = value;
    if (view && v !== view.state.doc.toString()) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: v } });
    }
  });

  // SQL's table and column completion is built from the inputs.
  let contextKey = $derived(JSON.stringify(context));
  let builtFor = "";
  $effect(() => {
    const k = contextKey;
    if (!view || k === builtFor) return;
    if (builtFor && cm) view.dispatch({ effects: langC.reconfigure(cm.language(lang, () => context)) });
    builtFor = k;
  });

  $effect(() => {
    const k = keys;
    const v = view;
    if (!v || !cm) return;
    void cm.keysExtension(k).then((ext) => v.dispatch({ effects: keysC.reconfigure(ext) }));
  });

  $effect(() => {
    const w = wrap;
    if (view && cm) view.dispatch({ effects: wrapC.reconfigure(w ? cm.EditorView.lineWrapping : []) });
  });

  $effect(() => {
    const p = problems;
    if (view && cm) view.dispatch(cm.setDiagnostics(view.state, cm.diagnostics(view.state.doc, p)));
  });

  let invalid = $derived(problems.some((p) => p.severity === "error"));

  /** Put the cursor at the start of a line and show it. */
  export function focusLine(n: number) {
    if (!view) return;
    const line = view.state.doc.line(Math.min(Math.max(n, 1), view.state.doc.lines));
    view.dispatch({ selection: { anchor: line.from, head: line.to }, scrollIntoView: true });
    view.focus();
  }

  export function focus() {
    view?.focus();
  }
</script>

<div
  bind:this={host}
  class="code-area"
  class:single={singleLine}
  data-invalid={invalid}
  style:--min-lines={minLines}
>
  {#if !view}
    <!-- Until the editor loads (a moment, once): the code as it will look. -->
    <pre class="code-placeholder code" aria-hidden="true">{value || placeholder || ""}</pre>
  {/if}
</div>

<style>
  .code-placeholder {
    margin: 0;
    border: 1px solid var(--color-rule-strong);
    border-radius: 5px;
    background: var(--color-panel);
    padding: 6px 8px 6px 3.1rem;
    white-space: pre-wrap;
    min-height: calc(var(--min-lines) * 1.55em + 14px);
  }
  .code-area.single .code-placeholder {
    min-height: 0;
    padding: 4px 6px;
  }
  .code-area :global(.cm-editor) {
    max-height: 32rem;
  }
  .code-area:not(.single) :global(.cm-scroller) {
    min-height: calc(var(--min-lines) * 1.55em + 12px);
  }
  .code-area[data-invalid="true"] :global(.cm-editor) {
    border-color: var(--color-failed);
  }
  .code-area.single :global(.cm-content) {
    padding: 4px 2px;
  }
</style>
