# ADR-0003: Desktop frontend

- **Status:** Accepted (2026-11-02)
- **Related:** ADR-0001 (local-first desktop, Tauri 2 shell, Rust core)

## Decision

The desktop UI is **Svelte 5** inside **Tauri 2**.

| Concern | Choice |
|---|---|
| UI framework | Svelte 5 with runes (`$state`, `$derived`, `$effect`), TypeScript |
| Build | Vite, as a single-page app. No SvelteKit: one window, no routing, no server rendering. |
| Styling and components | Tailwind CSS v4; shadcn-svelte components (on bits-ui) where they fit; Lucide icons |
| Charts | Vega-Lite specs from the core, rendered with `vega-embed` |
| Large tables | A virtualized grid that requests pages from the core as the user scrolls; rows never travel in bulk |
| Backend access | Typed wrapper modules, one per backend area, over Tauri `invoke` and events |

The React/TypeScript suggestion in the v0.2.0 briefs is superseded. The choice matches the Tauri 2 + Rust + Svelte 5 stack already used on another desktop app in this portfolio, so build tooling, IPC conventions and components carry over.

## Architecture

```
apps/desktop/
  src/                Svelte 5 UI
    lib/api/          typed wrappers: project.ts, run.ts, data.ts, steps.ts
    lib/api/mock.ts   in-memory backend for UI tests and browser preview
  src-tauri/          Tauri 2 shell: registers commands, runs analyses on a worker thread, emits events
crates/vda-app/       command layer: serde request/response types and the commands themselves,
                      over vda-core; no Tauri dependency, unit-tested without a window
crates/vda-core/      unchanged
```

**The command layer is a separate crate.** Every command the UI can call is a plain Rust function on an `App` value, with serde types for its inputs and outputs. The Tauri crate only forwards to it. Commands are tested with `cargo test` and no window, WebKit or display, and a future server can expose the same functions.

**Data crosses the IPC boundary in pages.** The UI asks for `preview(step, offset, limit)` and receives at most one page. Values are sent as JSON with their core type, so the grid can format dates, nulls and numbers consistently with the notebook.

**Long work never blocks the UI thread.** `run` starts the analysis on a worker thread and returns at once. Per-step progress arrives as `run://step` events and completion as `run://done`. `cancel` sets the session's cancel flag.

## Testing

- Rust: `cargo test -p vda-app` covers every command against real projects.
- UI: Vitest with the mock backend for component logic; `svelte-check` for types.
- End to end: the real Tauri app is launched under Xvfb in CI-like conditions and screenshotted through the mixed workflow.

## Options not chosen

- **React**: named in the briefs, but nothing in the product depends on its ecosystem, and it would split conventions across the portfolio's Tauri apps.
- **SvelteKit**: adds routing and server-rendering concepts a single-window desktop app does not use. Tauri supports it if multiple windows or routes appear later.
- **Sending whole results to the UI**: impossible at the data sizes measured (tens of millions of rows); paging is the contract from the start.
