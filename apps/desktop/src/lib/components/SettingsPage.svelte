<script lang="ts">
  import { Copy, Download, Pencil, Plus, Search, Trash2, Upload } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import { THEMES, type CustomTheme } from "../theme";
  import { isCustomTheme, prefs, TEXT_SIZES, themeFor, updatePrefs, type Preferences, type TextSize } from "../prefs.svelte";
  import { ALL_PANES, savePanes } from "../layout";
  import ThemeEditor from "./ThemeEditor.svelte";

  // Settings, in sections, starting with General. Everything here is kept in
  // the user's settings file (beside approvals), not in a project.
  let { ws }: { ws: Workspace } = $props();

  const sections = [
    { id: "general", name: "General" },
    { id: "appearance", name: "Appearance" },
    { id: "editor", name: "Code editor" },
    { id: "environments", name: "Python and R" },
  ] as const;
  type SectionId = (typeof sections)[number]["id"];

  let query = $state("");
  let section = $state<SectionId>("general");
  $effect(() => {
    if (ws.settingsSection) {
      section = ws.settingsSection;
      ws.settingsSection = null;
    }
  });
  let error = $state<string | null>(null);
  let saved = $state(false);

  // Every setting, with the words it is found by.
  const rows: { section: SectionId; id: string; words: string }[] = [
    { section: "general", id: "author", words: "name comments author sign" },
    { section: "general", id: "runAfterSave", words: "run after save automatically" },
    { section: "general", id: "messages", words: "messages notices hide automatically" },
    { section: "general", id: "numbers", words: "numbers decimals thousands separators digits" },
    { section: "general", id: "layout", words: "layout panels docks reset sizes" },
    { section: "appearance", id: "theme", words: "theme colours colors dark light night sepia contrast paper" },
    { section: "appearance", id: "textSize", words: "text size font larger smaller zoom" },
    { section: "appearance", id: "yourThemes", words: "custom themes your own make import export" },
    { section: "editor", id: "keys", words: "keys vim keyboard shortcuts" },
    { section: "editor", id: "wrap", words: "wrap long lines" },
    { section: "environments", id: "python", words: "python interpreter environment packages" },
    { section: "environments", id: "r", words: "r rscript installation packages libraries" },
  ];
  let matching = $derived(
    query.trim() ? new Set(rows.filter((r) => query.toLowerCase().split(/\s+/).every((w) => r.words.includes(w) || r.id.toLowerCase().includes(w))).map((r) => r.id)) : null,
  );
  const shown = (id: string, sec: SectionId) => (matching ? matching.has(id) : section === sec);
  let anyShown = $derived(!matching || matching.size > 0);

  let author = $state(prefs.value.author ?? "");

  async function set(patch: Partial<Preferences>) {
    error = await updatePrefs(patch);
    saved = !error;
    if (saved) setTimeout(() => (saved = false), 1500);
  }

  // Number display changes redraw the tables.
  async function setNumbers(patch: Partial<Preferences>) {
    await set(patch);
    ws.revision++;
  }

  function resetLayout() {
    for (const k of ["vda.split.visual", "vda.split.code", "vda.split.chart", "vda.split.source", "vda.sidebar", "vda.dock.right", "vda.dock.bottom"]) {
      try {
        localStorage.removeItem(k);
      } catch {
        // Nothing kept.
      }
    }
    ws.panes = { ...ALL_PANES, activity: false };
    ws.consoleOpen = false;
    savePanes($state.snapshot(ws.panes));
    ws.notice = "The layout is back to how it started. Sizes apply as panels are next shown.";
  }

  // Custom themes.
  let editing = $state<CustomTheme | null>(null);
  let importText = $state("");
  let exported = $state<string | null>(null);

  function newTheme() {
    const base = THEMES.find((t) => t.id === prefs.value.theme) ?? THEMES.find((t) => t.id === themeFor(prefs.value.theme).id) ?? THEMES[0]!;
    editing = { id: `custom-${Date.now().toString(36)}`, name: `My ${base.name}`, base: base.id, colors: {} };
  }

  async function saveTheme(t: CustomTheme) {
    const others = prefs.value.customThemes.filter((c) => c.id !== t.id);
    await set({ customThemes: [...others, t], theme: t.id });
    editing = null;
  }

  async function removeTheme(t: CustomTheme) {
    await set({ customThemes: prefs.value.customThemes.filter((c) => c.id !== t.id), theme: prefs.value.theme === t.id ? t.base : prefs.value.theme });
  }

  function exportTheme(t: CustomTheme) {
    exported = JSON.stringify({ vda_theme: 1, ...t }, null, 2);
    void navigator.clipboard?.writeText(exported).catch(() => {});
  }

  async function importTheme(text: string) {
    error = null;
    try {
      const raw = JSON.parse(text) as Record<string, unknown>;
      const t = { ...raw, id: typeof raw.id === "string" && raw.id.startsWith("custom-") ? raw.id : `custom-${Date.now().toString(36)}` };
      delete (t as Record<string, unknown>).vda_theme;
      if (!isCustomTheme(t)) throw new Error("this is not a theme: it needs a name, a base theme (paper, night, sepia or contrast) and colours as #rrggbb");
      await saveTheme(t);
      importText = "";
    } catch (e) {
      error = `Could not import the theme: ${(e as Error).message}`;
    }
  }

  async function importFile(e: Event) {
    const file = (e.currentTarget as HTMLInputElement).files?.[0];
    if (file) await importTheme(await file.text());
    (e.currentTarget as HTMLInputElement).value = "";
  }
