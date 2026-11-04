<script lang="ts">
  import { DropdownMenu } from "bits-ui";
  import { Check, Palette } from "@lucide/svelte";
  import { THEMES, resolve, type ThemeChoice } from "../theme";
  import { setTheme, themeState } from "../themeState.svelte";

  // Themes, each shown with its own paper, ink and accent so they can be
  // compared before choosing.
  const choices: { id: ThemeChoice; name: string; hint: string }[] = [
    { id: "system", name: "Match the computer", hint: "Paper by day, Night when your computer is in dark mode" },
    ...THEMES.map((t) => ({ id: t.id, name: t.name, hint: t.hint })),
  ];
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger class="pane-toggle" aria-label="Theme" title="Theme: {choices.find((c) => c.id === themeState.choice)?.name}">
    <Palette size={15} />
  </DropdownMenu.Trigger>
  <DropdownMenu.Portal>
    <DropdownMenu.Content class="z-50 w-80 rounded-md border border-rule-strong bg-panel p-1 shadow-[var(--shadow-menu)]" side="top" align="end">
      <DropdownMenu.RadioGroup value={themeState.choice} onValueChange={(v) => setTheme(v as ThemeChoice)}>
        {#each choices as c (c.id)}
          {@const t = resolve(c.id)}
          <DropdownMenu.RadioItem value={c.id} class="flex cursor-pointer items-start gap-2.5 rounded px-2 py-1.5 data-highlighted:bg-select">
            <span class="mt-0.5 flex h-7 w-10 shrink-0 overflow-hidden rounded border" style:border-color={t.colors["rule-strong"]} aria-hidden="true">
              <span class="flex-1" style:background={t.colors.paper}></span>
              <span class="flex flex-1 items-center justify-center" style:background={t.colors.panel}>
                <span class="h-3 w-1.5 rounded-sm" style:background={t.colors.ink}></span>
              </span>
              <span class="w-1.5" style:background={t.colors.running}></span>
            </span>
            <span class="min-w-0 flex-1">
              <span class="block text-sm font-semibold">{c.name}</span>
              <span class="block text-xs text-ink-soft">{c.hint}</span>
            </span>
            {#if themeState.choice === c.id}<Check size={15} class="mt-1 text-ink" aria-hidden="true" />{/if}
          </DropdownMenu.RadioItem>
        {/each}
      </DropdownMenu.RadioGroup>
    </DropdownMenu.Content>
  </DropdownMenu.Portal>
</DropdownMenu.Root>
