<script lang="ts">
  import { Dialog } from "bits-ui";
  import { CircleCheck, CircleX, Terminal } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { RView } from "../api/types";
  import { errorMessage } from "../api/backend";

  // Which R runs the analysis's R steps, the package libraries it sees, and
  // choosing another installation. Remembered for this user; R steps rerun
  // when it changes (it is part of their cache key).
  let { ws }: { ws: Workspace } = $props();

  let env = $state<RView | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let custom = $state("");

  $effect(() => {
    if (!ws.rOpen) return;
    loading = true;
    error = null;
    ws.backend.rEnvironment().then(
      (e) => ((env = e), (loading = false)),
      (e) => ((error = errorMessage(e)), (loading = false)),
    );
  });

  async function use(path: string) {
    loading = true;
    error = null;
    try {
      env = await ws.backend.setR(path);
      custom = "";
      ws.notice = `R steps now use ${path}. Those that ran with another R are outdated.`;
      ws.view = await ws.backend.view();
      ws.revision++;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      loading = false;
    }
  }

  const why = { env: "set by VDA_R", chosen: "chosen here", default: "Rscript on your PATH" };
</script>

<Dialog.Root bind:open={ws.rOpen}>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-40 bg-ink/35" />
    <Dialog.Content
      class="fixed top-1/2 left-1/2 z-50 flex max-h-[min(42rem,92vh)] w-[min(40rem,94vw)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-rule-strong bg-panel shadow-[0_12px_40px_rgba(29,42,63,0.25)]"
    >
      <header class="flex gap-3 border-b border-rule px-5 pt-4 pb-3">
        <Terminal size={20} class="mt-1 shrink-0 text-ink-soft" aria-hidden="true" />
        <div>
          <Dialog.Title class="text-lg font-bold">R installation</Dialog.Title>
          <Dialog.Description class="mt-1 max-w-[56ch] text-sm text-ink-soft">
            The R that runs R steps, with the package libraries you use in RStudio. Base R is enough; packages are up to
            your code. Changing it reruns R steps, since results can differ between versions.
          </Dialog.Description>
        </div>
      </header>
      <div class="min-h-0 flex-1 overflow-y-auto px-5 py-4 text-sm">
        {#if env}
          <dl class="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-1">
            <dt class="text-ink-soft">Rscript</dt>
            <dd><code class="code break-all">{env.rscript}</code> <span class="text-ink-faint">({why[env.chosen_by]})</span></dd>
            <dt class="text-ink-soft">Version</dt>
            <dd>{env.version ? `R ${env.version}` : "unknown"}</dd>
            <dt class="text-ink-soft">Libraries</dt>
            <dd>
              {#each env.libraries as l (l)}<code class="code block break-all">{l}</code>{:else}<span class="text-ink-faint">none found</span>{/each}
            </dd>
          </dl>
          {#if env.problem}
            <p class="mt-3 rounded-md border border-failed/40 bg-failed-wash px-3 py-2" role="alert">R steps cannot run: {env.problem}</p>
          {/if}
          <table class="mt-4 w-full text-left" aria-label="R packages">
            <thead class="text-xs text-ink-soft"><tr><th class="py-1 font-normal">Package</th><th class="font-normal">For</th><th class="font-normal">Installed</th></tr></thead>
            <tbody>
              {#each env.packages as p (p.name)}
                <tr class="border-t border-rule">
                  <td class="py-1"><code class="code">{p.name}</code></td>
                  <td class="text-ink-soft">{p.purpose}</td>
                  <td>
                    {#if p.version}
                      <span class="flex items-center gap-1"><CircleCheck size={13} class="text-fresh" aria-hidden="true" />{p.version}</span>
                    {:else}
                      <span class="flex items-center gap-1 {p.required ? 'text-failed' : 'text-ink-faint'}"><CircleX size={13} aria-hidden="true" />not installed</span>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
          {#if env.chosen_by === "env"}
            <p class="mt-4 text-ink-soft">The VDA_R environment variable decides which R runs; unset it to choose one here.</p>
          {:else}
            <h3 class="mt-5 mb-1.5 font-semibold">Use another R</h3>
            {#if env.candidates.length}
              <ul class="space-y-1" aria-label="R installations found on this computer">
                {#each env.candidates as c (c)}
                  <li class="flex items-center gap-2">
                    <code class="code min-w-0 flex-1 truncate" title={c}>{c}</code>
                    <button class="btn !min-h-7" disabled={loading} onclick={() => use(c)}>Use</button>
                  </li>
                {/each}
              </ul>
            {/if}
            <form
              class="mt-2 flex items-center gap-2"
              onsubmit={(e) => {
                e.preventDefault();
                if (custom.trim()) use(custom.trim());
              }}
            >
              <input class="field code min-w-0 flex-1" aria-label="Rscript path" placeholder="/opt/R/4.4.1/bin/Rscript" bind:value={custom} spellcheck="false" />
              <button class="btn !min-h-7" disabled={loading || !custom.trim()}>Use this path</button>
            </form>
          {/if}
        {:else if loading}
          <p class="text-ink-soft">Asking R what it has…</p>
        {/if}
        {#if error}<p class="mt-3 text-failed" role="alert">{error}</p>{/if}
      </div>
      <footer class="flex justify-end border-t border-rule px-5 py-3">
        <Dialog.Close class="btn">Done</Dialog.Close>
      </footer>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
