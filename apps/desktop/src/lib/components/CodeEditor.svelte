<script lang="ts">
  let {
    code = $bindable(),
    language,
    inputName,
  }: { code: string; language: "sql" | "python"; inputName: string } = $props();

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
      Reads <code class="code">{inputName}</code> as the table <code class="code">input</code>. The query's result is
      this step's result.
    {:else}
      Receives <code class="code">{inputName}</code> as the pandas DataFrame <code class="code">df</code>. Assign a
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
  ></textarea>
</div>
