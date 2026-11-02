# First engineering slice — status

Updated: 2026-11-02. Decisions: [ADR-0001](docs/adr/0001-deployment-model.md) (local-first desktop; Tauri 2 + Rust core + embedded DuckDB + Python child process), [ADR-0002](docs/adr/0002-build-cache.md) (build cache).

## Acceptance scenario coverage

From the "Shared team acceptance" section of the v0.2.0 briefs. Tests: `crates/vda-core/tests/acceptance.rs`, run on the **DuckDB engine** with a real Python interpreter.

| # | Scenario step | Status | Evidence |
|---|---|---|---|
| 1 | Import a dataset and inspect its schema/profile | Done | `step1_import_and_profile` — types (including DATE/TIMESTAMP), missing counts, distinct, min/max |
| 2 | Visual transformation + inspect its executable meaning | Done | `step2_…` — filter/aggregate compile to SQL; results match an independent pandas implementation; the displayed SQL executes to the same result (`equivalence.rs`) |
| 3 | Custom code step using the prior result | Done | `step3_and_4_…` (Python), `custom_sql_step_runs_on_duckdb` (SQL) |
| 4 | Visualize the output and explain it in a notebook | Done (spec + Markdown render) | Vega-Lite spec (temporal axes for dates); notebook keeps document order separate from execution order |
| 5 | Save, reopen, replay with recorded inputs/environment | Done | `step5_…`, `reopening_a_project_shows_saved_results_without_rerunning`, `replay_ignores_saved_results` |
| 6 | Change an upstream step; downstream stale until rerun | Done | `step6_…`, `undoing_an_edit_reuses_the_earlier_saved_results` |
| 7 | Controlled failure: diagnose, cancel or correct, recover | Done | `step7_…` (2 tests), `unreadable_saved_result_is_recomputed` |

All 53 tests pass (`cargo test`). The demo writes a complete project folder: `cargo run -p vda-core --example slice -- <dir>`.

## What exists

| Module | Purpose |
|---|---|
| `model` | Analysis, Source, Step (filter, aggregate, sql, python, chart), notebook Cells |
| `table` | Values and types: BOOLEAN, BIGINT, DOUBLE, VARCHAR, DATE, TIMESTAMP (no time zone, microseconds); strict ISO parsing; profiles |
| `compile` | Visual ops → DuckDB SQL (typed `DATE`/`TIMESTAMP` literals); schema validation with actionable messages; inspectable SQL chain |
| `duck` | DuckDB engine (v2.0.0, bundled with Parquet compiled in, no run-time extension downloads); result types normalized to the core's six types; Parquet read/write |
| `reference` | Reference evaluator for visual ops with SQL NULL semantics. The correctness oracle for DuckDB (`tests/equivalence.rs`). |
| `plan` | Graph validation (unknown refs, duplicates, cycles, reading from a chart), execution order, chained cache keys |
| `artifacts` | Saved results: `artifacts/<cache-key>.parquet` / `.vl.json`, atomic writes, unreadable files treated as missing, pruning |
| `session` | Run/caching (memory, then saved results, then execute), `Session::open` without rerunning, fresh/stale/failed/blocked/cancelled state, run records with warnings, clean replay |
| `python` | Child-process runner: Parquet in/out with pandas nullable dtypes, `-I`, cleared env, private dir, timeout, cancel, memory cap (Unix), traceback summary, hard exit after errors |
| `chart` | Vega-Lite v5 spec, inline data capped at 5,000 rows with an explicit "first N of M" label |
| `notebook` | Markdown rendering: source profile, cells in document order, SQL/code, state badges, bounded previews |
| `persist` | `analysis.json` format v1 (integers as strings to survive > 2^53; dates and timestamps as ISO strings) |

## Cross-engine equivalence

`tests/equivalence.rs` runs 29 visual-operation cases on both engines and requires identical schemas and rows (floats within 1e-12 relative): every comparison operator, every aggregate, NULL keys, two-key groups, booleans, quoted text, int-vs-float literals, empty results, global aggregates, and dates/timestamps (pre-1970 values, fractional seconds, date-vs-timestamp comparisons, grouping by date, MIN/MAX of timestamps). It also checks:

