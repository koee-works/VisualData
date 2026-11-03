<script lang="ts">
  import type { Workspace } from "../workspace.svelte";
  import { Trash2 } from "@lucide/svelte";
  import type { ProfileRow, SourceView } from "../api/types";
  import { formatCount } from "../format";
  import ProfileTable from "./ProfileTable.svelte";

  let { ws, source }: { ws: Workspace; source: SourceView } = $props();

  let profile = $state<ProfileRow[] | null>(null);
  $effect(() => {
    void ws.revision;
    const id = source.id;
    profile = null;
    ws.backend.sourceProfile(id).then(
      (p) => id === source.id && (profile = p),
      () => (profile = null),
    );
  });
  let readers = $derived((ws.view?.steps ?? []).filter((s) => s.inputs.includes(source.id)).length);
  let several = $derived((ws.view?.sources.length ?? 0) > 1);
  let withMissing = $derived(profile?.filter((p) => p.missing > 0).length ?? 0);
</script>

<section class="flex h-full flex-col" aria-label="Source">
  <header class="flex items-start gap-3 border-b border-rule px-5 py-3">
    <div class="min-w-0 flex-1">
    <h2 class="text-lg font-semibold">{source.path}</h2>
    <p class="max-w-[42rem] text-sm text-ink-soft">
      {#if several}Custom SQL reads it as <code class="code">{source.id}</code>.{/if}
      {formatCount(source.rows)} rows, {source.columns.length} columns.
      {#if profile}
        {withMissing ? `${withMissing} column${withMissing === 1 ? " has" : "s have"} missing values.` : "No missing values."}
      {/if}
      Types were detected on import.
    </p>
    </div>
    {#if several}
      <button
        class="btn btn-quiet !px-2"
        disabled={readers > 0 || ws.running}
        title={readers ? "Steps read this source; delete them first" : "Remove from the analysis (the file stays in the folder)"}
        aria-label="Remove source"
        onclick={() => ws.removeSource(source.id)}
      >
        <Trash2 size={16} />
      </button>
    {/if}
  </header>
  <div class="flex-1 overflow-auto">
    <ProfileTable {profile} columns={source.columns} />
  </div>
</section>
