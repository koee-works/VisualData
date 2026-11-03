<script lang="ts">
  import { Dialog } from "bits-ui";
  import { CalendarClock, Copy } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { Every, ScheduleView } from "../api/types";
  import { errorMessage } from "../api/backend";

  // A refresh runs without the app (vda-run), started by the computer's own
  // scheduler. This writes the entry; the user adds it, so nothing is
  // installed behind their back.
  let { ws }: { ws: Workspace } = $props();

  let every = $state<Every>("weekdays");
  let time = $state("07:00");
  let exportStep = $state("");
  let exportPath = $state("");
  let result = $state<ScheduleView | null>(null);
  let error = $state<string | null>(null);
  let copied = $state(false);

  let tables = $derived((ws.view?.steps ?? []).filter((s) => s.mode !== "chart"));

  $effect(() => {
    if (!ws.scheduleOpen) return;
    const [h, m] = time.split(":").map(Number);
    const exports: [string, string][] = exportStep && exportPath.trim() ? [[exportStep, exportPath.trim()]] : [];
    copied = false;
    ws.backend.schedule(every, h ?? 7, m ?? 0, exports).then(
      (r) => ((result = r), (error = null)),
      (e) => ((result = null), (error = errorMessage(e))),
    );
  });

  async function copy() {
    if (!result) return;
    try {
      await navigator.clipboard.writeText(result.entry);
      copied = true;
    } catch {
      copied = false;
    }
  }
</script>

<Dialog.Root bind:open={ws.scheduleOpen}>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-40 bg-ink/35" />
    <Dialog.Content
      class="fixed top-1/2 left-1/2 z-50 flex max-h-[min(44rem,92vh)] w-[min(44rem,94vw)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-rule-strong bg-panel shadow-[0_12px_40px_rgba(29,42,63,0.25)]"
    >
      <header class="flex gap-3 border-b border-rule px-5 pt-4 pb-3">
        <CalendarClock size={20} class="mt-1 shrink-0 text-ink-soft" aria-hidden="true" />
        <div>
          <Dialog.Title class="text-lg font-bold">Refresh on a schedule</Dialog.Title>
          <Dialog.Description class="mt-1 max-w-[58ch] text-sm text-ink-soft">
            <code class="code">vda-run</code> refreshes this project without the app: it re-reads the sources, reruns what
            changed and can write a file. Python code runs only if you have approved it. Scheduled runs appear in the run log.
          </Dialog.Description>
        </div>
      </header>
      <div class="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4 text-sm">
        <div class="flex flex-wrap items-center gap-2">
          <label class="flex items-center gap-2">
            <span class="text-ink-soft">Run</span>
            <select class="field" aria-label="How often" bind:value={every}>
              <option value="hourly">every hour</option>
              <option value="daily">every day</option>
              <option value="weekdays">every weekday</option>
              <option value="weekly">every Monday</option>
            </select>
          </label>
          {#if every !== "hourly"}
            <label class="flex items-center gap-2">
              <span class="text-ink-soft">at</span>
              <input class="field" type="time" aria-label="Time" bind:value={time} />
            </label>
          {/if}
        </div>
        <div class="flex flex-wrap items-center gap-2">
          <span class="text-ink-soft">Then save</span>
          <select class="field" aria-label="Result to save" bind:value={exportStep}>
            <option value="">nothing</option>
            {#each tables as s (s.id)}<option value={s.id}>{ws.nameOf(s.id)}</option>{/each}
          </select>
          {#if exportStep}
            <span class="text-ink-soft">to</span>
            <input class="field code min-w-0 flex-1" aria-label="File to save" placeholder="/reports/latest.xlsx" bind:value={exportPath} spellcheck="false" />
          {/if}
        </div>
        {#if exportStep}<p class="text-xs text-ink-faint">.csv, .xlsx or .parquet. Written only when every step succeeds, so it never mixes old and new results.</p>{/if}
        {#if result}
          <div>
            <p class="mb-1 font-semibold">{result.how}</p>
            <pre class="code max-h-52 overflow-auto rounded-md border border-rule bg-paper px-3 py-2 whitespace-pre-wrap break-all" aria-label="Schedule entry">{result.entry}</pre>
            <div class="mt-2 flex items-center gap-2">
              <button class="btn" onclick={copy}><Copy size={14} /> {copied ? "Copied" : "Copy"}</button>
              <span class="text-xs text-ink-faint">To run it once now: <code class="code break-all">{result.command}</code></span>
            </div>
          </div>
        {/if}
        {#if error}<p class="text-failed" role="alert">{error}</p>{/if}
      </div>
      <footer class="flex justify-end border-t border-rule px-5 py-3">
        <Dialog.Close class="btn">Done</Dialog.Close>
      </footer>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