- both engines import CSV identically, including what stays text: columns mixing bare dates with timestamps, US-style dates, and impossible dates like `2026-02-30`;
- integer SUM overflow is an error in both, never a silent wrap; SUM of a date is rejected before execution;
- validation errors are worded identically on both;
- the combined SQL shown to users, run as one query in DuckDB, returns exactly what the steps return one by one.

## Fixed in this round

- **Out-of-memory Python steps hung until their timeout.** After a `MemoryError`, Python's shutdown deadlocked in pyarrow's thread pool, so the step sat for the full timeout (30 s by default) instead of failing at once. Found through an intermittent test failure; reproduced by catching the hung process. The wrapper now flushes its error and exits immediately, and the memory test asserts the failure arrives within 10 s. Twenty consecutive stress runs had no hang.
- **A test changed a process-wide environment variable** while other tests started child processes in parallel (a data race; `set_var` is `unsafe` in newer Rust editions for this reason). The test now inspects the child's whole environment instead.

## Explicit boundaries — not implemented

| Boundary | Current behaviour | Next step |
|---|---|---|
| **Large data** | Saved results are on disk, but a running session still holds each result in memory and copies the upstream table into DuckDB for every step. Previews bounded to 10 rows, charts to 5,000. | DuckDB reads upstream artifacts directly (`read_parquet`), and only bounded previews come into memory. Benchmark before setting budgets. |
| **Other temporal types** | TIMESTAMPTZ, TIME and INTERVAL results are cast to VARCHAR; nanosecond timestamps are truncated to microseconds; DECIMAL becomes DOUBLE. CSV import recognizes ISO formats only. | Add time-zone-aware timestamps when a real dataset needs them; let users override inferred types on import. |
| **Artifact growth** | Saved results accumulate as definitions change. `prune_saved_results` exists but nothing calls it automatically. | Prune on save or on close, keeping results referenced by the current definition and the latest run record. |
| **Parallelism** | DuckDB runs with `threads = 1` so floating-point aggregation order, and therefore replay hashes, are deterministic. | Tolerance-aware replay comparison for floats, then raise the thread count. |
| **Hashing / JSON** | **TEMPORARY** in-house SHA-256 (verified against NIST vectors) and JSON. | `sha2`, `serde_json`. The on-disk formats stay the same. |
| **Notebook** | Markdown export only. `.ipynb` import/export not implemented. | Define the supported cell/output subset first (developer brief, rule 4). |
| **Trust prompt** | Not implemented. Any analysis that is opened can run its Python and SQL. | "Trust this analysis" gate before running code from analyses not created locally (ADR-0001). |
| **SQL file access** | Custom SQL can read any file the user can (`read_csv('/any/path')`). Acceptable for the user's own analyses under ADR-0001. | Restrict with DuckDB's `enable_external_access` / allowed paths for untrusted analyses, together with the trust prompt. |
| **Windows** | Not run. The memory cap is Unix only (`ulimit -v`); CI runs Windows as allowed-to-fail. | Job Object limits, then make the Windows job required. |
| **Seeds / nondeterminism** | Not recorded. Replay detects divergence but cannot explain it. | Record seeds where users set them; flag known nondeterministic functions. |
| **Desktop app / UI** | Not started. Frontend framework not chosen (ADR-0001). | ADR-0003 (React vs Svelte 5), then a Tauri shell calling `vda-core`. |

## Known issues

- Cold builds compile DuckDB from source (17 minutes on 2 cores without dependency debug info; 38 s when sccache is warm). See ADR-0002. The CI workflow in `.github/workflows/ci.yml` has not run yet.
- In the combined SQL shown to users, only the final step's `ORDER BY` is guaranteed. Earlier steps are nested and their row order is not, which matters only if a later custom SQL step depends on input row order without sorting. Execution itself is step by step and unaffected.
- A failed or cancelled run keeps the previous result for display, labelled stale. The UI must not present it as current.
- Python results containing values pyarrow cannot store (for example a column mixing numbers and text) fail with a clear message rather than being coerced.
