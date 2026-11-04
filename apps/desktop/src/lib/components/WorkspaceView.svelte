<script lang="ts">
  import { Tabs } from "bits-ui";
  import { X } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import TopBar from "./TopBar.svelte";
  import Pipeline from "./Pipeline.svelte";
  import StepEditor from "./StepEditor.svelte";
  import ResultPanel from "./ResultPanel.svelte";
  import Notebook from "./Notebook.svelte";
  import SourceOverview from "./SourceOverview.svelte";
  import ReviewDialog from "./ReviewDialog.svelte";
  import PythonDialog from "./PythonDialog.svelte";
  import RDialog from "./RDialog.svelte";
  import ScheduleDialog from "./ScheduleDialog.svelte";
  import Console from "./Console.svelte";
  import StatusBar from "./StatusBar.svelte";
  import CodeConsole from "./CodeConsole.svelte";
  import SettingsPage from "./SettingsPage.svelte";
  import CommandPalette from "./CommandPalette.svelte";
  import { editorParameters } from "../editor/complete";

  let { ws }: { ws: Workspace } = $props();

  import { paneKind, resultHeight, defaultSplit, savedSplit, saveSplit, savedSidebar, saveSidebar, savedDock, saveDock, clamp, SPLIT_MIN, SPLIT_MAX, SIDEBAR_MIN, SIDEBAR_MAX } from "../layout";

  // The editor's share of the centre column, per kind of step: what the user
  // last dragged to, else room for a small result, else a default.
  let kind = $derived(paneKind(ws.selectedStep, ws.selectedSource));
  let chosen = $state<Record<string, number | null>>({});
  let split = $derived(chosen[kind] !== undefined ? chosen[kind] : savedSplit(kind));
  let auto = $derived(split === null ? resultHeight(kind, ws.selectedStep?.rows ?? null, ws.selectedStep?.state === "stale") : null);
  let rows = $derived(
    auto !== null ? `minmax(0,1fr) 8px ${auto}px` : `${split ?? defaultSplit(kind)}fr 8px ${1 - (split ?? defaultSplit(kind))}fr`,
  );
  function setSplit(v: number | null) {
    chosen[kind] = v === null ? null : clamp(v, SPLIT_MIN, SPLIT_MAX);
    saveSplit(kind, chosen[kind] ?? null);
  }

  let sidebar = $state(savedSidebar() ?? 272);
  // Docks, as in Zed: the console on the right, Activity at the bottom.
  let right = $state(savedDock("right") ?? 460);
  let bottom = $state(savedDock("bottom") ?? 224);
  let codeStep = $derived(ws.centerTab === "step" && (ws.selectedStep?.op.type === "python" || ws.selectedStep?.op.type === "r"));
  let showConsole = $derived(codeStep && ws.panes.console);
  let consoleLang = $derived(ws.selectedStep?.op.type === "r" ? ("r" as const) : ("python" as const));
  let consoleContext = $derived({
    inputs: (ws.selectedStep?.inputs ?? []).map((id) => ({ id, name: ws.nameOf(id), columns: ws.columnsOf(id) })),
    parameters: editorParameters(ws.view?.parameters ?? []),
  });
  let consoleRef: CodeConsole | undefined = $state();
  $effect(() => {
    ws.codeConsole = consoleRef ?? null;
  });
  let columns = $derived(
    [ws.panes.pipeline ? `${sidebar}px 6px` : "", "minmax(0,1fr)", showConsole ? `6px ${right}px` : ""].filter(Boolean).join(" "),
  );

  // Opening History from the menu (or anything that opens the Activity
  // panel) shows the panel even if it was hidden.
  $effect(() => {
    if (ws.consoleOpen && !ws.panes.activity) ws.togglePane("activity");
    else if (!ws.consoleOpen && ws.panes.activity) ws.togglePane("activity");
  });
  let dragging = $state<"split" | "sidebar" | "right" | "bottom" | null>(null);
  let main: HTMLElement | undefined = $state();
  let centre: HTMLElement | undefined = $state();

  function drag(e: PointerEvent) {
    if (dragging === "split" && centre) {
      const r = centre.getBoundingClientRect();
      setSplit((e.clientY - r.top) / r.height);
    } else if (dragging === "sidebar") {
      sidebar = clamp(e.clientX, SIDEBAR_MIN, SIDEBAR_MAX);
    } else if (dragging === "right" && main) {
      right = clamp(main.getBoundingClientRect().right - e.clientX, 300, 900);
    } else if (dragging === "bottom" && main) {
      bottom = clamp(main.getBoundingClientRect().bottom - e.clientY, 120, 600);
    }
  }
  function stop() {
    if (dragging === "sidebar") saveSidebar(sidebar);
    if (dragging === "right") saveDock("right", right);
    if (dragging === "bottom") saveDock("bottom", bottom);
    dragging = null;
  }
  function dockKeys(which: "right" | "bottom", e: KeyboardEvent) {
    const step = which === "right" ? (e.key === "ArrowLeft" ? 24 : e.key === "ArrowRight" ? -24 : 0) : e.key === "ArrowUp" ? 24 : e.key === "ArrowDown" ? -24 : 0;
    if (!step) return;
    e.preventDefault();
    if (which === "right") saveDock("right", (right = clamp(right + step, 300, 900)));
    else saveDock("bottom", (bottom = clamp(bottom + step, 120, 600)));
  }
  // Keyboard: arrows move the divider; Home puts it back to fit the result.
  function splitKeys(e: KeyboardEvent) {
    const now = split ?? (centre && auto !== null ? 1 - (auto + 8) / centre.getBoundingClientRect().height : defaultSplit(kind));
    if (e.key === "ArrowUp") setSplit(now - 0.05);
    else if (e.key === "ArrowDown") setSplit(now + 0.05);
    else if (e.key === "Home") setSplit(null);
    else return;
    e.preventDefault();
  }
  function sidebarKeys(e: KeyboardEvent) {
    if (e.key === "ArrowLeft") sidebar = clamp(sidebar - 24, SIDEBAR_MIN, SIDEBAR_MAX);
    else if (e.key === "ArrowRight") sidebar = clamp(sidebar + 24, SIDEBAR_MIN, SIDEBAR_MAX);
    else return;
    e.preventDefault();
    saveSidebar(sidebar);
  }
