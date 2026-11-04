<script lang="ts">
  import { untrack } from "svelte";
  import { TriangleAlert, CircleCheck, RotateCcw } from "@lucide/svelte";
  import { apply, buildCustom, contrastProblems, THEMES, themeById, TOKEN_GROUPS, type CustomTheme, type ThemeId } from "../theme";
  import { applyPrefs } from "../prefs.svelte";

  // Edit a theme's colours. The whole app previews the theme while editing,
  // and goes back to the chosen theme on cancel.
  let { theme, onsave, oncancel }: { theme: CustomTheme; onsave: (t: CustomTheme) => void; oncancel: () => void } = $props();

  let draft = $state<CustomTheme>(untrack(() => structuredClone($state.snapshot(theme))));
  let full = $derived(buildCustom(draft));
  let problems = $derived(contrastProblems(full.colors));

  $effect(() => {
    apply(full);
  });
  $effect(() => () => applyPrefs());

  function setColor(token: string, value: string) {
    if (!/^#[0-9a-fA-F]{6}$/.test(value)) return;
    const base = themeById(draft.base).colors[token];
    const colors = { ...draft.colors };
    if (value.toLowerCase() === base?.toLowerCase()) delete colors[token];
    else colors[token] = value.toLowerCase();
    draft.colors = colors;
  }

  function changeBase(id: ThemeId) {
    draft.base = id;
  }

  const names = Object.fromEntries(TOKEN_GROUPS.flatMap((g) => g.tokens));
</script>

<div class="rounded-md border border-rule-strong" aria-label="Theme editor" role="group">
  <div class="flex flex-wrap items-end gap-3 border-b border-rule px-4 py-3">
    <label class="block">
      <span class="block text-sm font-semibold">Name</span>
      <input class="field w-56" bind:value={draft.name} aria-label="Theme name" />
    </label>
    <label class="block">
      <span class="block text-sm font-semibold">Starts from</span>
      <select class="field" value={draft.base} onchange={(e) => changeBase(e.currentTarget.value as ThemeId)} aria-label="Starts from">
        {#each THEMES as t (t.id)}<option value={t.id}>{t.name}</option>{/each}
      </select>
    </label>
    <p class="pb-1 text-xs text-ink-faint">The whole app shows the theme while you edit it.</p>
  </div>

  <div class="grid gap-x-8 gap-y-4 px-4 py-3 sm:grid-cols-2">
    {#each TOKEN_GROUPS as g (g.group)}
      <fieldset class="min-w-0" class:sm:col-span-2={g.group === "States"}>
        <legend class="mb-1 text-sm font-semibold">{g.group}</legend>
        <div class="grid gap-1 {g.group === 'States' ? 'sm:grid-cols-2 sm:gap-x-8' : ''}">
          {#each g.tokens as [token, label] (token)}
            {@const value = full.colors[token]!}
            {@const changed = token in draft.colors}
            <div class="flex items-center gap-2">
              <input type="color" class="h-7 w-9 shrink-0 cursor-pointer rounded border border-rule-strong bg-panel p-0.5" {value} aria-label="{label} colour" oninput={(e) => setColor(token, e.currentTarget.value)} />
              <input class="field code !min-h-7 w-24 !px-1.5 text-xs" {value} aria-label="{label}, hex" onchange={(e) => setColor(token, e.currentTarget.value.trim())} spellcheck="false" />
              <span class="min-w-0 flex-1 truncate text-sm" class:font-semibold={changed}>{label}</span>
              {#if changed}
                <button class="btn btn-quiet !min-h-6 !min-w-6 justify-center !px-1" title="Back to {themeById(draft.base).name}'s colour" aria-label="Reset {label}" onclick={() => setColor(token, themeById(draft.base).colors[token]!)}><RotateCcw size={13} /></button>
              {/if}
            </div>
          {/each}
        </div>
      </fieldset>
    {/each}
  </div>

  <div class="border-t border-rule px-4 py-3 text-sm" aria-live="polite">
    {#if problems.length}
      <p class="flex items-center gap-1.5 font-semibold text-stale"><TriangleAlert size={15} /> Hard to read in {problems.length} place{problems.length === 1 ? "" : "s"}</p>
      <ul class="mt-1 space-y-0.5" aria-label="Contrast problems">
        {#each problems as p (p.what)}
          <li>{p.what}: {p.ratio.toFixed(1)}:1, needs {p.min}:1 <span class="text-ink-faint">({names[p.tokens[0]]} on {names[p.tokens[1]]})</span></li>
        {/each}
      </ul>
      <p class="mt-1 text-xs text-ink-faint">You can still save it; this is a warning, not a rule.</p>
    {:else}
      <p class="flex items-center gap-1.5 text-fresh"><CircleCheck size={15} /> All text is readable (WCAG AA).</p>
    {/if}
  </div>

  <div class="flex justify-end gap-2 border-t border-rule px-4 py-3">
    <button class="btn btn-quiet" onclick={oncancel}>Cancel</button>
    <button class="btn btn-primary" disabled={!draft.name.trim()} onclick={() => onsave({ ...$state.snapshot(draft), name: draft.name.trim() })}>Save and use</button>
  </div>
</div>
