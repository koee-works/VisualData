// What the app uses, read from the backend every couple of seconds while the
// status bar shows it. Reading pauses when the window is hidden, so a window
// left in the background costs nothing.

import type { Backend } from "./api/backend";
import type { ResourceView } from "./api/types";

const EVERY_MS = 2000;

class Resources {
  value = $state<ResourceView | null>(null);
  /** The latest reading failed (the backend is busy or gone). */
  stale = $state(false);

  /** Start reading; returns what stops it. */
  watch(backend: Backend, every = EVERY_MS): () => void {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let stopped = false;
    const tick = async () => {
      if (stopped) return;
      if (typeof document === "undefined" || document.visibilityState === "visible") {
        try {
          this.value = await backend.resources();
          this.stale = false;
        } catch {
          this.stale = true;
        }
      }
      if (!stopped) timer = setTimeout(tick, every);
    };
    void tick();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }
}

export const resources = new Resources();