</script>

<svelte:window onpointermove={drag} onpointerup={stop} />

<ReviewDialog {ws} />
<PythonDialog {ws} />
<RDialog {ws} />
<ScheduleDialog {ws} />
<CommandPalette {ws} />

<div class="grid h-full grid-rows-[auto_1fr_auto]">
  <TopBar {ws} />
  <div bind:this={main} class="grid min-h-0" class:select-none={dragging !== null} style:grid-template-columns={columns}>
    {#if ws.panes.pipeline}
      <aside class="min-h-0 bg-panel">
        <Pipeline {ws} />
      </aside>
      <button
        class="divider divider-v"
        class:active={dragging === "sidebar"}
        aria-label="Resize the pipeline (arrow keys)"
        onpointerdown={() => (dragging = "sidebar")}
        onkeydown={sidebarKeys}
      ></button>
    {/if}

    <Tabs.Root bind:value={ws.centerTab} class="flex min-h-0 min-w-0 flex-col">
      <Tabs.List class="flex border-b border-rule bg-panel px-3">
        <Tabs.Trigger value="step" class="tab">{ws.selectedSource ? "Source" : "Step"}</Tabs.Trigger>
        <Tabs.Trigger value="notebook" class="tab">Notebook</Tabs.Trigger>
        {#if ws.settingsOpen}
          <span class="flex items-center">
            <Tabs.Trigger value="settings" class="tab !pr-1">Settings</Tabs.Trigger>
            <button class="btn btn-quiet !min-h-6 !min-w-6 justify-center !px-1" aria-label="Close settings" onclick={() => ws.closeSettings()}><X size={13} /></button>
          </span>
        {/if}
      </Tabs.List>

      {#if ws.notice && !ws.error}
        <div class="flex items-start gap-3 border-b border-fresh/40 bg-fresh-wash px-4 py-2 text-sm" role="status">
          <p class="min-w-0 flex-1 truncate" title={ws.notice}>{ws.notice}</p>
          <button class="btn btn-quiet !min-h-6 !min-w-6 justify-center !px-1" aria-label="Dismiss" onclick={() => (ws.notice = null)}><X size={14} /></button>
        </div>
      {/if}
      {#if ws.error}
        <div class="flex items-start gap-3 border-b border-failed/40 bg-failed-wash px-4 py-2 text-sm" role="alert">
          <p class="flex-1"><span class="font-semibold text-failed">{ws.error.action} failed.</span> {ws.error.message}</p>
          <button class="btn btn-quiet !min-h-6 !min-w-6 justify-center !px-1" aria-label="Dismiss" onclick={() => (ws.error = null)}><X size={14} /></button>
        </div>
      {/if}

      <Tabs.Content value="step" class="min-h-0 min-w-0 flex-1">
        <!-- min-w-0 on every grid item: a wide result scrolls inside its panel
             instead of widening the window. -->
        <div bind:this={centre} class="grid h-full min-h-0 min-w-0 grid-cols-[minmax(0,1fr)]" style:grid-template-rows={ws.panes.editor && ws.panes.result ? rows : "minmax(0,1fr)"}>
          {#if ws.panes.editor}
          <div class="min-h-0 min-w-0 overflow-hidden bg-panel">
            {#if ws.selectedStep}
              <StepEditor {ws} step={ws.selectedStep} />
            {:else}
              {#if ws.selectedSource}<SourceOverview {ws} source={ws.selectedSource} />{/if}
            {/if}
          </div>
          {/if}
          {#if ws.panes.editor && ws.panes.result}
          <button
            class="divider divider-h"
            class:active={dragging === "split"}
            aria-label="Resize editor and result (arrow keys; Home fits the result, double-click too)"
            title="Drag to resize; double-click to fit the result"
            onpointerdown={() => (dragging = "split")}
            ondblclick={() => setSplit(null)}
            onkeydown={splitKeys}
          ></button>
          {/if}
          {#if ws.panes.result}
          <div class="min-h-0 min-w-0 overflow-hidden">
            <ResultPanel {ws} />
          </div>
          {/if}
        </div>
      </Tabs.Content>
      <Tabs.Content value="notebook" class="min-h-0 flex-1 overflow-y-auto">
        <Notebook {ws} />
      </Tabs.Content>
      {#if ws.settingsOpen}
        <Tabs.Content value="settings" class="min-h-0 min-w-0 flex-1 bg-panel">
          <SettingsPage {ws} />
        </Tabs.Content>
      {/if}
      {#if ws.panes.activity}
        <button
          class="divider divider-h shrink-0"
          style:height="6px"
          class:active={dragging === "bottom"}
          aria-label="Resize the Activity panel (arrow keys)"
          onpointerdown={() => (dragging = "bottom")}
          onkeydown={(e) => dockKeys("bottom", e)}
        ></button>
        <div class="flex min-h-0 shrink-0 flex-col" style:height="{bottom}px"><Console {ws} /></div>
      {/if}
    </Tabs.Root>

    {#if showConsole && ws.selectedStep}
      <button
        class="divider divider-v"
        class:active={dragging === "right"}
        aria-label="Resize the console (arrow keys)"
        onpointerdown={() => (dragging = "right")}
        onkeydown={(e) => dockKeys("right", e)}
      ></button>
      <aside class="min-h-0 min-w-0 border-l border-rule bg-panel" aria-label="Right dock">
        {#key ws.selectedStep.id}
          <CodeConsole bind:this={consoleRef} {ws} stepId={ws.selectedStep.id} language={consoleLang} context={consoleContext} ongoto={(n) => ws.focusCodeLine?.(n)} docked />
        {/key}
      </aside>
    {/if}
  </div>
  <StatusBar {ws} />
</div>
