<script lang="ts">
  import type { StepStateLabel } from "../api/types";

  let { state, running = false }: { state: StepStateLabel; running?: boolean } = $props();

  const words: Record<StepStateLabel, string> = {
    fresh: "Current",
    stale: "Outdated",
    "not run": "Not run",
    failed: "Failed",
    cancelled: "Cancelled",
    blocked: "Waiting on a failed step",
  };
  let label = $derived(running ? "Running" : words[state]);
</script>

<!-- A short bar beside each pipeline node; outdated results are hatched so
     the state reads without colour. -->
<span
  class="mark"
  class:hatch-stale={!running && state === "stale"}
  data-state={running ? "running" : state}
  role="img"
  aria-label={label}
  title={label}
></span>

<style>
  .mark {
    display: inline-block;
    width: 6px;
    align-self: stretch;
    min-height: 1.5rem;
    border-radius: 3px;
    background: var(--color-rule);
    flex: none;
  }
  .mark[data-state="fresh"] {
    background-color: var(--color-fresh);
  }
  .mark[data-state="stale"] {
    background-color: var(--color-stale-wash);
    box-shadow: inset 0 0 0 1px var(--color-stale);
  }
  .mark[data-state="failed"],
  .mark[data-state="blocked"] {
    background-color: var(--color-failed);
  }
  .mark[data-state="blocked"] {
    opacity: 0.45;
  }
  .mark[data-state="running"] {
    background-color: var(--color-running);
    animation: pulse 1.1s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
</style>
