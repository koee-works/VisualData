<script lang="ts">
  import type { Workspace } from "../workspace.svelte";

  let { ws }: { ws: Workspace } = $props();

  let csv = $state<string | null>(null);
  let folder = $state<string | null>(null);
  let name = $state("");

  async function pickCsv() {
    csv = await ws.backend.pickCsv();
    if (csv && !name) name = csv.split(/[\\/]/).pop()!.replace(/\.csv$/i, "").replace(/[_-]+/g, " ");
  }
  async function openExisting() {
    const dir = await ws.backend.pickFolder();
    if (dir) await ws.open(dir);
  }
  async function create() {
    if (csv && folder) await ws.create(folder, csv, name.trim() || "Untitled analysis");
  }
</script>

<main class="flex h-full items-center justify-center p-8">
  <div class="w-full max-w-xl">
    <h1 class="text-xl font-bold">Start an analysis</h1>
    <p class="mt-1 text-ink-soft">
      An analysis is a folder holding your data, its steps, and their saved results. Nothing leaves this computer.
    </p>

    <section class="mt-6 rounded-lg border border-rule bg-panel p-5">
      <h2 class="font-semibold">From a CSV file</h2>
      <div class="mt-3 grid grid-cols-[7rem_1fr] items-center gap-x-3 gap-y-2.5">
        <span class="text-sm text-ink-soft">Data</span>
        <button class="btn justify-start truncate" onclick={pickCsv}>{csv ?? "Choose a CSV file…"}</button>
        <span class="text-sm text-ink-soft">Save in</span>
        <button class="btn justify-start truncate" onclick={async () => (folder = await ws.backend.pickFolder())}>
          {folder ?? "Choose an empty folder…"}
        </button>
        <label for="new-name" class="text-sm text-ink-soft">Name</label>
        <input id="new-name" class="field" bind:value={name} placeholder="Untitled analysis" />
      </div>
      <button class="btn btn-primary mt-4" disabled={!csv || !folder} onclick={create}>Create analysis</button>
    </section>

    <p class="mt-4 text-sm text-ink-soft">
      Already have one? <button class="font-semibold text-ink underline underline-offset-2" onclick={openExisting}>Open an analysis folder</button>
    </p>

    {#if ws.error}
      <p class="mt-4 rounded-md border border-failed/40 bg-failed-wash px-3 py-2 text-sm" role="alert">
        <span class="font-semibold text-failed">{ws.error.action} failed.</span>
        {ws.error.message}
      </p>
    {/if}
  </div>
</main>
