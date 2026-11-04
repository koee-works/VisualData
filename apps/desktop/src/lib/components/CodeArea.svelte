<script lang="ts">
  import { onMount } from "svelte";
  import { Compartment, EditorState } from "@codemirror/state";
  import { EditorView } from "@codemirror/view";
  import { setDiagnostics } from "@codemirror/lint";
  import type { Problem } from "../api/types";
  import type { EditorContext, Lang } from "../editor/complete";
  import { diagnostics, extensions, keysExtension, language, loadKeys, type Keys, type RunRequest } from "../editor/setup";

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
    keys = loadKeys(),
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
    keys?: Keys;
  } = $props();

  let host: HTMLDivElement;
  let view: EditorView | undefined;
  const keysC = new Compartment();
  const langC = new Compartment();

  onMount(() => {
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
          onchange: (code) => {
            if (code !== value) value = code;
          },
        }),
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
  });

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
    if (builtFor) view.dispatch({ effects: langC.reconfigure(language(lang, () => context)) });
    builtFor = k;
  });

  $effect(() => {
    const k = keys;
    view?.dispatch({ effects: keysC.reconfigure(keysExtension(k)) });
  });

  $effect(() => {
    const p = problems;
    if (view) view.dispatch(setDiagnostics(view.state, diagnostics(view.state.doc, p)));
  });

  let invalid = $derived(problems.some((p) => p.severity === "error"));
</script>

<div
  bind:this={host}
  class="code-area"
  class:single={singleLine}
  data-invalid={invalid}
  style:--min-lines={minLines}
></div>

<style>
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
