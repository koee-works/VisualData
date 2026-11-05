<script lang="ts">
  import { Dialog } from "bits-ui";
  import { Search } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import { filter, loadRecent, remember, type Command } from "../commands";
  import { THEMES } from "../theme";
  import { prefs, updatePrefs } from "../prefs.svelte";
  import { stepTitle } from "../format";
  import type { PaneName } from "../layout";

  // Every action in the app by name, as in VS Code and Zed: type a few
  // letters, Enter to do it.
  let { ws }: { ws: Workspace } = $props();

  const mac = typeof navigator !== "undefined" && /Mac/.test(navigator.platform);
  const key = mac ? "⌘" : "Ctrl+";

  let query = $state("");
  let active = $state(0);
  let recent = $state<string[]>(loadRecent());
  let list = $state<HTMLElement | null>(null);

  let commands = $derived.by((): Command[] => {
    const v = ws.view;
    const running = ws.running;
    const step = ws.centerTab === "step" ? ws.selectedStep : null;
    const code = step && (step.op.type === "python" || step.op.type === "r") ? step : null;
    const busy = running ? "Wait for the run to finish" : undefined;
    const pane = (name: PaneName, title: string, shortcut?: string, disabled?: string): Command => ({
      id: `pane.${name}`,
      group: "Actions",
      title: `${ws.panes[name] ? "Hide" : "Show"} ${title}`,
      keywords: "toggle panel dock",
      shortcut,
      disabled,
      run: () => ws.togglePane(name),
    });
    const out: Command[] = [
      running
        ? { id: "cancel", group: "Actions", title: "Cancel the run", keywords: "stop", run: () => ws.cancel() }
        : { id: "run", group: "Actions", title: "Run analysis", keywords: "refresh execute", disabled: v?.steps.length ? undefined : "Add a step first", run: () => ws.run() },
      { id: "undo", group: "Actions", title: v?.undo ? `Undo: ${v.undo}` : "Undo", shortcut: `${key}Z`, disabled: busy ?? (v?.undo ? undefined : "Nothing to undo"), run: () => ws.undo() },
      { id: "redo", group: "Actions", title: v?.redo ? `Redo: ${v.redo}` : "Redo", shortcut: mac ? "⇧⌘Z" : "Ctrl+Y", disabled: busy ?? (v?.redo ? undefined : "Nothing to redo"), run: () => ws.redo() },
      ...ws.stepKinds.map(
        (k): Command => ({
          id: `add.${k.type}`,
          group: "Actions",
          title: `Add step: ${k.name}`,
          keywords: `new ${k.type}`,
          disabled: busy,
          run: () => {
            if (ws.addStepKind && !ws.addStepKind(k.type)) ws.notice = `${k.name} needs a second table: add a source or another step first.`;
          },
        }),
      ),
      { id: "add.source", group: "Actions", title: "Add a source", keywords: "csv excel parquet database file import", disabled: busy, run: () => ws.addSource() },
      { id: "review", group: "Actions", title: "Review code waiting for approval", keywords: "python r approve trust", disabled: ws.pendingReview.length ? undefined : "No code is waiting", run: () => (ws.reviewOpen = true) },
      { id: "export.html", group: "Actions", title: "Export notebook as a web page", keywords: "html report share", run: () => ws.exportNotebook("html") },
      { id: "export.ipynb", group: "Actions", title: "Export notebook for Jupyter (Python)", keywords: "ipynb python", run: () => ws.exportNotebook("python") },
      { id: "export.ipynb-sql", group: "Actions", title: "Export notebook for Jupyter (SQL)", keywords: "ipynb sql duckdb", run: () => ws.exportNotebook("sql") },
      { id: "export.ipynb-r", group: "Actions", title: "Export notebook for Jupyter (R)", keywords: "ipynb r", run: () => ws.exportNotebook("r") },
      ...(code
        ? [
            { id: "external", group: "Actions", title: `Open ${stepTitle(code)} in ${code.op.type === "r" ? "RStudio" : "Jupyter"}`, keywords: "edit externally other editor", run: () => ws.editExternally(code.id) } as Command,
          ]
        : []),
      pane("pipeline", "pipeline", `${key}B`),
      pane("editor", ws.selectedSource ? "source details" : "step editor", undefined, ws.centerTab === "step" ? undefined : "Only on the Step tab"),
      pane("result", "result", undefined, ws.centerTab === "step" ? undefined : "Only on the Step tab"),
      pane("activity", "activity", `${key}J`),
      pane("console", "code console", `${key}\``, code ? undefined : "Only for Python and R steps"),
      { id: "pane.all", group: "Actions", title: "Show all panels", keywords: "reset layout", run: () => ws.showAllPanes() },
      ...[{ id: "system", name: "Match the computer" }, ...THEMES, ...prefs.value.customThemes].map(
        (t): Command => ({
          id: `theme.${t.id}`,
          group: "Actions",
          title: `Theme: ${t.name}${prefs.value.theme === t.id ? " (in use)" : ""}`,
          keywords: "colour color appearance dark light",
          run: () => void updatePrefs({ theme: t.id }),
        }),
      ),
      { id: "theme.new", group: "Actions", title: "Make your own theme", keywords: "custom colours appearance", run: () => ws.openSettings("appearance") },
      { id: "python", group: "Actions", title: "Choose the Python environment", keywords: "interpreter venv conda", run: () => (ws.pythonOpen = true) },
      { id: "r", group: "Actions", title: "Choose the R installation", keywords: "rscript", run: () => (ws.rOpen = true) },
      { id: "schedule", group: "Actions", title: "Refresh on a schedule", keywords: "cron vda-run automate", run: () => (ws.scheduleOpen = true) },
      { id: "go.settings", group: "Go to", title: "Settings", shortcut: `${key},`, keywords: "preferences options", run: () => ws.openSettings() },
      { id: "go.settings.general", group: "Go to", title: "Settings: General", keywords: "author numbers run after save", run: () => ws.openSettings("general") },
      { id: "go.settings.appearance", group: "Go to", title: "Settings: Appearance", keywords: "theme text size", run: () => ws.openSettings("appearance") },
      { id: "go.settings.editor", group: "Go to", title: "Settings: Code editor", keywords: "vim wrap keys", run: () => ws.openSettings("editor") },
      { id: "go.settings.environments", group: "Go to", title: "Settings: Python and R", run: () => ws.openSettings("environments") },
      { id: "go.notebook", group: "Go to", title: "Notebook", keywords: "report cells", run: () => (ws.centerTab = "notebook") },
      { id: "go.problems", group: "Go to", title: "Problems", keywords: "lint warnings errors", run: () => openActivity("problems") },
      { id: "go.history", group: "Go to", title: "History", keywords: "versions restore", run: () => openActivity("history") },
      { id: "go.log", group: "Go to", title: "Run log", keywords: "runs timings", run: () => openActivity("log") },
      ...(v?.sources ?? []).map((s): Command => ({ id: `go.source.${s.id}`, group: "Go to", title: `Source: ${s.id}`, keywords: s.path, run: () => select(s.id) })),
      ...(v?.steps ?? []).map((s, i): Command => ({ id: `go.step.${s.id}`, group: "Go to", title: `Step ${i + 1}: ${stepTitle(s)}`, keywords: s.op.type, run: () => select(s.id) })),
    ];
    return out;
  });

  let shown = $derived(filter(commands, query, recent));

  function openActivity(tab: "problems" | "history" | "log") {
    ws.consoleTab = tab;
    if (!ws.panes.activity) ws.togglePane("activity");
  }

  function select(id: string) {
    ws.selected = id;
    ws.centerTab = "step";
  }

  function choose(c: Command | undefined) {
    if (!c || c.disabled) return;
    recent = remember(c.id, recent);
    ws.paletteOpen = false;
    // After the dialog has closed and given focus back.
    setTimeout(() => c.run(), 0);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = shown.length;
      if (n) active = (active + (e.key === "ArrowDown" ? 1 : n - 1)) % n;
      list?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(shown[active]);
    }
  }

  $effect(() => {
    if (ws.paletteOpen) {
      query = "";
      active = 0;
    }
  });
