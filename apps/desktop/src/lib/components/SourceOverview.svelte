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

  // Renaming: the id custom SQL and Python use for this source.
  let name = $state("");
  let sheets = $state<string[]>([]);
  $effect.pre(() => {
    name = source.id;
  });
  $effect(() => {
    const id = source.id;
    void ws.revision;
    if (source.format !== "excel") return void (sheets = []);
    ws.backend.sourceSheets(id).then((s) => (sheets = s), () => (sheets = []));
  });
  let valid = $derived(/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) && name.toLowerCase() !== "input");
  const formatNames = { csv: "CSV", parquet: "Parquet", json: "JSON", excel: "Spreadsheet" };
  let several = $derived((ws.view?.sources.length ?? 0) > 1);
  let withMissing = $derived(profile?.filter((p) => p.missing > 0).length ?? 0);
</script>

<section class="flex h-full flex-col" aria-label="Source">
  <header class="flex items-start gap-3 border-b border-rule px-5 py-3">
    <div class="min-w-0 flex-1">
    <h2 class="text-lg font-semibold">{source.path}</h2>
    <p class="max-w-[42rem] text-sm text-ink-soft">
      {formatNames[source.format]} file. {formatCount(source.rows)} rows, {source.columns.length} columns.
      {#if profile}
        {withMissing ? `${withMissing} column${withMissing === 1 ? " has" : "s have"} missing values.` : "No missing values."}
      {/if}
      Types were detected on import.
    </p>
    <div class="mt-2 flex flex-wrap items-center gap-x-4 gap-y-2 text-sm">
      <form
        class="flex items-center gap-2"
        onsubmit={(e) => {
          e.preventDefault();
          if (valid && name !== source.id) ws.updateSource(source.id, name, null);
        }}
      >
        <label for="source-name" class="text-ink-soft">Name in SQL and Python</label>
        <input id="source-name" class="field code w-44" bind:value={name} aria-invalid={!valid} spellcheck="false" />
        {#if name !== source.id}
          <button class="btn !min-h-7" disabled={!valid || ws.running}>Rename</button>
        {/if}
      </form>
      {#if sheets.length}
        <label class="flex items-center gap-2">
          <span class="text-ink-soft">Sheet</span>
          <select
            class="field"
            value={source.sheet ?? sheets[0]}
            disabled={ws.running}
            onchange={(e) => ws.updateSource(source.id, null, e.currentTarget.value)}
          >
            {#each sheets as s (s)}<option value={s}>{s}</option>{/each}
          </select>
        </label>
      {/if}
    </div>
    {#if !valid}
      <p class="mt-1 text-sm text-failed">Start with a letter and use only letters, digits and _.</p>
    {:else if name !== source.id && readers}
      <p class="mt-1 max-w-[60ch] text-sm text-ink-soft">
        Steps that read it, and SQL and Python that name it, are updated. Those steps run again.
      </p>
    {/if}
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
