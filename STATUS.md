# First engineering slice — status

Date: 2026-11-02. Deployment model: [ADR-0001](docs/adr/0001-deployment-model.md) (local-first desktop; Tauri 2 + Rust core + embedded DuckDB + Python child process).

**Build constraint for this slice:** crates.io and PyPI were unreachable from the build environment, so `vda-core` uses the Rust standard library only. Python with pandas was available. Items marked **TEMPORARY** are stand-ins for crates that should replace them.

## Acceptance scenario coverage

From the "Shared team acceptance" section of the v0.2.0 briefs. Tests: `crates/vda-core/tests/acceptance.rs`.

| # | Scenario step | Status | Evidence |
|---|---|---|---|
| 1 | Import a dataset and inspect its schema/profile | Done (reference engine) | `step1_import_and_profile` — types, missing counts, distinct, min/max |
| 2 | Visual transformation + inspect its executable meaning | Done | `step2_…` — filter/aggregate compile to SQL; results match an independent pandas implementation |
| 3 | Custom code step using the prior result | Done | `step3_and_4_…` — Python step reads `df`, returns `result` |
| 4 | Visualize the output and explain it in a notebook | Done (spec + Markdown render) | Vega-Lite spec; notebook keeps document order separate from execution order |
| 5 | Save, reopen, replay with recorded inputs/environment | Done | `step5_…` — `analysis.json`, `runs/*.json`, clean-session replay, all steps reproduced |
| 6 | Change an upstream step; downstream stale until rerun | Done | `step6_…` — key chaining; label edits don't invalidate; source edits do |
| 7 | Controlled failure: diagnose, cancel or correct, recover | Done | `step7_…` (2 tests) — error with line number, downstream blocked, upstream kept fresh, cancel mid-step |

All 31 tests pass (`cargo test`). The demo writes a complete project folder: `cargo run -p vda-core --example slice -- <dir>`.

## What exists

| Module | Purpose |
|---|---|
| `model` | Analysis, Source, Step (filter, aggregate, sql, python, chart), notebook Cells |
| `compile` | Visual ops → DuckDB SQL; schema validation with actionable messages; inspectable CTE chain back to the source or nearest Python step |
| `reference` | Reference evaluator for visual ops with SQL NULL semantics. The correctness oracle for the DuckDB engine. |
| `plan` | Graph validation (unknown refs, duplicates, cycles, reading from a chart), execution order, chained cache keys |
| `session` | Run/caching, fresh/stale/failed/blocked/cancelled state, run records, replay with reproduced / diverged / inputs-changed outcomes |
| `python` | Child-process runner: `-I`, cleared env, private dir, timeout, cancel, memory cap (Unix), traceback summary |
| `chart` | Vega-Lite v5 spec, inline data capped at 5,000 rows with an explicit "first N of M" label |
| `notebook` | Markdown rendering: source profile, cells in document order, SQL/code, state badges, bounded previews |
| `persist` | `analysis.json` format v1 (integers stored as strings to survive > 2^53) |

## Explicit boundaries — not implemented

| Boundary | Current behaviour | Next step |
|---|---|---|
| **DuckDB engine** | Not built. Visual steps run on the reference engine; **custom SQL steps fail** with "needs the DuckDB engine" and show their SQL. | Add `duckdb` crate; implement `Engine` for it; run every reference test against both engines (cross-mode equivalence). |
| **Python interchange format** | **TEMPORARY CSV.** Float values are exact (round-trip parsing), but types can change: an integer column with NULLs returns as float; dates are text. | Parquet via pyarrow on the Python side and DuckDB on the Rust side. |
| **Source import** | **TEMPORARY** in-house CSV reader: infers BOOLEAN/BIGINT/DOUBLE/VARCHAR only; no dates, no delimiter sniffing. | `read_csv_auto` in DuckDB. Show inferred types to the user and allow overrides. |
| **Artifacts on disk** | Results are held in memory per session. Run records store content hashes, not results. Reopening means rerunning. | Content-addressed Parquet under `artifacts/<cache-key>.parquet`. |
| **Large data** | Whole tables in memory. Previews bounded to 10 rows, charts to 5,000. | DuckDB holds full results; UI pages previews. Benchmark before setting budgets. |
| **Hashing / JSON** | **TEMPORARY** in-house SHA-256 (verified against NIST vectors) and JSON. | `sha2`, `serde_json`. The on-disk formats stay the same. |
| **Notebook** | Markdown export only. `.ipynb` import/export not implemented. | Define the supported cell/output subset first (developer brief, rule 4). |
| **Trust prompt** | Not implemented. Any analysis that is opened can run its Python. | "Trust this analysis" gate before running code from analyses not created locally (ADR-0001). |
| **Memory cap on Windows** | Unix only (`ulimit -v`). | Job Object limits. |
| **Seeds / nondeterminism** | Not recorded. Replay detects divergence but cannot explain it. | Record seeds where users set them; flag known nondeterministic functions. |
| **Desktop app / UI** | Not started. Frontend framework not chosen (ADR-0001). | ADR-0002 (React vs Svelte 5), then a Tauri shell calling `vda-core`. |

## Known issues

- The inspectable pipeline SQL wraps each step in a subquery. SQL does not guarantee that an `ORDER BY` inside a subquery survives to the outer query. Execution is unaffected (each step runs separately with its own top-level `ORDER BY`), but the displayed combined SQL could return rows in a different order if pasted into DuckDB as is.
- Float sums in the reference engine use naive left-to-right addition. DuckDB may sum in a different order, so cross-engine comparisons must use a tolerance for floating-point aggregates.
- A failed or cancelled run keeps the previous result for display, labelled stale. The UI must not present it as current.