</script>

<Dialog.Root bind:open={ws.paletteOpen}>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-40 bg-black/25" />
    <Dialog.Content
      class="fixed top-[12vh] left-1/2 z-50 flex max-h-[min(32rem,76vh)] w-[min(38rem,94vw)] -translate-x-1/2 flex-col overflow-hidden rounded-lg border border-rule-strong bg-panel shadow-[var(--shadow-pop)]"
      aria-label="Command palette"
    >
      <Dialog.Title class="sr-only">Command palette</Dialog.Title>
      <div class="flex items-center gap-2 border-b border-rule px-3">
        <Search size={16} class="shrink-0 text-ink-faint" aria-hidden="true" />
        <input
          class="min-h-11 w-full bg-transparent text-base outline-none placeholder:text-ink-faint"
          placeholder="Type a command, a step or a setting"
          aria-label="Command"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={shown.length ? `palette-${active}` : undefined}
          bind:value={query}
          oninput={() => (active = 0)}
          {onkeydown}
          spellcheck="false"
        />
      </div>
      <ul id="palette-list" class="min-h-0 flex-1 overflow-y-auto p-1" role="listbox" aria-label="Commands" bind:this={list}>
        {#each shown as c, i (c.id)}
          {#if !query.trim() ? i === 0 || (recent.includes(shown[i - 1]!.id) && !recent.includes(c.id)) : i === 0 || shown[i - 1]!.group !== c.group}
            <li class="px-2 pt-2 pb-1 text-xs font-semibold text-ink-faint" role="presentation">
              {!query.trim() ? (recent.includes(c.id) ? "Recently used" : "All commands") : c.group}
            </li>
          {/if}
          <!-- The search field takes the keys (arrows and Enter), as a combobox does. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <li
            id="palette-{i}"
            data-index={i}
            role="option"
            aria-selected={i === active}
            aria-disabled={c.disabled ? "true" : undefined}
            class="flex cursor-pointer items-center gap-3 rounded px-2 py-1.5 text-sm"
            class:bg-select={i === active}
            class:text-ink-faint={!!c.disabled}
            onmousemove={() => (active = i)}
            onclick={() => choose(c)}
          >
            <span class="min-w-0 flex-1 truncate">{c.title}</span>
            {#if c.disabled && i === active}<span class="shrink-0 text-xs">{c.disabled}</span>{/if}
            {#if c.shortcut}<kbd class="code shrink-0 rounded border border-rule px-1.5 text-xs text-ink-soft">{c.shortcut}</kbd>{/if}
          </li>
        {:else}
          <li class="px-3 py-6 text-center text-sm text-ink-soft" role="presentation">No command matches “{query}”.</li>
        {/each}
      </ul>
      <p class="border-t border-rule px-3 py-1.5 text-xs text-ink-faint">↑↓ to move, Enter to choose, Esc to close</p>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
