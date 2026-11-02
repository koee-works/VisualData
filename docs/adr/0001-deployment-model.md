# ADR-0001: Deployment model for the MVP

- **Status:** Accepted (2026-11-02)
- **Supersedes:** "Still undecided: desktop/local versus hosted deployment" in the v0.2.0 briefs
- **Revisit when:** any trigger in *Revisit triggers* below is met

## Decision

The MVP is a **single-user, local-first desktop application**.

| Concern | Decision |
|---|---|
| Shell | Tauri 2 desktop app (macOS, Windows, Linux) |
| Core | Rust library crate (`vda-core`) that owns the analysis model, planning, staleness, run records and execution orchestration. It has **no Tauri dependency**, so it can later be wrapped by a server. |
| SQL engine | DuckDB embedded **in-process** via the `duckdb` crate |
| Python | Run as a **managed child process per step run**, against a user-selected or bundled interpreter. Data moves in and out through files, never through the Tauri IPC bridge. |
| Interchange | Parquet files on local disk (Arrow IPC is acceptable for in-memory previews) |
| Persistence | A project is a folder: `analysis.json` (model), `runs/` (run records), `artifacts/` (content-addressed results). No server database. |
| Network | None required. Data never leaves the machine. |
| Collaboration | Out of scope for the MVP. Sharing means handing over the project folder; the recipient replays it. |

The frontend framework is **not** decided here. Tauri works with React (named in the briefs) and with Svelte 5 (used on other Tauri apps in this portfolio). That choice gets its own ADR once the first UI slice starts.

## Why

1. **Running user code is the hardest problem, and local removes most of it.** On a desktop the user runs their own code with their own permissions, which is the same trust model as running Jupyter locally. A hosted product must isolate untrusted code from many tenants (gVisor/Firecracker-class sandboxes, quotas, network egress control). That isolation was the main driver behind the earlier Kubernetes recommendation and is the single largest cost the briefs identified.
2. **The core value is continuity, not hosting.** The first thing to prove is that visual, SQL, Python and notebook work stay connected. None of that needs a server.
3. **Data gravity.** The first workflow starts from a CSV on the analyst's machine. In-process DuckDB on local files is the fastest path, with no upload step and no data-residency questions.
4. **Reproducibility is simpler.** Content-addressed artifacts in a local folder make clean replay and stale detection straightforward and testable.
5. **Fits the existing toolchain.** Tauri 2 + Rust is already in use on other projects, so packaging, IPC patterns and release tooling carry over.

## Consequences

**Accepted costs**

- No real-time collaboration, shared workspaces, tenancy, or org administration in the MVP.
- Each install needs a Python interpreter. The app must detect one, let the user choose, or bundle one (bundling decision deferred).
- Dataset size is bounded by the user's machine. DuckDB's out-of-core execution softens this but does not remove it.

**Security posture**

- A child process gives **lifecycle isolation, not a sandbox**. That is acceptable for the user's own code.
- **Analyses or notebooks received from someone else are untrusted.** The app requires an explicit "trust this analysis" confirmation before any of its code runs, and rich outputs are sanitized before display.
- Every Python run has a wall-clock timeout and can be cancelled. On Unix the runner also sets a memory limit (`RLIMIT_AS`); Windows uses a Job Object (to implement).
- The Python child starts with a minimal environment (no inherited secrets) and in isolated mode (`python -I`).

**What keeps the hosted option open**

- `vda-core` talks to execution through two interfaces: an **engine** (SQL and Python) and an **artifact store**. A future server implements the same interfaces with sandboxed workers and object storage.
- The analysis file format and run records are deployment-neutral.

## Revisit triggers

Reopen this decision if any of these become true:

- Users need to work on the same analysis at the same time.
- Typical datasets no longer fit on a laptop.
- A paying customer requires central administration, audit, or SSO.
- Distribution of a desktop app becomes a bigger barrier than hosting cost.

A likely next step at that point is **hybrid**: keep the local engine, add sync and sharing of analysis files and artifacts.

## Options not chosen

- **Hosted web app (multi-tenant):** strongest for collaboration, but requires sandboxed code execution, tenancy, and operations before proving the core value.
- **Hybrid from day one:** adds sync and conflict handling before there is anything worth syncing.
- **Electron instead of Tauri:** larger bundles and no advantage for a Rust core.