</script>

<div class="flex h-full min-h-0">
  <nav class="w-52 shrink-0 border-r border-rule bg-paper px-2 py-3" aria-label="Settings sections">
    <label class="relative mb-3 block">
      <Search size={14} class="pointer-events-none absolute top-1/2 left-2 -translate-y-1/2 text-ink-faint" aria-hidden="true" />
      <input class="field w-full !pl-7 text-sm" placeholder="Search settings" aria-label="Search settings" bind:value={query} />
    </label>
    {#each sections as s (s.id)}
      <button class="settings-nav" aria-current={!matching && section === s.id ? "page" : undefined} onclick={() => ((section = s.id), (query = ""))}>{s.name}</button>
    {/each}
    <p class="mt-4 px-2 text-xs text-ink-faint">
      Kept in your settings file, not in the project, so they apply to every analysis you open.
      {#if saved}<span class="block text-fresh" role="status">Saved.</span>{/if}
    </p>
  </nav>

  <div class="min-w-0 flex-1 overflow-y-auto px-8 py-6">
    <div class="max-w-3xl space-y-7">
      {#if error}<p class="rounded-md border border-failed/40 bg-failed-wash px-3 py-2 text-sm" role="alert">{error}</p>{/if}
      {#if !anyShown}<p class="text-ink-soft">No setting matches “{query}”.</p>{/if}

      {#if !matching}<h2 class="text-xl font-bold">{sections.find((s) => s.id === section)?.name}</h2>{/if}

      {#if shown("author", "general")}
        <div class="setting">
          <label for="set-author">Your name on comments</label>
          <p>Shown beside comments and replies you write. Others see it when you share the project.</p>
          <input id="set-author" class="field w-72" bind:value={author} onchange={() => author.trim() && set({ author: author.trim() })} />
        </div>
      {/if}
      {#if shown("runAfterSave", "general")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.runAfterSave} onchange={(e) => set({ runAfterSave: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Run the analysis after saving a step</span>
            <span class="block text-sm text-ink-soft">What the change affects reruns at once; nothing else does. Off: results show as outdated until you run.</span>
          </span>
        </label>
      {/if}
      {#if shown("messages", "general")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.autoHideMessages} onchange={(e) => set({ autoHideMessages: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Hide messages after a few seconds</span>
            <span class="block text-sm text-ink-soft">Messages such as “Exported 4 rows”. Errors always stay until you close them.</span>
          </span>
        </label>
      {/if}
      {#if shown("numbers", "general")}
        <div class="setting">
          <span class="setting-title">Numbers in tables</span>
          <p>How decimals are shown. The exact value is always in the cell's tooltip and in exports.</p>
          <div class="flex flex-wrap items-center gap-4">
            <label class="flex items-center gap-2 text-sm">
              <input type="checkbox" checked={prefs.value.thousandsSeparators} onchange={(e) => setNumbers({ thousandsSeparators: e.currentTarget.checked })} />
              Thousands separators (11,656.0)
            </label>
            <label class="flex items-center gap-2 text-sm">
              At most
              <select class="field" value={prefs.value.maxDecimals} onchange={(e) => setNumbers({ maxDecimals: Number(e.currentTarget.value) })}>
                {#each [1, 2, 3, 4, 6, 8] as n (n)}<option value={n}>{n}</option>{/each}
              </select>
              decimals
            </label>
          </div>
        </div>
      {/if}
      {#if shown("layout", "general")}
        <div class="setting">
          <span class="setting-title">Layout</span>
          <p>Docks and dividers remember where you leave them. Start over with every panel at its first size.</p>
          <button class="btn" onclick={resetLayout}>Reset the layout</button>
        </div>
      {/if}

      {#if shown("theme", "appearance")}
        <div class="setting">
          <span class="setting-title">Theme</span>
          <p>Every built-in theme keeps text readable (WCAG AA). Charts and code follow it; exported reports stay in Paper for printing.</p>
          <div class="grid grid-cols-[repeat(auto-fill,minmax(10.5rem,1fr))] gap-2" role="radiogroup" aria-label="Theme">
            {#each [{ id: "system", name: "Match the computer" }, ...THEMES.map((t) => ({ id: t.id as string, name: t.name })), ...prefs.value.customThemes.map((t) => ({ id: t.id, name: t.name }))] as c (c.id)}
              {@const t = themeFor(c.id)}
              <button class="theme-card" role="radio" aria-checked={prefs.value.theme === c.id} onclick={() => set({ theme: c.id })}>
                <span class="block h-14 rounded-t border-b" style:background={t.colors.paper} style:border-color={t.colors.rule}>
                  <span class="m-2 inline-block rounded px-2 py-1 text-xs font-semibold" style:background={t.colors.panel} style:color={t.colors.ink}>Aa
                    <span class="ml-1 inline-block h-2 w-2 rounded-full" style:background={t.colors.fresh}></span>
                    <span class="inline-block h-2 w-2 rounded-full" style:background={t.colors.stale}></span>
                    <span class="inline-block h-2 w-2 rounded-full" style:background={t.colors.running}></span>
                  </span>
                </span>
                <span class="block px-2 py-1.5 text-left text-sm font-semibold">{c.name}</span>
              </button>
            {/each}
          </div>
        </div>
      {/if}
      {#if shown("textSize", "appearance")}
        <div class="setting">
          <span class="setting-title">Text size</span>
          <p>Scales the whole interface, code and tables included.</p>
          <div class="flex gap-1" role="radiogroup" aria-label="Text size">
            {#each Object.entries(TEXT_SIZES) as [id, s] (id)}
              <button class="seg" role="radio" aria-checked={prefs.value.textSize === id} onclick={() => set({ textSize: id as TextSize })}>{s.name}</button>
            {/each}
          </div>
        </div>
      {/if}
      {#if shown("yourThemes", "appearance")}
        <div class="setting">
          <span class="setting-title">Your themes</span>
          <p>Start from any theme and change its colours. The editor warns where text would be hard to read.</p>
          {#if editing}
            <ThemeEditor theme={editing} onsave={saveTheme} oncancel={() => (editing = null)} />
          {:else}
            {#if prefs.value.customThemes.length}
              <ul class="divide-y divide-rule rounded-md border border-rule" aria-label="Your themes">
                {#each prefs.value.customThemes as t (t.id)}
                  <li class="flex items-center gap-2 px-3 py-2">
                    <span class="min-w-0 flex-1"><span class="font-semibold">{t.name}</span> <span class="text-xs text-ink-faint">from {THEMES.find((b) => b.id === t.base)?.name}</span></span>
                    <button class="btn btn-quiet !min-h-7 !px-2 text-sm" onclick={() => (editing = structuredClone($state.snapshot(t)))}><Pencil size={14} /> Edit</button>
                    <button class="btn btn-quiet !min-h-7 !px-2 text-sm" onclick={() => (editing = { ...structuredClone($state.snapshot(t)), id: `custom-${Date.now().toString(36)}`, name: `${t.name} copy` })}><Copy size={14} /> Duplicate</button>
                    <button class="btn btn-quiet !min-h-7 !px-2 text-sm" onclick={() => exportTheme(t)}><Download size={14} /> Export</button>
                    <button class="btn btn-quiet !min-h-7 !min-w-7 justify-center !px-1.5" aria-label="Delete {t.name}" onclick={() => removeTheme(t)}><Trash2 size={14} /></button>
                  </li>
                {/each}
              </ul>
            {/if}
            {#if exported}
              <div class="mt-2">
                <p class="text-sm text-ink-soft">Copied to the clipboard. Share it as a file or paste it into “Import”.</p>
                <textarea class="field code mt-1 h-32 w-full" readonly aria-label="Exported theme">{exported}</textarea>
              </div>
            {/if}
            <div class="mt-2 flex flex-wrap items-center gap-2">
              <button class="btn" onclick={newTheme}><Plus size={14} /> New theme</button>
              <label class="btn cursor-pointer"><Upload size={14} /> Import a file<input type="file" accept=".json,application/json" class="sr-only" onchange={importFile} /></label>
            </div>
            <details class="mt-2 text-sm">
              <summary class="cursor-pointer text-ink-soft">Or paste a theme</summary>
              <textarea class="field code mt-1 h-24 w-full" aria-label="Theme to import" placeholder="Paste a theme exported from here" bind:value={importText}></textarea>
              <button class="btn mt-1" disabled={!importText.trim()} onclick={() => importTheme(importText)}>Import</button>
            </details>
          {/if}
        </div>
      {/if}

      {#if shown("keys", "editor")}
        <div class="setting">
          <span class="setting-title">Keys</span>
          <p>Ctrl+Enter, Shift+Enter and the rest work with either. Vim keys load the first time you choose them.</p>
          <div class="flex gap-1" role="radiogroup" aria-label="Editor keys">
            {#each [["standard", "Standard"], ["vim", "Vim"]] as [id, name] (id)}
              <button class="seg" role="radio" aria-checked={prefs.value.editorKeys === id} onclick={() => set({ editorKeys: id as "standard" | "vim" })}>{name}</button>
            {/each}
          </div>
        </div>
      {/if}
      {#if shown("wrap", "editor")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.wrapCode} onchange={(e) => set({ wrapCode: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Wrap long lines</span>
            <span class="block text-sm text-ink-soft">Off: long lines scroll sideways, as in most code editors.</span>
          </span>
        </label>
      {/if}

      {#if shown("python", "environments")}
        <div class="setting">
          <span class="setting-title">Python</span>
          <p>Which Python runs Python steps and their console, and what it has installed.</p>
          <button class="btn" onclick={() => (ws.pythonOpen = true)}>Python environment…</button>
        </div>
      {/if}
      {#if shown("r", "environments")}
        <div class="setting">
          <span class="setting-title">R</span>
          <p>Which R runs R steps, the package libraries it sees, and its packages.</p>
          <button class="btn" onclick={() => (ws.rOpen = true)}>R installation…</button>
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .settings-nav {
    display: block;
    width: 100%;
    text-align: left;
    padding: 0.35rem 0.6rem;
    border-radius: 5px;
    font-size: var(--text-sm);
    color: var(--color-ink-soft);
  }
  .settings-nav:hover {
    background: var(--color-panel);
    color: var(--color-ink);
  }
  .settings-nav[aria-current="page"] {
    background: var(--color-select);
    color: var(--color-ink);
    font-weight: 600;
  }
  .setting {
    display: block;
  }
  .setting > label:first-child,
  .setting-title {
    display: block;
    font-weight: 600;
  }
  .setting > p {
    margin: 0.15rem 0 0.6rem;
    font-size: var(--text-sm);
    color: var(--color-ink-soft);
    max-width: 62ch;
  }
  .setting-check {
    display: flex;
    gap: 0.6rem;
    align-items: flex-start;
    cursor: pointer;
  }
  .setting-check input {
    margin-top: 0.3rem;
  }
  .seg {
    border: 1px solid var(--color-rule-strong);
    border-radius: 5px;
    padding: 0.3rem 0.8rem;
    font-size: var(--text-sm);
  }
  .seg[aria-checked="true"] {
    background: var(--color-ink);
    border-color: var(--color-ink);
    color: var(--color-panel);
    font-weight: 600;
  }
  .theme-card {
    border: 1px solid var(--color-rule-strong);
    border-radius: 6px;
    background: var(--color-panel);
    overflow: hidden;
  }
  .theme-card[aria-checked="true"] {
    outline: 2px solid var(--color-running);
    outline-offset: 1px;
  }
</style>
