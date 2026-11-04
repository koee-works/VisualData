<script lang="ts">
  import { editorParameters } from "../editor/complete";
  import CodeArea from "./CodeArea.svelte";
  import type { Column, Parameter, Problem } from "../api/types";
  import type { RunRequest } from "../editor/setup";

  let {
    code = $bindable(),
    language,
    inputs,
    parameters = [],
    problems = [],
    onrun,
  }: {
    code: string;
    language: "sql" | "python" | "r";
    inputs: { id: string; name: string; columns?: Column[] | null }[];
    parameters?: Parameter[];
    problems?: Problem[];
    onrun?: (r: RunRequest) => void;
  } = $props();

  let first = $derived(inputs[0]!);
  let more = $derived(inputs.slice(1));
  let context = $derived({
    inputs: inputs.map((i) => ({ id: i.id, name: i.name, columns: i.columns ?? null })),
    parameters: editorParameters(parameters),
  });
  const LABEL = { sql: "SQL query", python: "Python code", r: "R code" };
  let mac = typeof navigator !== "undefined" && /Mac/.test(navigator.platform);
</script>

<div class="space-y-2">
  <p class="text-sm text-ink-soft">
    {#if language === "sql"}
      Reads {first.name} as the table <code class="code">input</code>{#if more.length}, and
        {#each more as m, i (m.id)}{#if i > 0}, {/if}{m.name} as <code class="code">{m.id}</code>{/each}{/if}. Parameters are
      <code class="code">$name</code>. The query's result is this step's result.
    {:else if language === "python"}
      Receives {first.name} as the pandas DataFrame <code class="code">df</code>{#if more.length}. Every input is in the
        dict <code class="code">dfs</code>:
        {#each inputs as m, i (m.id)}{#if i > 0}, {/if}<code class="code">dfs["{m.id}"]</code>{/each}{/if}. Parameters are
      in <code class="code">params</code>. Assign a DataFrame to <code class="code">result</code>.
    {:else}
      Receives {first.name} as the data frame <code class="code">df</code>{#if more.length}. Every input is in the
        list <code class="code">dfs</code>:
        {#each inputs as m, i (m.id)}{#if i > 0}, {/if}<code class="code">dfs$`{m.id}`</code>{/each}{/if}. Parameters are
      in <code class="code">params</code>. Assign a data frame to <code class="code">result</code>.
    {/if}
    Custom code is not converted back into visual controls.
  </p>
  <CodeArea bind:value={code} lang={language} label={LABEL[language]} {context} {problems} {onrun} minLines={12} />
  <p class="text-xs text-ink-faint">
    {mac ? "⌘" : "Ctrl"}+Enter or Shift+Enter saves and runs. Ctrl+Space suggests names{#if language === "r"};
      Alt+− types <code class="code">&lt;-</code>, {mac ? "⌘" : "Ctrl"}+Shift+M types <code class="code">|&gt;</code>{/if}.
  </p>
  {#each problems.filter((p) => p.line === null && p.column === null) as p, i (i)}
    <p class="text-sm {p.severity === 'error' ? 'text-failed' : 'text-stale'}" role={p.severity === "error" ? "alert" : undefined}>{p.message}</p>
  {/each}
  {#each problems.filter((p) => p.line !== null || p.column !== null) as p, i (i)}
    <p class="text-sm {p.severity === 'error' ? 'text-failed' : 'text-stale'}" role={p.severity === "error" ? "alert" : undefined}>
      {#if p.line}Line {p.line}{#if p.column}, character {p.column}{/if}: {/if}{p.message}
    </p>
  {/each}
</div>
