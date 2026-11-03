<script lang="ts">
  import { Tabs } from "bits-ui";
  import { X } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import { SOURCE_ID } from "../api/types";
  import TopBar from "./TopBar.svelte";
  import Pipeline from "./Pipeline.svelte";
  import StepEditor from "./StepEditor.svelte";
  import ResultPanel from "./ResultPanel.svelte";
  import Notebook from "./Notebook.svelte";
  import SourceOverview from "./SourceOverview.svelte";
  import ReviewDialog from "./ReviewDialog.svelte";

  let { ws }: { ws: Workspace } = $props();

  // Share of the centre column given to the editor; the rest is the result.
  let split = $state(0.45);
  let dragging = $state(false);
  let centre: HTMLElement | undefined = $state();

  function drag(e: PointerEvent) {
    if (!dragging || !centre) return;
    const r = centre.getBoundingClientRect();
    split = Math.min(0.8, Math.max(0.15, (e.clientY - r.top) / r.height));
  }
</script>

<svelte:window onpointermove={drag} onpointerup={() => (dragging = false)} />

<ReviewDialog {ws} />

<div class="grid h-full grid-rows-[auto_1fr]">
  <TopBar {ws} />
  <div class="grid min-h-0 grid-cols-[17rem_1fr]">
    <aside class="min-h-0 border-r border-rule bg-panel">
      <Pipeline {ws} />
    </aside>

    <Tabs.Root bind:value={ws.centerTab} class="flex min-h-0 min-w-0 flex-col">
      <Tabs.List class="flex border-b border-rule bg-panel px-3">
        <Tabs.Trigger value="step" class="tab">{ws.selected === SOURCE_ID ? "Source" : "Step"}</Tabs.Trigger>
        <Tabs.Trigger value="notebook" class="tab">Notebook</Tabs.Trigger>
      </Tabs.List>

      {#if ws.error}
        <div class="flex items-start gap-3 border-b border-failed/40 bg-failed-wash px-4 py-2 text-sm" role="alert">
          <p class="flex-1"><span class="font-semibold text-failed">{ws.error.action} failed.</span> {ws.error.message}</p>
          <button class="btn btn-quiet !min-h-6 !px-1" aria-label="Dismiss" onclick={() => (ws.error = null)}><X size={14} /></button>
        </div>
      {/if}

      <Tabs.Content value="step" class="min-h-0 flex-1">
        <div bind:this={centre} class="grid h-full min-h-0" style:grid-template-rows="{split}fr 6px {1 - split}fr">
          <div class="min-h-0 overflow-hidden bg-panel">
            {#if ws.selectedStep}
              <StepEditor {ws} step={ws.selectedStep} />
            {:else}
              <SourceOverview {ws} />
            {/if}
          </div>
          <button
            class="cursor-row-resize border-y border-rule bg-paper hover:bg-select"
            class:bg-select={dragging}
            aria-label="Resize editor and result (arrow keys)"
            onpointerdown={() => (dragging = true)}
            onkeydown={(e) => {
              if (e.key === "ArrowUp") split = Math.max(0.15, split - 0.05);
              if (e.key === "ArrowDown") split = Math.min(0.8, split + 0.05);
            }}
          ></button>
          <div class="min-h-0">
            <ResultPanel {ws} />
          </div>
        </div>
      </Tabs.Content>
      <Tabs.Content value="notebook" class="min-h-0 flex-1 overflow-y-auto">
        <Notebook {ws} />
      </Tabs.Content>
    </Tabs.Root>
  </div>
</div>
