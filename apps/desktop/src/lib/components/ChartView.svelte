<script lang="ts">
  import { chartConfigFor } from "../chartConfig";
  import { themeById } from "../theme";
  import { themeState } from "../themeState.svelte";

  let { spec }: { spec: unknown } = $props();

  let el: HTMLDivElement | undefined = $state();
  let error = $state<string | null>(null);

  $effect(() => {
    const target = el;
    // vega-embed clones the spec; reactive proxies cannot be cloned.
    const raw = $state.snapshot(spec) as Record<string, unknown> | null;
    // Drawn again when the theme changes.
    const config = chartConfigFor(themeById(themeState.id));
    if (!target || !raw) return;
    // Fill the panel's width at a readable height.
    const s = { ...raw, width: "container", height: 280, autosize: { type: "fit", contains: "padding" } };
    let finalize: (() => void) | undefined;
    let cancelled = false;
    Promise.all([import("vega-embed"), import("vega-interpreter")]).then(async ([{ default: embed }, { expressionInterpreter }]) => {
      if (cancelled) return;
      try {
        // The interpreter evaluates chart expressions without `new Function`,
        // so the desktop app's content security policy can forbid eval.
        const result = await embed(target, s as never, {
          actions: false,
          renderer: "svg",
          config,
          ast: true,
          expr: expressionInterpreter,
        } as never);
        finalize = () => result.finalize();
        error = null;
      } catch (e) {
        error = (e as Error).message;
      }
    });
    return () => {
      cancelled = true;
      finalize?.();
    };
  });
</script>

{#if error}<p class="p-4 text-sm text-failed" role="alert">The chart could not be drawn: {error}</p>{/if}
<div class="min-w-0 overflow-hidden p-4">
  <div bind:this={el} class="w-full max-w-[56rem]" aria-label="Chart"></div>
</div>
