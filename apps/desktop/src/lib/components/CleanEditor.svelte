<script lang="ts">
  import { FlaskConical, Plus, Trash2, X, LoaderCircle, Check } from "@lucide/svelte";
  import type { CleanColumn, CleanReport, Column, ColumnDetail, Comparison, Fill, FillMethod, Parameter } from "../api/types";
  import { editorParameters } from "../editor/complete";
  import { formatCount, isNumericType } from "../format";
  import CodeArea from "./CodeArea.svelte";
  import ColumnChips from "./ColumnChips.svelte";
  import OrderByList from "./OrderByList.svelte";

  // Find missing values (empty cells, values that stand for missing, rows a
  // formula picks out) and fill them, each column its own way; test the ways
  // by hiding known values and filling them.
  let {
    markers = $bindable(),
    columns = $bindable(),
    flag = $bindable(),
    seed = $bindable(),
    inputColumns,
    details,
    parameters,
    report,
    compare,
  }: {
    markers: string[];
    columns: CleanColumn[];
    flag: boolean;
    seed: number;
    inputColumns: Column[] | null;
    /** The input's columns with missing counts and common values, if it has run. */
    details: ColumnDetail[] | null;
    parameters: Parameter[];
    /** The last run's report, while the step is unchanged. */
    report: CleanReport | null;
    compare: (column: string) => Promise<Comparison>;
  } = $props();

  const methods: { method: FillMethod; name: string; hint: string; numbers?: boolean; grouped?: boolean }[] = [
    { method: "keep", name: "Leave missing", hint: "Only find them (and flag them)" },
    { method: "mean", name: "Mean", hint: "The average of the known values", numbers: true, grouped: true },
    { method: "median", name: "Median", hint: "The middle known value; steadier with outliers", numbers: true, grouped: true },
    { method: "mode", name: "Most common value", hint: "Ties go to the smallest", grouped: true },
    { method: "constant", name: "A fixed value", hint: "The same value everywhere" },
    { method: "random", name: "Random known value", hint: "Keeps the column's spread; repeatable", grouped: true },
    { method: "previous", name: "Previous known value", hint: "Carry the last value forward", grouped: true },
    { method: "next", name: "Next known value", hint: "Carry the next value back", grouped: true },
    { method: "regression", name: "Regression", hint: "A line fitted to other number columns", numbers: true },
    { method: "neighbours", name: "Nearest neighbours", hint: "From the most similar rows" },
    { method: "drop", name: "Drop the row", hint: "Remove rows where it is missing" },
  ];

  // Values that often stand for "missing" in exported data.
  const LIKELY = /^(n\/?a|na|nan|null|none|nil|missing|unknown|-|--|\?|\.|#n\/a|-?9{3,}|-1)$/i;

  const typeOf = (name: string) => inputColumns?.find((c) => c.name === name)?.type ?? null;
  const outType = (c: CleanColumn) => c.as_type ?? typeOf(c.column);
  const numericOut = (c: CleanColumn) => isNumericType(outType(c) ?? "VARCHAR");
  const numberColumns = (except: string) => (inputColumns ?? []).filter((c) => c.name !== except && isNumericType(c.type));
  const detail = (name: string) => details?.find((d) => d.name === name) ?? null;
  const normalized = (s: string) => s.trim().toLowerCase();

  /** Missing cells and marker matches among a column's common values. */
  function gaps(name: string) {
    const d = detail(name);
    if (!d) return null;
    const marked = d.top.filter((t) => markers.some((m) => normalized(m) === normalized(t.value))).reduce((a, t) => a + t.count, 0);
    return { empty: d.missing + (markers.includes("") ? (d.blank ?? 0) : 0), marked, rows: d.rows };
  }

  /** Common values that look like markers and aren't ones yet. */
  let suggestions = $derived.by(() => {
    const out = new Map<string, { value: string; count: number; columns: string[] }>();
    for (const d of details ?? []) {
      for (const t of d.top) {
        if (!LIKELY.test(t.value.trim()) || markers.some((m) => normalized(m) === normalized(t.value))) continue;
        const e = out.get(t.value) ?? { value: t.value, count: 0, columns: [] };
        e.count += t.count;
        e.columns.push(d.name);
        out.set(t.value, e);
      }
      if (d.type === "VARCHAR" && (d.blank ?? 0) > 0 && !markers.includes("")) {
        const e = out.get("") ?? { value: "", count: 0, columns: [] };
        e.count += d.blank ?? 0;
        e.columns.push(d.name);
        out.set("", e);
      }
    }
    return [...out.values()].sort((a, b) => b.count - a.count).slice(0, 6);
  });

  /** Text whose common values are mostly numbers: probably a number column with markers in it. */
  function looksNumeric(name: string): boolean {
    const d = detail(name);
    if (!d || d.type !== "VARCHAR" || !d.top.length) return false;
    // Values that are or look like markers don't count against it.
    const rest = d.top.filter((t) => !markers.some((m) => normalized(m) === normalized(t.value)) && !LIKELY.test(t.value.trim()) && t.value.trim() !== "");
    return rest.length > 0 && rest.every((t) => /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(t.value.trim()));
  }

  let newMarker = $state("");
  function addMarker(v: string) {
    if (!markers.some((m) => m === v)) markers = [...markers, v];
    newMarker = "";
  }

  function defaultFill(method: FillMethod, c: CleanColumn): Fill {
    const predictors = numberColumns(c.column)
      .filter((x) => !/(^|_)id$/i.test(x.name))
      .slice(0, 3)
      .map((x) => x.name);
    switch (method) {
      case "constant":
        return { method, value: numericOut(c) ? "0" : "unknown" };
      case "previous":
      case "next":
        return { method, order_by: [] };
      case "regression":
        return { method, predictors };
      case "neighbours":
        return { method, predictors, k: 5 };
      default:
        return { method } as Fill;
    }
  }

  function addColumn(name: string) {
    if (columns.some((c) => c.column === name)) return;
    const c: CleanColumn = { column: name, as_type: looksNumeric(name) ? "DOUBLE" : null, missing_when: null, group_by: [], fill: { method: "keep" } };
    c.fill = defaultFill(numericOut(c) ? "median" : "mode", c);
    columns = [...columns, c];
  }

  function setMethod(i: number, method: FillMethod) {
    const c = columns[i]!;
    c.fill = defaultFill(method, c);
    if (!methods.find((m) => m.method === method)?.grouped) c.group_by = [];
  }

  function setType(i: number, t: string) {
    const c = columns[i]!;
    c.as_type = t ? (t as CleanColumn["as_type"]) : null;
    // A fill for numbers no longer fits text.
    if (!numericOut(c) && ["mean", "median", "regression"].includes(c.fill.method)) c.fill = defaultFill("mode", c);
  }

  let available = $derived(
    (inputColumns ?? [])
      .map((c) => ({ ...c, gaps: gaps(c.name) }))
      .sort((a, b) => (b.gaps ? b.gaps.empty + b.gaps.marked : 0) - (a.gaps ? a.gaps.empty + a.gaps.marked : 0)),
  );

  // Testing methods on hidden known values.
  let tests = $state<Record<string, { busy: boolean; result: Comparison | null; error: string | null }>>({});
  async function test(column: string) {
    tests[column] = { busy: true, result: null, error: null };
    try {
      tests[column] = { busy: false, result: await compare(column), error: null };
    } catch (e) {
      tests[column] = { busy: false, result: null, error: (e as { message?: string }).message ?? String(e) };
    }
  }
  const pct = (x: number | null) => (x === null ? "—" : `${Math.round(x * 100)}%`);
  const num = (x: number | null) => (x === null ? "—" : Math.abs(x) >= 100 ? x.toFixed(0) : x.toPrecision(3));
  function best(r: Comparison): number {
    let at = -1;
    r.methods.forEach((m, i) => {
      const b = at >= 0 ? r.methods[at]! : null;
      if (m.problem || !m.filled) return;
      if (!b) at = i;
      else if (m.mae !== null && b.mae !== null ? m.mae < b.mae : (m.accuracy ?? 0) > (b.accuracy ?? 0)) at = i;
    });
    return at;
  }
  const same = (a: Fill, b: Fill) => JSON.stringify(a) === JSON.stringify(b);

  function reportFor(name: string) {
    return report?.columns.find((c) => c.column === name) ?? null;
  }
  function found(r: NonNullable<ReturnType<typeof reportFor>>) {
    const parts = [
      [r.empty, "empty"],
      [r.markers, "marked"],
      [r.condition, "by the condition"],
      [r.conversion, "didn't convert"],
    ].filter(([n]) => (n as number) > 0);
    const total = r.empty + r.markers + r.condition + r.conversion;
    return total ? `${formatCount(total)} missing (${parts.map(([n, w]) => `${formatCount(n as number)} ${w}`).join(", ")})` : "Nothing missing";
  }
</script>

<div class="space-y-6 text-sm">
  <fieldset>
    <legend class="mb-1 font-semibold">Values that mean missing</legend>
    <p class="mb-2 max-w-[64ch] text-ink-soft">Empty cells always count. These count too, in every column below; text matches without case or surrounding spaces.</p>
    <div class="flex flex-wrap items-center gap-1.5" aria-label="Markers">
      {#each markers as m, i (i)}
        <span class="inline-flex items-center gap-1 rounded border border-rule-strong bg-panel py-0.5 pr-0.5 pl-2">
          <code class="code text-xs">{m === "" ? "(blank text)" : m}</code>
          <button class="btn btn-quiet !min-h-5 !min-w-5 justify-center !px-0.5" aria-label="Remove {m === '' ? 'blank text' : m}" onclick={() => (markers = markers.filter((_, j) => j !== i))}><X size={12} /></button>
        </span>
      {/each}
      <input
        class="field w-36 text-xs"
        placeholder="Add, e.g. -999"
        aria-label="Add a value that means missing"
        bind:value={newMarker}
        onkeydown={(e) => {
          if (e.key === "Enter" && newMarker.trim()) {
            e.preventDefault();
            addMarker(newMarker.trim());
          }
        }}
      />
    </div>
    {#if suggestions.length}
      <p class="mt-2 flex flex-wrap items-center gap-1.5 text-xs text-ink-soft">
        Found in the data:
        {#each suggestions as s (s.value)}
          <button class="btn !min-h-6 !px-2 !py-0 text-xs" title="In {s.columns.join(', ')}" onclick={() => addMarker(s.value)}>
            <Plus size={12} /> {s.value === "" ? "blank text" : `“${s.value}”`} <span class="text-ink-faint">{formatCount(s.count)}</span>
          </button>
        {/each}
      </p>
    {/if}
  </fieldset>

  <fieldset>
    <legend class="mb-1.5 font-semibold">Columns</legend>
    {#if details}
      <table class="w-full max-w-xl border-collapse text-left" aria-label="Missing values by column">
        <thead class="text-xs text-ink-faint">
          <tr><th class="py-1 font-normal">Column</th><th class="py-1 text-right font-normal">Empty</th><th class="py-1 text-right font-normal">Markers</th><th></th></tr>
        </thead>
        <tbody>
          {#each available as c (c.name)}
            {@const g = c.gaps}
            {@const cleaning = columns.some((x) => x.column === c.name)}
            <tr class="border-t border-rule" class:text-ink-faint={!!g && !g.empty && !g.marked && !cleaning}>
              <td class="py-1">{c.name} <span class="text-xs text-ink-faint">{c.type.toLowerCase()}</span>{#if looksNumeric(c.name)}{" "}<span class="ml-1 text-xs text-stale">numbers stored as text</span>{/if}</td>
              <td class="py-1 text-right tabular-nums">{g ? formatCount(g.empty) : "—"}</td>
              <td class="py-1 text-right tabular-nums">{g ? formatCount(g.marked) : "—"}</td>
              <td class="py-1 pl-3 text-right">
                {#if cleaning}
                  <span class="inline-flex items-center gap-1 text-xs text-fresh"><Check size={13} /> Cleaning</span>
                {:else}
                  <button class="btn btn-quiet !min-h-6 !px-2 !py-0 text-xs" aria-label="Clean {c.name}" onclick={() => addColumn(c.name)}><Plus size={13} /> Clean</button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="mt-1 text-xs text-ink-faint">Markers are counted among each column's most common values; the run counts every one.</p>
    {:else}
      <div class="flex flex-wrap gap-1.5">
        {#each inputColumns ?? [] as c (c.name)}
          {#if !columns.some((x) => x.column === c.name)}
            <button class="btn !min-h-7 !px-2 text-xs" onclick={() => addColumn(c.name)}><Plus size={13} /> {c.name}</button>
          {/if}
        {/each}
      </div>
      <p class="mt-1 text-xs text-ink-faint">Run the analysis to see how many values each column is missing.</p>
    {/if}
  </fieldset>

  {#each columns as c, i (c.column)}
    {@const m = methods.find((x) => x.method === c.fill.method)}
    {@const r = reportFor(c.column)}
    {@const t = tests[c.column]}
    <section class="rounded-md border border-rule" aria-label="Clean {c.column}">
      <header class="flex flex-wrap items-center gap-2 border-b border-rule px-3 py-2">
        <h3 class="font-semibold">{c.column}</h3>
        {#if typeOf(c.column) === "VARCHAR"}
          <label class="flex items-center gap-1.5 text-xs text-ink-soft">
            read as
            <select class="field !min-h-7 text-xs" aria-label="Read {c.column} as" value={c.as_type ?? ""} onchange={(e) => setType(i, e.currentTarget.value)}>
              <option value="">text</option>
              <option value="DOUBLE">numbers</option>
              <option value="BIGINT">whole numbers</option>
              <option value="DATE">dates</option>
            </select>
          </label>
        {:else}
          <span class="text-xs text-ink-faint">{typeOf(c.column)?.toLowerCase()}</span>
        {/if}
        <button class="btn btn-quiet ml-auto !min-h-7 !min-w-7 justify-center !px-1.5" aria-label="Stop cleaning {c.column}" onclick={() => (columns = columns.filter((_, j) => j !== i))}><Trash2 size={14} /></button>
      </header>

      <div class="space-y-3 px-3 py-3">
        <div>
          <span class="mb-1 block text-xs font-semibold text-ink-soft">Also missing when</span>
          <CodeArea
            bind:value={() => c.missing_when ?? "", (v: string) => (c.missing_when = v.trim() ? v : null)}
            lang="formula"
            label="Also missing when, for {c.column}"
            placeholder={numericOut(c) ? `For example: [${c.column}] < 0` : `For example: [${c.column}] = "?"`}
            singleLine
            context={{ inputs: [{ id: "input", name: "the input", columns: inputColumns }], parameters: editorParameters(parameters) }}
            problems={[]}
          />
        </div>

        <div class="flex flex-wrap items-center gap-2">
          <label class="flex items-center gap-2">
            <span class="font-semibold">Fill with</span>
            <select class="field" aria-label="Fill {c.column} with" value={c.fill.method} onchange={(e) => setMethod(i, e.currentTarget.value as FillMethod)}>
              {#each methods as x (x.method)}
                <option value={x.method} disabled={x.numbers && !numericOut(c)}>{x.name}{x.numbers && !numericOut(c) ? " (numbers only)" : ""}</option>
              {/each}
            </select>
          </label>
          {#if c.fill.method === "constant"}
            <input class="field w-40" aria-label="Fixed value for {c.column}" bind:value={c.fill.value} />
          {/if}
          {#if c.fill.method === "neighbours"}
            <label class="flex items-center gap-1.5 text-ink-soft">
              <input class="field w-16 tabular-nums" type="number" min="1" max="50" aria-label="Neighbours" bind:value={c.fill.k} /> most similar rows
            </label>
          {/if}
          <span class="text-xs text-ink-faint">{m?.hint}</span>
        </div>

        {#if c.fill.method === "regression" || c.fill.method === "neighbours"}
          <fieldset>
            <legend class="mb-1 text-xs font-semibold text-ink-soft">{c.fill.method === "regression" ? "Predict from" : "Compare rows by"}</legend>
            <ColumnChips bind:value={c.fill.predictors} columns={numberColumns(c.column)} label="Predictors for {c.column}" />
            <p class="mt-1 max-w-[64ch] text-xs text-ink-faint">
              {c.fill.method === "regression"
                ? "Fitted on rows where every one of these is known; a row missing one stays missing."
                : "Distances use each column scaled to its spread. Fills at most 20,000 values."}
            </p>
          </fieldset>
        {/if}
        {#if c.fill.method === "previous" || c.fill.method === "next"}
          <OrderByList
            bind:by={c.fill.order_by}
            columns={inputColumns}
            legend="In order of"
            addLabel={["Choose an order", "Then by"]}
            note={c.fill.order_by.length ? "Ties keep row order." : "Nothing chosen: the rows' current order."}
          />
        {/if}
        {#if m?.grouped}
          <fieldset>
            <legend class="mb-1 text-xs font-semibold text-ink-soft">Within groups of</legend>
            <ColumnChips bind:value={c.group_by} columns={(inputColumns ?? []).filter((x) => x.name !== c.column)} label="Groups for {c.column}" />
          </fieldset>
        {/if}

        {#if r}
          <p class="rounded bg-paper px-2 py-1.5 text-xs" aria-label="Last run of {c.column}">
            {found(r)}{#if r.method === "drop" && r.empty + r.markers + r.condition + r.conversion > 0}; those rows dropped.{:else if r.filled}; filled {formatCount(r.filled)} with {r.detail}{r.left ? `, ${formatCount(r.left)} still missing` : ""}.{:else}.{/if}
          </p>
        {/if}

        <div>
          <button class="btn !min-h-7 !px-2 text-xs" disabled={t?.busy} onclick={() => test(c.column)}>
            {#if t?.busy}<LoaderCircle size={13} class="animate-spin" />{:else}<FlaskConical size={13} />{/if}
            Compare methods
          </button>
          {#if t?.error}<p class="mt-1 text-xs text-failed" role="alert">{t.error}</p>{/if}
          {#if t?.result}
            {@const res = t.result}
            {@const top = best(res)}
            {@const isNum = res.methods.some((x) => x.mae !== null)}
            <p class="mt-2 max-w-[70ch] text-xs text-ink-soft">
              Hid {formatCount(res.hidden)} of the {formatCount(res.known)} known values in the first {formatCount(res.rows)} rows, filled them each way and compared with the real values.{#if res.predictors.length}{" "}Regression and neighbours use {res.predictors.join(", ")}.{/if}
            </p>
            <table class="mt-1 w-full max-w-2xl border-collapse text-left text-xs" aria-label="Methods for {c.column}">
              <thead class="text-ink-faint">
                <tr>
                  <th class="py-1 font-normal">Method</th>
                  <th class="py-1 text-right font-normal">Filled</th>
                  {#if isNum}
                    <th class="py-1 text-right font-normal" title="The average distance from the real value">Typical error</th>
                    <th class="py-1 text-right font-normal" title="Root mean square error: punishes big misses more">RMSE</th>
                  {:else}
                    <th class="py-1 text-right font-normal">Right</th>
                  {/if}
                  <th class="py-1 text-right font-normal" title="How much of the real values' variety the filled values keep. One value for every gap keeps none.">Keeps spread</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {#each res.methods as x, k (x.label)}
                  <tr class="border-t border-rule" class:bg-fresh-wash={k === top}>
                    <td class="py-1">{x.label}{#if k === top}{" "}<span class="ml-1 font-semibold text-fresh">closest</span>{/if}</td>
                    {#if x.problem}
                      <td colspan={isNum ? 4 : 3} class="py-1 text-ink-faint">{x.problem}</td>
                    {:else}
                      <td class="py-1 text-right tabular-nums">{formatCount(x.filled)}</td>
                      {#if isNum}
                        <td class="py-1 text-right tabular-nums">{num(x.mae)}</td>
                        <td class="py-1 text-right tabular-nums">{num(x.rmse)}</td>
                      {:else}
                        <td class="py-1 text-right tabular-nums">{pct(x.accuracy)}</td>
                      {/if}
                      <td class="py-1 text-right tabular-nums">{pct(x.spread)}</td>
                    {/if}
                    <td class="py-1 pl-2 text-right">
                      {#if same(x.fill, c.fill)}
                        <span class="text-ink-faint">in use</span>
                      {:else if !x.problem}
                        <button class="btn btn-quiet !min-h-6 !px-1.5 !py-0" aria-label="Use {x.label} for {c.column}" onclick={() => (c.fill = structuredClone($state.snapshot(x.fill)))}>Use</button>
                      {/if}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
            <p class="mt-1 max-w-[70ch] text-xs text-ink-faint">
              Lower error is closer; a method that fills every gap with one value can score well yet flatten the column, which the spread shows.
            </p>
          {/if}
        </div>
      </div>
    </section>
  {/each}

  <fieldset class="space-y-2">
    <legend class="mb-1 font-semibold">Options</legend>
    <label class="flex items-start gap-2">
      <input type="checkbox" class="mt-0.5" bind:checked={flag} />
      <span>Add a column saying which values were missing <span class="text-ink-faint">({columns.map((c) => `${c.column}_missing`).join(", ") || "one per column"})</span></span>
    </label>
    <label class="flex items-center gap-2 text-ink-soft">
      Random draws use seed <input class="field w-20 tabular-nums" type="number" min="0" aria-label="Seed" bind:value={seed} />
      <span class="text-xs text-ink-faint">The same seed gives the same draws every run.</span>
    </label>
  </fieldset>
</div>
