<script lang="ts">
  import { CalendarClock, Command, Ellipsis, Settings, Play, Redo2, ShieldAlert, ShieldOff, Square, Terminal, Undo2 } from "@lucide/svelte";
  import type { PaneName } from "../layout";
  import { DropdownMenu } from "bits-ui";
  import type { Workspace } from "../workspace.svelte";
  import { formatCount } from "../format";

  let { ws }: { ws: Workspace } = $props();

  let name = $state("");
  $effect.pre(() => {
    name = ws.view?.name ?? "";
  });

  let steps = $derived(ws.view?.steps ?? []);
  let outdated = $derived(steps.filter((s) => s.state !== "fresh").length);
  let done = $derived(Object.keys(ws.progress).length);
  let lastFailed = $derived(Object.values(ws.progress).filter((e) => e.status === "failed").length);

  const mac = typeof navigator !== "undefined" && /Mac/.test(navigator.platform);
  const key = mac ? "⌘" : "Ctrl+";

  let review = $derived(ws.pendingReview.length);

  let codeStep = $derived(ws.centerTab === "step" && (ws.selectedStep?.op.type === "python" || ws.selectedStep?.op.type === "r"));


  // Undo and redo the analysis, but not while typing: text fields keep
  // their own undo.
  function onkeydown(e: KeyboardEvent) {
    if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
    // The command palette, as in VS Code (Ctrl+Shift+P) and Zed (Ctrl+K too).
    if ((e.shiftKey && e.code === "KeyP") || (!e.shiftKey && e.code === "KeyK")) {
      e.preventDefault();
      ws.paletteOpen = !ws.paletteOpen;
      return;
    }
    // Settings, as in Zed and VS Code.
    if (e.key === "," && !e.shiftKey) {
      e.preventDefault();
      ws.openSettings();
      return;
    }
    // Panels, as in VS Code: these work from anywhere, even while typing.
    const pane: PaneName | null = e.shiftKey ? null : e.code === "KeyB" ? "pipeline" : e.code === "KeyJ" ? "activity" : e.code === "Backquote" ? "console" : null;
    if (pane && (pane !== "console" || codeStep)) {
      e.preventDefault();
      ws.togglePane(pane);
      return;
    }
    const t = e.target as HTMLElement | null;
    if (t && (t.closest("input, textarea, select, [contenteditable=true]") || t.isContentEditable)) return;
    const k = e.key.toLowerCase();
    if (k === "z" && !e.shiftKey && ws.view?.undo && !ws.running) {
      e.preventDefault();
      ws.undo();
    } else if (((k === "z" && e.shiftKey) || k === "y") && ws.view?.redo && !ws.running) {
      e.preventDefault();
      ws.redo();
    }
  }

  let summary = $derived.by(() => {
    if (ws.running) return `Running step ${Math.min(done + 1, steps.length)} of ${steps.length}`;
    if (!steps.length) return "Add a step to start";
    if (lastFailed) return `${lastFailed} step${lastFailed === 1 ? "" : "s"} failed on the last run`;
    if (!outdated) return "All results current";
    return `${outdated} of ${steps.length} results need a run`;
  });
</script>

<svelte:window {onkeydown} />

<header class="flex items-center gap-4 border-b border-rule bg-panel px-4 py-2">
  <input
    class="min-w-0 flex-1 rounded px-1 py-0.5 text-lg font-bold hover:bg-paper focus:bg-paper focus:outline-none"
    aria-label="Analysis name"
    bind:value={name}
    onchange={() => name.trim() && name !== ws.view?.name && ws.rename(name.trim())}
  />
  <p class="hidden text-sm text-ink-soft md:block" title={ws.view?.project_dir}>
    {#if (ws.view?.sources.length ?? 0) === 1}
      {ws.view?.sources[0]?.path}, {formatCount(ws.view?.sources[0]?.rows ?? 0)} rows
    {:else}
      {ws.view?.sources.length} sources
    {/if}
  </p>
  {#if ws.view?.trust_all}
    <p class="flex items-center gap-2 rounded-md border border-review/50 bg-review-wash py-0.5 pr-0.5 pl-2.5 text-sm text-review">
      <ShieldOff size={14} aria-hidden="true" /> Python runs without review until you quit
      <button class="btn !min-h-6 !py-0 text-sm" onclick={() => ws.setTrustAll(false)}>Ask again</button>
    </p>
  {/if}
  {#if review && !ws.running}
    <button class="btn border-review/50 text-review" onclick={() => (ws.reviewOpen = true)}>
      <ShieldAlert size={14} /> Review {review} Python step{review === 1 ? "" : "s"}
    </button>
  {/if}
  <div class="flex items-center">
    <button
      class="btn btn-quiet !px-2"
      aria-label="Undo"
      title={ws.view?.undo ? `Undo: ${ws.view.undo} (${key}Z)` : "Nothing to undo"}
      disabled={!ws.view?.undo || ws.running}
      onclick={() => ws.undo()}
    >
      <Undo2 size={16} />
    </button>
    <button
      class="btn btn-quiet !px-2"
      aria-label="Redo"
      title={ws.view?.redo ? `Redo: ${ws.view.redo} (${mac ? "⇧⌘Z" : "Ctrl+Y"})` : "Nothing to redo"}
      disabled={!ws.view?.redo || ws.running}
      onclick={() => ws.redo()}
    >
      <Redo2 size={16} />
    </button>
    <DropdownMenu.Root>
      <DropdownMenu.Trigger class="btn btn-quiet !px-2" aria-label="More">
        <Ellipsis size={16} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          class="z-50 min-w-64 rounded-md border border-rule-strong bg-panel p-1 shadow-[var(--shadow-menu)]"
          align="end"
          sideOffset={4}
        >
          {#each [{ icon: Command, name: "All commands", hint: `Find any action by name (${mac ? "⇧⌘P" : "Ctrl+Shift+P"})`, open: () => (ws.paletteOpen = true) }, { icon: Settings, name: "Settings", hint: `General, appearance, the code editor (${key},)`, open: () => ws.openSettings() }, { icon: Terminal, name: "Python environment", hint: "Which Python runs your steps", open: () => (ws.pythonOpen = true) }, { icon: Terminal, name: "R installation", hint: "Which R runs your steps", open: () => (ws.rOpen = true) }, { icon: CalendarClock, name: "Refresh on a schedule", hint: "Rerun and export without the app", open: () => (ws.scheduleOpen = true) }, { icon: Undo2, name: "History", hint: "Every saved version", open: () => ((ws.consoleTab = "history"), (ws.consoleOpen = true)) }] as item (item.name)}
            <DropdownMenu.Item
              class="flex cursor-pointer items-start gap-2 rounded px-2 py-1.5 outline-none data-[highlighted]:bg-select"
              onSelect={item.open}
            >
              <item.icon size={15} class="mt-0.5 text-ink-soft" />
              <span><span class="block">{item.name}</span><span class="block text-xs text-ink-faint">{item.hint}</span></span>
            </DropdownMenu.Item>
          {/each}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  </div>
  {#if ws.running}
    <button class="btn" onclick={() => ws.cancel()}><Square size={14} /> Cancel</button>
  {:else}
    <button class="btn btn-primary" onclick={() => ws.run()} disabled={!steps.length}>
      <Play size={14} /> Run analysis
    </button>
  {/if}
</header>
