<script lang="ts">
  import { onMount } from "svelte";
  import type { Backend } from "./lib/api/backend";
  import { Workspace } from "./lib/workspace.svelte";
  import { initPrefs } from "./lib/prefs.svelte";
  import Welcome from "./lib/components/Welcome.svelte";
  import WorkspaceView from "./lib/components/WorkspaceView.svelte";

  let ws = $state<Workspace | null>(null);

  onMount(async () => {
    // Inside the desktop app use Tauri IPC; in a plain browser (design work,
    // `npm run dev`) use the in-memory mock.
    const inTauri = "__TAURI_INTERNALS__" in window;
    const backend: Backend = inTauri
      ? (await import("./lib/api/tauri")).tauriBackend
      : new (await import("./lib/api/mock")).MockBackend();
    if (!inTauri) {
      const q = new URLSearchParams(location.search);
      if (q.has("demo")) {
        const mock = backend as import("./lib/api/mock").MockBackend;
        const demo = q.get("demo");
        await mock.seedDemo({
          run: demo !== "fresh-start" && demo !== "review",
          editAfterRun: demo === "stale",
          received: demo === "review",
          join: demo === "join",
        });
      }
    }
    await initPrefs(backend);
    const w = new Workspace(backend);
    const dir = await backend.startupProject().catch(() => null);
    if (dir) await w.open(dir);
    const select = new URLSearchParams(location.search).get("select");
    if (select) w.selected = select;
    if (new URLSearchParams(location.search).get("tab") === "notebook") w.centerTab = "notebook";
    ws = w;
  });
</script>

{#if ws}
  {#if ws.view}
    <WorkspaceView {ws} />
  {:else}
    <Welcome {ws} />
  {/if}
{/if}
