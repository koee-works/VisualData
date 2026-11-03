<script lang="ts">
  let {
    code = $bindable(),
    language,
    inputs,
    problems = [],
  }: {
    code: string;
    language: "sql" | "python";
    inputs: { id: string; name: string }[];
    problems?: import("../api/types").Problem[];
  } = $props();

  let first = $derived(inputs[0]!);
  let more = $derived(inputs.slice(1));

  // Tab inserts spaces instead of leaving the editor; Escape still leaves.
  function onkeydown(e: KeyboardEvent) {
    if (e.key !== "Tab" || e.shiftKey) return;
    e.preventDefault();
    const el = e.currentTarget as HTMLTextAreaElement;
    const { selectionStart: a, selectionEnd: b } = el;
    code = code.slice(0, a) + "    " + code.slice(b);
    queueMicrotask(() => el.setSelectionRange(a + 4, a + 4));
  }
</script>

<div class="space-y-2">
  <p class="text-sm text-ink-soft">
    {#if language === "sql"}
      Reads {first.name} as the table <code class="code">input</code>{#if more.length}, and
        {#each more as m, i (m.id)}{#if i > 0}, {/if}{m.name} as <code class="code">{m.id}</code>{/each}{/if}. The query's
      result is this step's result.
    {:else}
      Receives {first.name} as the pandas DataFrame <code class="code">df</code>{#if more.length}. Every input is in the
        dict <code class="code">dfs</code>:
        {#each inputs as m, i (m.id)}{#if i > 0}, {/if}<code class="code">dfs["{m.id}"]</code>{/each}{/if}. Assign a
      DataFrame to <code class="code">result</code>.
    {/if}
    Custom code is not converted back into visual controls.
  </p>
  <textarea
    class="field code h-64 w-full resize-y whitespace-pre"
    spellcheck="false"
    aria-label={language === "sql" ? "SQL query" : "Python code"}
    bind:value={code}
    {onkeydown}
    aria-invalid={problems.some((p) => p.severity === "error")}
  ></textarea>
  {#each problems as p, i (i)}
    <p class="text-sm {p.severity === 'error' ? 'text-failed' : 'text-stale'}" role={p.severity === "error" ? "alert" : undefined}>
      {#if p.line}Line {p.line}{#if p.column}, character {p.column}{/if}: {/if}{p.message}
    </p>
  {/each}
</div>
