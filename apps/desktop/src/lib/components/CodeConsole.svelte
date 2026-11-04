<script lang="ts">
  import { tick } from "svelte";
  import { CircleStop, RotateCcw, Eraser, Play } from "@lucide/svelte";
  import type { Workspace } from "../workspace.svelte";
  import type { ConsoleObject, ConsoleReply, ConsoleView } from "../api/types";
  import type { EditorContext } from "../editor/complete";
  import { errorMessage } from "../api/backend";
  import CodeArea from "./CodeArea.svelte";

  // A console beside a Python or R step, like RStudio's or a notebook's:
  // the step's inputs stay loaded, so trying a line is instant. The step
  // itself still runs clean when saved; afterwards the console's `result` is
  // compared with the step's, to catch state only the console has.
  let {
    ws,
    stepId,
    language,
    context,
    ongoto,
  }: {
    ws: Workspace;
    stepId: string;
    language: "python" | "r";
    context: EditorContext;
    /** Show a line of the step's code (from an error). */
    ongoto?: (line: number) => void;
  } = $props();

  type Entry =
    | { kind: "run"; code: string; fromLine: number | null; reply: ConsoleReply | null; failed?: string }
    | { kind: "note"; text: string; tone: "info" | "warn" | "error"; action?: "run" };

  let status = $state<"off" | "starting" | "ready" | "running">("off");
  let info = $state<ConsoleView | null>(null);
  let objects = $state<ConsoleObject[]>([]);
  let entries = $state<Entry[]>([]);
  let input = $state("");
  let history: string[] = [];
  let recall = -1;
  let log: HTMLDivElement | undefined = $state();
  let startedFor = "";

  const name = $derived(language === "r" ? "R" : "Python");

  // Another step, or another language: the console belongs to one step.
  $effect(() => {
    const key = `${stepId}:${language}`;
    if (startedFor && startedFor !== key) {
      void stop();
    }
  });

  async function scroll() {
    await tick();
    log?.scrollTo({ top: log.scrollHeight });
  }

  async function start(): Promise<boolean> {
    status = "starting";
    try {
      info = await ws.backend.consoleStart(stepId);
      objects = info.objects;
      startedFor = `${stepId}:${language}`;
      status = "ready";
      if (info.outdated.length)
        entries.push({ kind: "note", tone: "warn", text: `Using the last results of ${info.outdated.map((i) => ws.nameOf(i)).join(", ")}, which are outdated. Run the analysis for current inputs.` });
      return true;
    } catch (e) {
      status = "off";
      const m = errorMessage(e);
      entries.push({ kind: "note", tone: "error", text: `The console did not start: ${m}`, action: /run the analysis/.test(m) ? "run" : undefined });
      void scroll();
      return false;
    }
  }

  /** Run code in the console, starting it first if needed. */
  export async function run(code: string, fromLine: number | null = null) {
    if (!code.trim() || status === "starting" || status === "running") return;
    if (status === "off" && !(await start())) return;
    const entry: Entry = { kind: "run", code, fromLine, reply: null };
    entries.push(entry);
    const at = entries.length - 1;
    void scroll();
    status = "running";
    try {
      const reply = await ws.backend.consoleRun(code);
      entries[at] = { ...entry, reply };
      objects = reply.objects;
      status = "ready";
    } catch (e) {
      entries[at] = { ...entry, failed: errorMessage(e) };
      // The process is gone (it was stopped or would not stop).
      status = "off";
      info = null;
      objects = [];
    }
    void scroll();
  }

  /** After the step ran: say if its result differs from the console's. */
  export async function compare() {
    if (status !== "ready") return;
    try {
      const d = await ws.backend.consoleCompare(stepId);
      if (d) entries.push({ kind: "note", tone: "warn", text: d });
      void scroll();
    } catch {
      // Only a hint; nothing to do when it cannot be checked.
    }
  }

  async function stop() {
    await ws.backend.consoleStop().catch(() => {});
    status = "off";
    info = null;
    objects = [];
    startedFor = "";
  }

  async function restart() {
    await stop();
    entries.push({ kind: "note", tone: "info", text: `Restarted: only the inputs and parameters are loaded.` });
    await start();
    void scroll();
  }

  function submit() {
    const code = input;
    if (!code.trim()) return;
    history.push(code);
    recall = -1;
    input = "";
    void run(code);
  }

  function back(step: -1 | 1): string | null {
    if (!history.length) return null;
    if (recall === -1) recall = history.length;
    recall = Math.min(Math.max(recall + step, 0), history.length);
    return recall === history.length ? "" : history[recall]!;
  }

  function cell(v: unknown): string {
    if (v === null || v === undefined) return "";
    return typeof v === "number" ? v.toLocaleString(undefined, { maximumFractionDigits: 6 }) : String(v);
  }
</script>

