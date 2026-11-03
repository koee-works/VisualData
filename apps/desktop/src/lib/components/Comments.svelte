<script lang="ts">
  import { Check, MessageSquare, RotateCcw, Trash2 } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { Comment } from "../api/types";

  // Comments on one step or source, kept with the project. Replies sit
  // under their comment; resolved threads fold away.
  let { ws, target }: { ws: Workspace; target: string } = $props();

  let threads = $derived(ws.comments.filter((c) => c.target === target && !c.reply_to));
  let open = $derived(threads.filter((c) => !c.resolved));
  let resolved = $derived(threads.filter((c) => c.resolved));
  let showResolved = $state(false);
  let draft = $state("");
  let replying = $state<string | null>(null);
  let reply = $state("");

  const replies = (id: string) => ws.comments.filter((c) => c.reply_to === id);
  const when = (ms: number) => new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

  async function post() {
    if (await ws.addComment(target, draft)) draft = "";
  }

  async function postReply(id: string) {
    if (await ws.addComment(target, reply, id)) {
      reply = "";
      replying = null;
    }
  }
</script>

{#snippet one(c: Comment, isReply: boolean)}
  <div class="group {isReply ? 'mt-2 border-l-2 border-rule pl-3' : ''}">
    <p class="text-xs text-ink-soft"><span class="font-semibold text-ink">{c.author}</span> on <time>{when(c.time_unix_ms)}</time></p>
    <p class="mt-0.5 whitespace-pre-wrap">{c.text}</p>
  </div>
{/snippet}

<section class="mt-8 border-t border-rule pt-4 text-sm" aria-label="Comments">
  <h3 class="mb-2 flex items-center gap-1.5 font-semibold"><MessageSquare size={15} aria-hidden="true" /> Comments</h3>
  {#if open.length}
    <ul class="mb-3 space-y-3">
      {#each open as c (c.id)}
        <li class="rounded-md border border-rule bg-panel px-3 py-2">
          {@render one(c, false)}
          {#each replies(c.id) as r (r.id)}{@render one(r, true)}{/each}
          <div class="mt-2 flex flex-wrap items-center gap-1">
            {#if replying === c.id}
              <form
                class="flex w-full items-start gap-2"
                onsubmit={(e) => {
                  e.preventDefault();
                  postReply(c.id);
                }}
              >
                <textarea class="field min-h-[2.25rem] flex-1" rows="2" aria-label="Reply" bind:value={reply}></textarea>
                <button class="btn !min-h-7" disabled={!reply.trim()}>Reply</button>
                <button type="button" class="btn btn-quiet !min-h-7" onclick={() => (replying = null)}>Cancel</button>
              </form>
            {:else}
              <button class="btn btn-quiet !min-h-7 !px-2" onclick={() => ((replying = c.id), (reply = ""))}>Reply</button>
              <button class="btn btn-quiet !min-h-7 !px-2" onclick={() => ws.resolveComment(c.id, true)}><Check size={13} /> Resolve</button>
              <button class="btn btn-quiet !min-h-7 !px-1.5" aria-label="Delete comment" title="Delete (your comments only)" onclick={() => ws.deleteComment(c.id)}>
                <Trash2 size={13} />
              </button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
  <form
    class="flex items-start gap-2"
    onsubmit={(e) => {
      e.preventDefault();
      post();
    }}
  >
    <textarea
      class="field min-h-[2.25rem] flex-1"
      rows="2"
      aria-label="New comment"
      placeholder={open.length ? "Add a comment" : "A question or a note for whoever opens this project next"}
      bind:value={draft}
      onkeydown={(e) => {
        if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          post();
        }
      }}
    ></textarea>
    <button class="btn" disabled={!draft.trim()}>Comment</button>
  </form>
  {#if resolved.length}
    <button class="mt-2 text-xs text-ink-soft underline underline-offset-2" onclick={() => (showResolved = !showResolved)}>
      {showResolved ? "Hide" : "Show"} {resolved.length} resolved
    </button>
    {#if showResolved}
      <ul class="mt-2 space-y-2 text-ink-soft">
        {#each resolved as c (c.id)}
          <li class="rounded-md border border-rule px-3 py-2">
            {@render one(c, false)}
            {#each replies(c.id) as r (r.id)}{@render one(r, true)}{/each}
            <button class="btn btn-quiet mt-1 !min-h-7 !px-2" onclick={() => ws.resolveComment(c.id, false)}><RotateCcw size={13} /> Reopen</button>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>
