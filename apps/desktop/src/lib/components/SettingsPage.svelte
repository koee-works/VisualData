<script lang="ts">
  import { Copy, Download, Pencil, Plus, Search, Trash2, Upload, X } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import { THEMES, type CustomTheme } from "../theme";
  import {
    DATE_FORMATS,
    DEFAULTS,
    EXPORT_FORMATS,
    isCustomTheme,
    MEMORY_LIMITS_MB,
    MISSING_TEXTS,
    normalize,
    prefs,
    ROW_DENSITIES,
    STEP_TIMEOUTS,
    TAB_WIDTHS,
    TEXT_SIZES,
    themeFor,
    updatePrefs,
    VERSION_COUNTS,
    type Preferences,
    type TextSize,
  } from "../prefs.svelte";
  import { formatDateText } from "../format";
  import { shortcutGroups } from "../shortcuts";
  import type { AboutView } from "../api/types";
  import { ALL_PANES, savePanes } from "../layout";
  import ThemeEditor from "./ThemeEditor.svelte";

  // Settings, in sections, starting with General. Everything here is kept in
  // the user's settings file (beside approvals), not in a project.
  let { ws }: { ws: Workspace } = $props();

  const sections = [
    { id: "general", name: "General" },
    { id: "data", name: "Data" },
    { id: "appearance", name: "Appearance" },
    { id: "editor", name: "Code editor" },
    { id: "environments", name: "Running" },
    { id: "safety", name: "Safety" },
    { id: "projects", name: "Projects" },
    { id: "keyboard", name: "Keyboard" },
    { id: "about", name: "About" },
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
    { section: "general", id: "messages", words: "messages notices hide automatically" },
    { section: "general", id: "layout", words: "layout panels docks reset sizes" },
    { section: "data", id: "numbers", words: "numbers decimals thousands separators digits" },
    { section: "data", id: "density", words: "row height density compact roomy spacing table" },
    { section: "data", id: "missingText", words: "missing values blank dash null empty cells show" },
    { section: "data", id: "dates", words: "date format dates time day month year" },
    { section: "data", id: "markers", words: "missing markers clean n/a na values default" },
    { section: "appearance", id: "theme", words: "theme colours colors dark light night sepia contrast paper" },
    { section: "appearance", id: "textSize", words: "text size font larger smaller zoom" },
    { section: "appearance", id: "yourThemes", words: "custom themes your own make import export" },
    { section: "appearance", id: "motion", words: "reduce motion animation accessibility transitions" },
    { section: "editor", id: "keys", words: "keys vim keyboard shortcuts" },
    { section: "editor", id: "wrap", words: "wrap long lines" },
    { section: "editor", id: "fontSize", words: "code font size text editor" },
    { section: "editor", id: "tabWidth", words: "tab width indent spaces indentation" },
    { section: "editor", id: "lineNumbers", words: "line numbers gutter" },
    { section: "editor", id: "brackets", words: "brackets quotes auto close pairs" },
    { section: "editor", id: "activeLine", words: "highlight current line" },
    { section: "environments", id: "runAfterSave", words: "run after save automatically" },
    { section: "environments", id: "runOnOpen", words: "run when open start project automatically" },
    { section: "environments", id: "timeout", words: "timeout time limit seconds python r new step default" },
    { section: "environments", id: "memory", words: "memory limit ram gigabytes python r console" },
    { section: "environments", id: "python", words: "python interpreter environment packages" },
    { section: "environments", id: "r", words: "r rscript installation packages libraries" },
    { section: "safety", id: "review", words: "review trust approve run code python r ask" },
    { section: "safety", id: "forget", words: "forget approved approvals code revoke" },
    { section: "projects", id: "versions", words: "versions history keep saved how many" },
    { section: "projects", id: "exportFormat", words: "export format csv excel parquet default save" },
    { section: "projects", id: "reopen", words: "reopen last project start open again" },
    { section: "projects", id: "folders", words: "folders default location projects exports directory" },
    { section: "keyboard", id: "shortcuts", words: "keyboard shortcuts keys list hotkeys commands" },
    { section: "about", id: "version", words: "about version build" },
    { section: "about", id: "paths", words: "settings file location path approvals where kept" },
    { section: "about", id: "diagnostics", words: "diagnostics copy bug report support system info" },
    { section: "about", id: "backup", words: "export import reset restore defaults back up settings" },
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


  // ---- Data
  const DENSITY_NAMES = { compact: "Compact", normal: "Normal", roomy: "Roomy" } as const;
  const MISSING_NAMES = { missing: "missing", dash: "—", blank: "Blank", null: "null" } as const;
  const DATE_EXAMPLE = "2026-07-02";
  const EXPORT_NAMES = { csv: "CSV", xlsx: "Excel", parquet: "Parquet" } as const;

  let newMarker = $state("");
  async function addMarker() {
    const m = newMarker.trim();
    if (!m || prefs.value.missingMarkers.some((x) => x.toLowerCase() === m.toLowerCase())) return void (newMarker = "");
    await set({ missingMarkers: [...prefs.value.missingMarkers, m] });
    newMarker = "";
  }

  // ---- Running
  const seconds = (n: number) => (n < 60 ? `${n} seconds` : n === 60 ? "1 minute" : `${n / 60} minutes`);
  /** The listed choices, plus the current value when a settings file holds another. */
  const withCurrent = (list: number[], now: number) => (list.includes(now) ? list : [...list, now].sort((a, b) => a - b));
  const gigabytes = (mb: number) => (mb >= 1024 && mb % 1024 === 0 ? `${mb / 1024} GB` : `${mb} MB`);

  // ---- Safety, About
  let about = $state<AboutView | null>(null);
  async function loadAbout() {
    try {
      about = await ws.backend.about();
    } catch {
      about = null;
    }
  }
  void loadAbout();

  let confirming = $state<"trust" | "forget" | "reset" | null>(null);
  async function forgetApprovals() {
    if (confirming !== "forget") return void (confirming = "forget");
    confirming = null;
    const n = about?.approvals ?? 0;
    try {
      await ws.forgetApprovals();
    } catch (e) {
      error = (e as { message?: string }).message ?? String(e);
      return;
    }
    await loadAbout();
    ws.notice = `Forgot ${n} approved piece${n === 1 ? "" : "s"} of code. Python and R steps ask for review again.`;
  }
  async function trustAll() {
    if (confirming !== "trust") return void (confirming = "trust");
    confirming = null;
    await ws.setTrustAll(true);
  }

  // ---- Projects
  async function chooseFolder(which: "projectsFolder" | "exportsFolder") {
    const dir = await ws.backend.pickFolder(prefs.value[which] ?? null);
    if (dir) await set(which === "projectsFolder" ? { projectsFolder: dir } : { exportsFolder: dir });
  }

  // ---- About
  async function copy(text: string, what: string) {
    try {
      await navigator.clipboard.writeText(text);
      ws.notice = `Copied ${what}.`;
    } catch {
      error = `Could not copy ${what}: select it and copy by hand.`;
    }
  }

  async function copyDiagnostics() {
    const [py, r] = await Promise.all([ws.backend.pythonEnvironment().catch(() => null), ws.backend.rEnvironment().catch(() => null)]);
    const env = (name: string, v: { version: string | null; executable?: string | null; interpreter?: string; rscript?: string; chosen_by: string; problem: string | null } | null) =>
      `${name}: ${v ? `${v.version ?? "not found"} at ${v.executable ?? v.interpreter ?? v.rscript} (${v.chosen_by})${v.problem ? `; problem: ${v.problem}` : ""}` : "unknown"}`;
    await copy(
      [
        `Visual data analysis ${about?.version ?? "unknown"}`,
        `System: ${navigator.platform || "unknown"}, ${navigator.userAgent}`,
        env("Python", py),
        env("R", r),
        `Settings file: ${about?.settings_path ?? "none"}`,
        `Approved code: ${about?.approvals ?? 0} pieces`,
        `Theme: ${prefs.value.theme}; text size: ${prefs.value.textSize}`,
      ].join("\n"),
      "diagnostics",
    );
  }

  let settingsText = $state("");
  let settingsOut = $state<string | null>(null);
  function exportSettings() {
    const out: Record<string, unknown> = { ...$state.snapshot(prefs.value) };
    delete out.author;
    delete out.lastProject;
    settingsOut = JSON.stringify({ vda_settings: 1, ...out }, null, 2);
    void navigator.clipboard?.writeText(settingsOut).catch(() => {});
  }
  async function importSettings(text: string) {
    error = null;
    try {
      const raw = JSON.parse(text) as unknown;
      if (!raw || typeof raw !== "object" || Array.isArray(raw)) throw new Error("this is not a settings file");
      const given = raw as Record<string, unknown>;
      delete given.vda_settings;
      delete given.author;
      delete given.lastProject;
      // Only what the file holds, and only values that fit: the rest stays as it is.
      const fit = normalize(given) as unknown as Record<string, unknown>;
      const patch = Object.fromEntries(Object.entries(fit).filter(([k, v]) => k in given && v !== undefined));
      if (!Object.keys(patch).length) throw new Error("it holds no setting this version knows");
      await setNumbers(patch as Partial<Preferences>);
      settingsText = "";
    } catch (e) {
      error = `Could not import settings: ${(e as Error).message}`;
    }
  }
  async function importSettingsFile(e: Event) {
    const file = (e.currentTarget as HTMLInputElement).files?.[0];
    if (file) await importSettings(await file.text());
    (e.currentTarget as HTMLInputElement).value = "";
  }
  async function resetSettings() {
    if (confirming !== "reset") return void (confirming = "reset");
    confirming = null;
    await setNumbers({
      ...DEFAULTS,
      missingMarkers: [...DEFAULTS.missingMarkers],
      customThemes: [],
      author: prefs.value.author,
      lastProject: prefs.value.lastProject,
      codeMemoryMb: undefined,
      projectsFolder: undefined,
      exportsFolder: undefined,
    });
    ws.notice = "Every setting is back to how it started. Your name on comments is unchanged.";
  }

  const mac = typeof navigator !== "undefined" && /Mac/.test(navigator.platform);
  const keyGroups = shortcutGroups(mac);

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

      {#if shown("messages", "general")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.autoHideMessages} onchange={(e) => set({ autoHideMessages: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Hide messages after a few seconds</span>
            <span class="block text-sm text-ink-soft">Messages such as “Exported 4 rows”. Errors always stay until you close them.</span>
          </span>
        </label>
      {/if}

      {#if shown("layout", "general")}
        <div class="setting">
          <span class="setting-title">Layout</span>
          <p>Docks and dividers remember where you leave them. Start over with every panel at its first size.</p>
          <button class="btn" onclick={resetLayout}>Reset the layout</button>
        </div>
      {/if}

      {#if shown("numbers", "data")}
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

      {#if shown("density", "data")}
        <div class="setting">
          <span class="setting-title">Row height</span>
          <p>How much room each table row takes. Compact fits more rows on screen.</p>
          <div class="flex gap-1" role="radiogroup" aria-label="Row height">
            {#each ROW_DENSITIES as d (d)}
              <button class="seg" role="radio" aria-checked={prefs.value.rowDensity === d} onclick={() => setNumbers({ rowDensity: d })}>{DENSITY_NAMES[d]}</button>
            {/each}
          </div>
        </div>
      {/if}

      {#if shown("missingText", "data")}
        <div class="setting">
          <span class="setting-title">Missing values in tables</span>
          <p>What an empty cell shows. Hover a cell for its exact value; exports are not changed.</p>
          <div class="flex gap-1" role="radiogroup" aria-label="Missing values in tables">
            {#each MISSING_TEXTS as t (t)}
              <button class="seg" role="radio" aria-checked={prefs.value.missingText === t} onclick={() => setNumbers({ missingText: t })}>{MISSING_NAMES[t]}</button>
            {/each}
          </div>
        </div>
      {/if}

      {#if shown("dates", "data")}
        <div class="setting">
          <span class="setting-title">Dates in tables</span>
          <p>How dates and times are shown. You still type dates as YYYY-MM-DD, and the tooltip and exports keep that form.</p>
          <div class="flex flex-wrap gap-1" role="radiogroup" aria-label="Dates in tables">
            {#each DATE_FORMATS as f (f)}
              <button class="seg" role="radio" aria-checked={prefs.value.dateFormat === f} onclick={() => setNumbers({ dateFormat: f })}>{formatDateText(DATE_EXAMPLE, f)}</button>
            {/each}
          </div>
        </div>
      {/if}

      {#if shown("markers", "data")}
        <div class="setting">
          <span class="setting-title">Values that mean “missing”</span>
          <p>A new Clean step treats these as missing, along with empty cells. Each step can change its own list.</p>
          <ul class="mb-2 flex flex-wrap gap-1.5" aria-label="Values that mean missing">
            {#each prefs.value.missingMarkers as m, i (i)}
              <li class="flex items-center gap-1 rounded border border-rule-strong bg-panel py-0.5 pr-0.5 pl-2 text-sm">
                <code class="code">{m}</code>
                <button class="btn btn-quiet !min-h-5 !min-w-5 justify-center !px-1" aria-label="Remove {m}" onclick={() => set({ missingMarkers: prefs.value.missingMarkers.filter((_, j) => j !== i) })}><X size={12} /></button>
              </li>
            {:else}
              <li class="text-sm text-ink-faint">None: only empty cells count.</li>
            {/each}
          </ul>
          <form class="flex flex-wrap gap-2" onsubmit={(e) => (e.preventDefault(), addMarker())}>
            <input class="field w-44" aria-label="Add a value" placeholder="Add a value" maxlength="40" bind:value={newMarker} />
            <button class="btn" disabled={!newMarker.trim()}>Add</button>
            <button class="btn btn-quiet" type="button" onclick={() => set({ missingMarkers: [...DEFAULTS.missingMarkers] })}>Back to {DEFAULTS.missingMarkers.join(", ")}</button>
          </form>
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

      {#if shown("motion", "appearance")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.reduceMotion} onchange={(e) => set({ reduceMotion: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Reduce motion</span>
            <span class="block text-sm text-ink-soft">Turns off animations and transitions. The computer's own reduce-motion setting is always followed too.</span>
          </span>
        </label>
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

      {#if shown("fontSize", "editor")}
        <div class="setting">
          <label for="set-code-size">Code text size</label>
          <p>Size of the text in code editors and consoles, apart from the interface's text size.</p>
          <select id="set-code-size" class="field" value={prefs.value.editorFontSize} onchange={(e) => set({ editorFontSize: Number(e.currentTarget.value) })}>
            {#each withCurrent([11, 12, 13, 14, 16, 18, 20], prefs.value.editorFontSize) as n (n)}<option value={n}>{n} px{n === DEFAULTS.editorFontSize ? " (default)" : ""}</option>{/each}
          </select>
        </div>
      {/if}

      {#if shown("tabWidth", "editor")}
        <div class="setting">
          <span class="setting-title">Indent width</span>
          <p>Spaces added by Tab and by automatic indenting. The Tab key indents; it never types a tab character.</p>
          <div class="flex gap-1" role="radiogroup" aria-label="Indent width">
            {#each TAB_WIDTHS as n (n)}
              <button class="seg" role="radio" aria-checked={prefs.value.tabWidth === n} onclick={() => set({ tabWidth: n })}>{n} spaces</button>
            {/each}
          </div>
        </div>
      {/if}

      {#if shown("lineNumbers", "editor")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.lineNumbers} onchange={(e) => set({ lineNumbers: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Show line numbers</span>
            <span class="block text-sm text-ink-soft">Beside multi-line code. Error messages name lines, so numbers help find them.</span>
          </span>
        </label>
      {/if}

      {#if shown("brackets", "editor")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.autoCloseBrackets} onchange={(e) => set({ autoCloseBrackets: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Close brackets and quotes automatically</span>
            <span class="block text-sm text-ink-soft">Typing ( or " adds the closing one and puts the cursor between.</span>
          </span>
        </label>
      {/if}

      {#if shown("activeLine", "editor")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.highlightActiveLine} onchange={(e) => set({ highlightActiveLine: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Highlight the current line</span>
            <span class="block text-sm text-ink-soft">A soft background on the line the cursor is on.</span>
          </span>
        </label>
      {/if}

      {#if shown("runAfterSave", "environments")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.runAfterSave} onchange={(e) => set({ runAfterSave: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Run the analysis after saving a step</span>
            <span class="block text-sm text-ink-soft">What the change affects reruns at once; nothing else does. Off: results show as outdated until you run.</span>
          </span>
        </label>
      {/if}

      {#if shown("runOnOpen", "environments")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.runOnOpen} onchange={(e) => set({ runOnOpen: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Run the analysis when a project opens</span>
            <span class="block text-sm text-ink-soft">Results are current as soon as you start. Python and R code waiting for your review is not run.</span>
          </span>
        </label>
      {/if}

      {#if shown("timeout", "environments")}
        <div class="setting">
          <label for="set-timeout">Time limit for new Python and R steps</label>
          <p>A step that runs longer is stopped. Each step can change its own limit; this is where new steps start.</p>
          <select id="set-timeout" class="field" value={prefs.value.stepTimeoutSeconds} onchange={(e) => set({ stepTimeoutSeconds: Number(e.currentTarget.value) })}>
            {#each withCurrent(STEP_TIMEOUTS, prefs.value.stepTimeoutSeconds) as n (n)}<option value={n}>{seconds(n)}{n === DEFAULTS.stepTimeoutSeconds ? " (default)" : ""}</option>{/each}
          </select>
        </div>
      {/if}

      {#if shown("memory", "environments")}
        <div class="setting">
          <label for="set-memory">Memory limit for Python and R</label>
          <p>Most memory a Python or R program may use (not on Windows). Default: 2 GB for Python steps, 4 GB for R steps and consoles. A new limit reloads the open project and closes any console.</p>
          <select
            id="set-memory"
            class="field"
            value={prefs.value.codeMemoryMb === undefined ? "default" : String(prefs.value.codeMemoryMb)}
            onchange={(e) => set({ codeMemoryMb: e.currentTarget.value === "default" ? undefined : Number(e.currentTarget.value) })}
          >
            <option value="default">Each language's own</option>
            {#each withCurrent(MEMORY_LIMITS_MB, prefs.value.codeMemoryMb && prefs.value.codeMemoryMb > 0 ? prefs.value.codeMemoryMb : 4096) as mb (mb)}<option value={String(mb)}>{gigabytes(mb)}</option>{/each}
            <option value="0">No limit</option>
          </select>
        </div>
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

      {#if shown("review", "safety")}
        <div class="setting">
          <span class="setting-title">Review before running</span>
          <p>
            Python and R code that someone else wrote asks for your review before it runs. Code you write or approve is remembered on this computer, never in a project, so a project cannot approve its own code.
          </p>
          {#if ws.view?.trust_all}
            <p class="!text-review">Code is running without review until you quit.</p>
            <button class="btn" onclick={() => ws.setTrustAll(false)}>Ask for review again</button>
          {:else}
            <button class="btn" onclick={trustAll} onblur={() => confirming === "trust" && (confirming = null)}>
              {confirming === "trust" ? "Click again to run everything without review" : "Run all code without review until I quit"}
            </button>
          {/if}
        </div>
      {/if}

      {#if shown("forget", "safety")}
        <div class="setting">
          <span class="setting-title">Approved code</span>
          <p>
            {#if about}{about.approvals} piece{about.approvals === 1 ? "" : "s"} of Python and R code {about.approvals === 1 ? "is" : "are"} approved to run on this computer.{:else}Which code is approved is not available now.{/if}
            Forgetting them makes every Python and R step ask for review again, in every project.
          </p>
          <button class="btn" disabled={!about?.approvals} onclick={forgetApprovals} onblur={() => confirming === "forget" && (confirming = null)}>
            {confirming === "forget" ? "Click again to forget them" : "Forget approved code"}
          </button>
        </div>
      {/if}

      {#if shown("versions", "projects")}
        <div class="setting">
          <label for="set-versions">Versions kept in a project's history</label>
          <p>Every saved change is a version you can restore. Past this number the oldest are deleted, in every project from now on.</p>
          <select id="set-versions" class="field" value={prefs.value.versionsKept} onchange={(e) => set({ versionsKept: Number(e.currentTarget.value) })}>
            {#each withCurrent(VERSION_COUNTS, prefs.value.versionsKept) as n (n)}<option value={n}>The last {n}{n === DEFAULTS.versionsKept ? " (default)" : ""}</option>{/each}
          </select>
        </div>
      {/if}

      {#if shown("exportFormat", "projects")}
        <div class="setting">
          <span class="setting-title">Export tables as</span>
          <p>The format the palette's “Export this table” uses, listed first in Export menus, and the one a save dialog starts on.</p>
          <div class="flex gap-1" role="radiogroup" aria-label="Export tables as">
            {#each EXPORT_FORMATS as f (f)}
              <button class="seg" role="radio" aria-checked={prefs.value.exportFormat === f} onclick={() => set({ exportFormat: f })}>{EXPORT_NAMES[f]}</button>
            {/each}
          </div>
        </div>
      {/if}

      {#if shown("reopen", "projects")}
        <label class="setting setting-check">
          <input type="checkbox" checked={prefs.value.reopenLastProject} onchange={(e) => set({ reopenLastProject: e.currentTarget.checked })} />
          <span>
            <span class="font-semibold">Reopen the last project at start</span>
            <span class="block text-sm text-ink-soft">The app opens the project you were working on instead of the welcome page. If it has moved, you see the welcome page.</span>
          </span>
        </label>
      {/if}

      {#if shown("folders", "projects")}
        <div class="setting">
          <span class="setting-title">Where file dialogs start</span>
          <p>Folder pickers (new projects) and save dialogs (exports, notebooks, reports) start here. Empty means the system's choice.</p>
          <div class="space-y-2">
            {#each [["projectsFolder", "Projects"], ["exportsFolder", "Exports"]] as [key, name] (key)}
              {@const k = key as "projectsFolder" | "exportsFolder"}
              <div class="flex flex-wrap items-center gap-2">
                <span class="w-20 text-sm">{name}</span>
                <span class="min-w-0 max-w-96 flex-1 truncate rounded border border-rule bg-paper px-2 py-1 text-sm {prefs.value[k] ? '' : 'text-ink-faint'}" title={prefs.value[k]}>{prefs.value[k] ?? "System default"}</span>
                <button class="btn" onclick={() => chooseFolder(k)}>Choose…</button>
                <button class="btn btn-quiet" disabled={!prefs.value[k]} onclick={() => set(k === "projectsFolder" ? { projectsFolder: undefined } : { exportsFolder: undefined })}>Use default</button>
              </div>
            {/each}
          </div>
        </div>
      {/if}

      {#if shown("shortcuts", "keyboard")}
        <div class="setting">
          <span class="setting-title">Keyboard shortcuts</span>
          <p>Shortcuts are fixed for now. Every action is also in the command palette ({mac ? "⌘K" : "Ctrl+K"}), by name.</p>
          <div class="space-y-5">
            {#each keyGroups as g (g.name)}
              <section aria-label={g.name}>
                <h3 class="mb-1 text-sm font-semibold">{g.name}</h3>
                <dl class="divide-y divide-rule rounded-md border border-rule text-sm">
                  {#each g.items as item (item.what)}
                    <div class="flex items-start gap-4 px-3 py-1.5">
                      <dt class="w-56 shrink-0 font-mono text-xs leading-6 text-ink">{item.keys}</dt>
                      <dd class="min-w-0 flex-1 text-ink-soft">{item.what}</dd>
                    </div>
                  {/each}
                </dl>
              </section>
            {/each}
          </div>
        </div>
      {/if}

      {#if shown("version", "about")}
        <div class="setting">
          <span class="setting-title">Visual data analysis {about?.version ?? ""}</span>
          <p>Analyses run on this computer. Nothing is sent anywhere, and no account is needed.</p>
        </div>
      {/if}

      {#if shown("paths", "about")}
        <div class="setting">
          <span class="setting-title">Where this is kept</span>
          <p>Settings and approved code live outside every project, in your user folder.</p>
          <dl class="space-y-2 text-sm">
            {#each [["Settings file", about?.settings_path], ["Approvals file", about?.approvals_path]] as [name, path] (name)}
              <div class="flex flex-wrap items-center gap-2">
                <dt class="w-28">{name}</dt>
                <dd class="min-w-0 max-w-[28rem] flex-1 truncate rounded border border-rule bg-paper px-2 py-1 font-mono text-xs" title={path ?? undefined}>{path ?? "Not kept (this session only)"}</dd>
                <button class="btn btn-quiet" disabled={!path} aria-label="Copy the {name!.toLowerCase()} location" onclick={() => path && copy(path, `the ${name!.toLowerCase()} location`)}><Copy size={14} /> Copy</button>
              </div>
            {/each}
          </dl>
        </div>
      {/if}

      {#if shown("diagnostics", "about")}
        <div class="setting">
          <span class="setting-title">Diagnostics</span>
          <p>Version, system, and which Python and R are in use, as text for a bug report. It includes file paths and no data.</p>
          <button class="btn" onclick={copyDiagnostics}><Copy size={14} /> Copy diagnostics</button>
        </div>
      {/if}

      {#if shown("backup", "about")}
        <div class="setting">
          <span class="setting-title">Back up, move, or reset settings</span>
          <p>Export every setting except your name and the last project, to use on another computer. Importing changes only the settings the file holds.</p>
          <div class="flex flex-wrap gap-2">
            <button class="btn" onclick={exportSettings}><Download size={14} /> Export settings</button>
            <label class="btn cursor-pointer"><Upload size={14} /> Import a file<input type="file" accept=".json,application/json" class="sr-only" onchange={importSettingsFile} /></label>
            <button class="btn" onclick={resetSettings} onblur={() => confirming === "reset" && (confirming = null)}>
              {confirming === "reset" ? "Click again to reset everything" : "Reset all settings"}
            </button>
          </div>
          {#if settingsOut}
            <p class="mt-2">Copied. Save it as a .json file:</p>
            <textarea class="field code mt-1 h-32 w-full" readonly aria-label="Exported settings">{settingsOut}</textarea>
          {/if}
          <details class="mt-2">
            <summary class="cursor-pointer text-sm">Paste settings instead</summary>
            <textarea class="field code mt-1 h-24 w-full" aria-label="Settings to import" placeholder="Paste settings exported from here" bind:value={settingsText}></textarea>
            <button class="btn mt-1" disabled={!settingsText.trim()} onclick={() => importSettings(settingsText)}>Import</button>
          </details>
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