<section class="mt-4 rounded-md border border-rule bg-panel" aria-label="{name} console">
  <header class="flex items-center gap-2 border-b border-rule px-3 py-1.5">
    <h3 class="min-w-0 text-sm font-semibold">{name} console</h3>
    <span class="min-w-0 flex-1 truncate text-xs text-ink-faint" aria-live="polite">
      {#if status === "off"}not started{:else if status === "starting"}starting with the inputs…{:else}{name} {info?.version}{status === "running" ? ", running…" : ""}{/if}
    </span>
    <span class="flex shrink-0 gap-1">
      {#if status === "running"}
        <button class="btn btn-quiet !min-h-7 !px-2 text-sm" onclick={() => ws.backend.consoleInterrupt()}><CircleStop size={14} /> Interrupt</button>
      {/if}
      {#if status === "off"}
        <button class="btn btn-quiet !min-h-7 !px-2 text-sm" onclick={() => start()}><Play size={14} /> Start</button>
      {:else}
        <button class="btn btn-quiet !min-h-7 !px-2 text-sm" disabled={status !== "ready"} onclick={restart} title="Start again with only the inputs"><RotateCcw size={14} /> Restart</button>
      {/if}
      <button class="btn btn-quiet !min-h-7 !px-2 text-sm" disabled={!entries.length} onclick={() => (entries = [])}><Eraser size={14} /> Clear</button>
    </span>
  </header>
  <div class="grid grid-cols-[minmax(0,1fr)_14rem] max-[56rem]:grid-cols-1">
    <div class="flex min-w-0 flex-col">
      <div bind:this={log} class="max-h-[26rem] min-h-24 overflow-y-auto px-3 py-2 text-sm" role="log" aria-label="{name} console output">
        {#each entries as e, i (i)}
          {#if e.kind === "note"}
            <p class="my-1.5 {e.tone === 'error' ? 'text-failed' : e.tone === 'warn' ? 'text-stale' : 'text-ink-soft'}" role={e.tone === "error" ? "alert" : undefined}>
              {e.text}
              {#if e.action === "run"}<button class="btn !min-h-6 ml-1 !px-2 text-xs" disabled={ws.running} onclick={async () => (await ws.run(), start())}>Run analysis</button>{/if}
            </p>
          {:else}
            <div class="console-entry border-t border-rule py-1.5 first:border-t-0">
              <pre class="code text-ink-soft whitespace-pre-wrap"><span class="text-ink-faint select-none">{language === "r" ? ">" : "›"} </span>{e.code}</pre>
              {#if e.failed}
                <p class="text-failed" role="alert">{e.failed}</p>
              {:else if !e.reply}
                <p class="text-ink-faint">Running…</p>
              {:else}
                {#if e.reply.stdout}<pre class="code whitespace-pre-wrap">{e.reply.stdout}</pre>{/if}
                {#if e.reply.stderr}<pre class="code whitespace-pre-wrap text-stale">{e.reply.stderr}</pre>{/if}
                {#if e.reply.text}<pre class="code whitespace-pre-wrap">{e.reply.text}</pre>{/if}
                {#if e.reply.table}
                  {@const t = e.reply.table}
                  <div class="mt-1 max-h-64 overflow-auto rounded border border-rule">
                    <table class="code w-full text-xs" aria-label="Console table">
                      <thead class="sticky top-0 bg-paper text-left">
                        <tr>{#each t.columns as [n, ty] (n)}<th class="px-2 py-1 font-semibold">{n}<span class="block font-normal text-ink-faint">{ty}</span></th>{/each}</tr>
                      </thead>
                      <tbody>
                        {#each t.rows as row, r (r)}
                          <tr class="border-t border-rule">
                            {#each row as v, c (c)}
                              <td class="px-2 py-0.5 whitespace-nowrap {typeof v === 'number' ? 'text-right' : ''}">{#if v === null}<span class="text-ink-faint italic">missing</span>{:else}{cell(v)}{/if}</td>
                            {/each}
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  </div>
                  <p class="mt-0.5 text-xs text-ink-faint">{t.rows.length < t.total ? `First ${t.rows.length} of ${t.total.toLocaleString()} rows` : `${t.total.toLocaleString()} row${t.total === 1 ? "" : "s"}`}</p>
                {/if}
                {#each e.reply.images as src, k (k)}<img class="mt-1 max-w-full rounded border border-rule bg-white" {src} alt="Plot {k + 1} drawn by the code" />{/each}
                {#if e.reply.error}
                  <p class="text-failed" role="alert">
                    {#if e.reply.line !== null && e.fromLine !== null}
                      <button class="underline decoration-dotted" onclick={() => ongoto?.(e.fromLine! + e.reply!.line! - 1)}>Line {e.fromLine + e.reply.line - 1}</button>:
                    {/if}{e.reply.error}
                  </p>
                {/if}
                <p class="text-xs text-ink-faint">{e.reply.elapsed_ms < 1000 ? `${e.reply.elapsed_ms} ms` : `${(e.reply.elapsed_ms / 1000).toFixed(1)} s`}</p>
              {/if}
            </div>
          {/if}
        {:else}
          <p class="text-ink-faint">
            Try code against this step's inputs without saving it. Objects you make stay here between runs, as in {language === "r" ? "RStudio" : "a notebook"}.
          </p>
        {/each}
      </div>
      <div class="flex items-start gap-2 border-t border-rule px-3 py-1.5">
        <span class="code mt-1.5 text-ink-faint select-none" aria-hidden="true">{language === "r" ? ">" : "›"}</span>
        <div class="min-w-0 flex-1">
          <CodeArea bind:value={input} lang={language} label="{name} console input" singleLine {context} placeholder="Code, then Enter (Up for earlier lines)" onenter={submit} onhistory={back} />
        </div>
      </div>
    </div>
    <aside class="border-l border-rule px-3 py-2 text-sm max-[56rem]:border-t max-[56rem]:border-l-0" aria-label="Environment">
      <h4 class="mb-1 text-xs font-semibold text-ink-soft">Environment</h4>
      {#each objects as o (o.name)}
        <div class="border-t border-rule py-1 first:border-t-0">
          <code class="code font-semibold">{o.name}</code>
          <span class="block text-xs text-ink-faint">{o.summary || o.kind}</span>
        </div>
      {:else}
        <p class="text-xs text-ink-faint">{status === "off" ? "Start the console to see what it holds." : "Nothing yet."}</p>
      {/each}
    </aside>
  </div>
</section>
