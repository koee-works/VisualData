<script lang="ts">
  import { DropdownMenu } from "bits-ui";
  import { Check, Palette, Settings2 } from "@lucide/svelte";
  import { THEMES } from "../theme";
  import { prefs, themeFor, updatePrefs } from "../prefs.svelte";

  // Themes, each shown with its own paper, ink and accent so they can be
  // compared before choosing; the user's own themes after the built-in ones.
  let { onedit }: { onedit?: () => void } = $props();

  let choices = $derived([
    { id: "system", name: "Match the computer", hint: "Paper by day, Night when your computer is in dark mode" },
    ...THEMES.map((t) => ({ id: t.id as string, name: t.name, hint: t.hint })),
    ...prefs.value.customThemes.map((t) => ({ id: t.id, name: t.name, hint: "Your theme" })),
  ]);
</script>

{#snippet swatch(id: string)}
  {@const t = themeFor(id)}
  <span class="mt-0.5 flex h-7 w-10 shrink-0 overflow-hidden rounded border" style:border-color={t.colors["rule-strong"]} aria-hidden="true">
    <span class="flex-1" style:background={t.colors.paper}></span>
    <span class="flex flex-1 items-center justify-center" style:background={t.colors.panel}>
      <span class="h-3 w-1.5 rounded-sm" style:background={t.colors.ink}></span>
    </span>
    <span class="w-1.5" style:background={t.colors.running}></span>
  </span>
{/snippet}

<DropdownMenu.Root>
  <DropdownMenu.Trigger class="pane-toggle" aria-label="Theme" title="Theme: {choices.find((c) => c.id === prefs.value.theme)?.name ?? 'Match the computer'}">
    <Palette size={15} />
  </DropdownMenu.Trigger>
  <DropdownMenu.Portal>
    <DropdownMenu.Content class="z-50 max-h-[70vh] w-80 overflow-y-auto rounded-md border border-rule-strong bg-panel p-1 shadow-[var(--shadow-menu)]" side="top" align="end">
      <DropdownMenu.RadioGroup value={prefs.value.theme} onValueChange={(v) => updatePrefs({ theme: v })}>
        {#each choices as c (c.id)}
          <DropdownMenu.RadioItem value={c.id} class="flex cursor-pointer items-start gap-2.5 rounded px-2 py-1.5 data-highlighted:bg-select">
            {@render swatch(c.id)}
            <span class="min-w-0 flex-1">
              <span class="block text-sm font-semibold">{c.name}</span>
              <span class="block text-xs text-ink-soft">{c.hint}</span>
            </span>
            {#if prefs.value.theme === c.id}<Check size={15} class="mt-1 text-ink" aria-hidden="true" />{/if}
          </DropdownMenu.RadioItem>
        {/each}
      </DropdownMenu.RadioGroup>
      {#if onedit}
        <DropdownMenu.Separator class="my-1 border-t border-rule" />
        <DropdownMenu.Item class="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-sm data-highlighted:bg-select" onSelect={onedit}>
          <Settings2 size={15} aria-hidden="true" /> Make your own theme…
        </DropdownMenu.Item>
      {/if}
    </DropdownMenu.Content>
  </DropdownMenu.Portal>
</DropdownMenu.Root>
