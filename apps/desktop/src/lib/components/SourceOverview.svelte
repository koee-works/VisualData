<script lang="ts">
  import type { Workspace } from "../workspace.svelte";
  import type { ProfileRow } from "../api/types";
  import { formatCount } from "../format";
  import ProfileTable from "./ProfileTable.svelte";

  let { ws }: { ws: Workspace } = $props();

  let profile = $state<ProfileRow[] | null>(null);
  $effect(() => {
    void ws.revision;
    ws.backend.sourceProfile().then((p) => (profile = p), () => (profile = null));
  });
  let withMissing = $derived(profile?.filter((p) => p.missing > 0).length ?? 0);
</script>

<section class="flex h-full flex-col" aria-label="Source">
  <header class="border-b border-rule px-5 py-3">
    <h2 class="text-lg font-semibold">{ws.view?.source.path}</h2>
    <p class="max-w-[42rem] text-sm text-ink-soft">
      {formatCount(ws.view?.source.rows ?? 0)} rows, {ws.view?.source.columns.length} columns.
      {#if profile}
        {withMissing ? `${withMissing} column${withMissing === 1 ? " has" : "s have"} missing values.` : "No missing values."}
      {/if}
      Types were detected on import.
    </p>
  </header>
  <div class="flex-1 overflow-auto">
    <ProfileTable {profile} columns={ws.view?.source.columns ?? []} />
  </div>
</section>
