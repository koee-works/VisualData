<script lang="ts">
  import { Dialog } from "bits-ui";
  import { ShieldAlert } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { ReviewItem } from "../api/types";
  import { describe, stepTitle } from "../format";

  // Python steps run as the user, so code the user did not write in this
  // app is shown in full and runs only once approved (ADR-0004).
  let { ws }: { ws: Workspace } = $props();

  let items = $derived(ws.pendingReview);
  // "Python", "R", or "Python and R".
  let langs = $derived(
    [...new Set(items.map((i) => (i.language === "r" ? "R" : "Python")))].sort((a, b) => (a === "Python" ? -1 : b === "Python" ? 1 : 0)).join(" and ") || "Python",
  );
  let busy = $state(false);

  function title(item: ReviewItem): string {
    const step = ws.view?.steps.find((s) => s.id === item.step_id);
    return step ? stepTitle(step) : item.label || describe({ type: item.language === "r" ? "r" : "python", code: item.code, timeout_ms: 0 });
  }

  function inputName(item: ReviewItem): string {
    return item.inputs.map((i) => ws.nameOf(i)).join(" and ");
  }

  function lines(code: string): number {
    return code.split("\n").length;
  }

  async function approve(ids: string[]) {
    busy = true;
    await ws.approve(ids);
    busy = false;
  }
</script>

<Dialog.Root bind:open={ws.reviewOpen}>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-40 bg-black/40" />
    <Dialog.Content
      class="fixed top-1/2 left-1/2 z-50 flex max-h-[min(44rem,92vh)] w-[min(46rem,94vw)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-rule-strong bg-panel shadow-[var(--shadow-pop)]"
    >
      <header class="flex gap-3 border-b border-rule px-5 pt-4 pb-3">
        <ShieldAlert size={22} class="mt-0.5 shrink-0 text-review" aria-hidden="true" />
        <div>
          <Dialog.Title class="text-lg font-bold">
            Review {items.length === 1 ? `this ${langs} step` : `${items.length} ${langs} steps`} before {items.length === 1
              ? "it runs"
              : "they run"}
          </Dialog.Title>
          <Dialog.Description class="mt-1 max-w-[62ch] text-sm text-ink-soft">
            This project contains {langs} code you haven't approved on this computer. Code steps run as you: they can
            read and change your files and use the network. Approve only code you understand or trust. Until then, these
            steps and the steps after them won't run. <em>Trust all until I quit</em> skips this check in every project
            you open until you close the app; nothing is saved.
          </Dialog.Description>
        </div>
      </header>

      <ol class="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4" aria-label="Code to review">
        {#each items as item (item.step_id)}
          <li class="rounded-md border border-review/40">
            <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1 border-b border-review/25 bg-review-wash px-3 py-2">
              <p class="font-semibold">{title(item)}</p>
              <p class="text-xs text-ink-soft">
                Reads {inputName(item)}, {lines(item.code)} line{lines(item.code) === 1 ? "" : "s"}
              </p>
              {#if items.length > 1}
                <button class="btn ml-auto !min-h-7" disabled={busy} onclick={() => approve([item.step_id])}>
                  Approve this step
                </button>
              {/if}
            </div>
            <pre class="code max-h-64 overflow-auto px-3 py-2 text-[0.8125rem] leading-relaxed"><code>{item.code}</code></pre>
          </li>
        {/each}
      </ol>

      <!-- The note on its own line, so the actions stay on one row with
           the main one on the right. -->
      <p class="border-t border-rule px-5 pt-3 text-xs text-ink-faint">
        SQL and visual steps don't need approval. They can only read data inside the project folder.
      </p>
      <footer class="flex flex-wrap items-center justify-end gap-2 px-5 pt-2 pb-3">
        <Dialog.Close class="btn btn-quiet">Not now</Dialog.Close>
        <button
          class="btn"
          disabled={busy}
          title="Runs code steps in every project you open until you quit, without asking. Nothing is saved; you'll be asked again next time."
          onclick={() => ws.setTrustAll(true)}
        >
          Trust all until I quit
        </button>
        <button class="btn btn-primary" disabled={busy || !items.length} onclick={() => approve(items.map((i) => i.step_id))}>
          {items.length > 1 ? `Approve all ${items.length}` : "Approve"}
        </button>
      </footer>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
