# First engineering slice — status

Updated: 2026-11-02. Deployment model: [ADR-0001](docs/adr/0001-deployment-model.md) (local-first desktop; Tauri 2 + Rust core + embedded DuckDB + Python child process).

## Acceptance scenario coverage

From the "Shared team acceptance" section of the v0.2.0 briefs. Tests: `crates/vda-core/tests/acceptance.rs`, run on the **DuckDB engine** with a real Python interpreter.

| # | Scenario step | Status | Evidence |
|---|---|---|---|
| 1 | Import a dataset and inspect its schema/profile | Done | `step1_import_and_profile` — types, missing counts, distinct, min/max |
| 2 | Visual transformation + inspect its executable meaning | Done | `step2_…` — filter/aggregate compile to SQL; results match an independent pandas implementation; the displayed SQL executes to the same result (`equivalence.rs`) |
| 3 | Custom code step using the prior result | Done | `step3_and_4_…` (Python), `custom_sql_step_runs_on_duckdb` (SQL) |
| 4 | Visualize the output and explain it in a notebook | Done (spec + Markdown render) | Vega-Lite spec; notebook keeps document order separate from execution order |
| 5 | Save, reopen, replay with recorded inputs/environment | Done | `step5_…` — `analysis.json`, `runs/*.json`, clean-session replay, all steps reproduced |
| 6 | Change an upstream step; downstream stale until rerun | Done | `step6_…` — key chaining; label edits don't invalidate; source edits and engine changes do |
| 7 | Controlled failure: diagnose, cancel or correct, recover | Done | `step7_…` (2 tests) — error with line number, downstream blocked, upstream kept fresh, cancel mid-step |

All 41 tests pass (`cargo test`). The demo writes a complete project folder: `cargo run -p vda-core --example slice -- <dir>`.

## What exists

| Module | Purpose |
|---|---|
| `model` | Analysis, Source, Step (filter, aggregate, sql, python, chart), notebook Cells |
| `compile` | Visual ops → DuckDB SQL; schema validation with actionable messages; inspectable SQL chain back to the source or nearest Python step |
| `duck` | DuckDB engine (v2.0.0, bundled with Parquet compiled in, no run-time extension downloads); result types normalized to the core's four types; Parquet read/write |
| `reference` | Reference evaluator for visual ops with SQL NULL semantics. The correctness oracle for DuckDB (`tests/equivalence.rs`). |
| `plan` | Graph validation (unknown refs, duplicates, cycles, reading from a chart), execution order, chained cache keys |
| `session` | Run/caching, fresh/stale/failed/blocked/cancelled state, run records, replay with reproduced / diverged / inputs-changed outcomes |
| `python` | Child-process runner: Parquet in/out with pandas nullable dtypes, `-I`, cleared env, private dir, timeout, cancel, memory cap (Unix), traceback summary |
| `chart` | Vega-Lite v5 spec, inline data capped at 5,000 rows with an explicit "first N of M" label |
| `notebook` | Markdown rendering: source profile, cells in document order, SQL/code, state badges, bounded previews |
| `persist` | `analysis.json` format v1 (integers stored as strings to survive > 2^53) |

## Cross-engine equivalence

`tests/equivalence.rs` runs 21 visual-operation cases (every comparison operator, every aggregate, NULL keys, two-key groups, booleans, quoted text, int-vs-float literals, empty results, global aggregates) plus the demo chain on both engines and requires identical schemas and rows (floats within 1e-12 relative). It also checks:

- both engines import CSV identically;
- integer SUM overflow is an error in both, never a silent wrap;
- validation errors are worded identically on both;
- the combined SQL shown to users, run as one query in DuckDB, returns exactly what the steps return one by one, including a final custom-SQL step with its own `WITH` and `ORDER BY`.

## Explicit boundaries — not implemented

| Boundary | Current behaviour | Next step |
|---|---|---|
| **Date and time types** | CSV inference is limited to BOOLEAN/BIGINT/DOUBLE/VARCHAR in both engines, so dates stay text. Custom SQL results of other types (DATE, TIMESTAMP, LIST…) are cast to VARCHAR; DECIMAL becomes DOUBLE. | Add DATE and TIMESTAMP to the core types, reference semantics, Parquet mapping and chart encoding (`temporal`). |
| **Artifacts on disk** | Results are held in memory per session (and copied into DuckDB per step). Run records store content hashes, not results. Reopening means rerunning. | Content-addressed Parquet under `artifacts/<cache-key>.parquet`; DuckDB reads upstream artifacts directly instead of loading in-memory tables. |
| **Large data** | Whole tables in memory. Previews bounded to 10 rows, charts to 5,000. | Follows from artifacts on disk. Benchmark before setting budgets. |
| **Parallelism** | DuckDB runs with `threads = 1` so floating-point aggregation order, and therefore replay hashes, are deterministic. | Tolerance-aware replay comparison for floats, then raise the thread count. |
| **Hashing / JSON** | **TEMPORARY** in-house SHA-256 (verified against NIST vectors) and JSON. | `sha2`, `serde_json`. The on-disk formats stay the same. |
| **Notebook** | Markdown export only. `.ipynb` import/export not implemented. | Define the supported cell/output subset first (developer brief, rule 4). |
| **Trust prompt** | Not implemented. Any analysis that is opened can run its Python and SQL. | "Trust this analysis" gate before running code from analyses not created locally (ADR-0001). |
| **SQL file access** | Custom SQL can read any file the user can (`read_csv('/any/path')`). Acceptable for the user's own analyses under ADR-0001. | Restrict with DuckDB's `enable_external_access` / allowed paths for untrusted analyses, together with the trust prompt. |
| **Memory cap on Windows** | Unix only (`ulimit -v`). | Job Object limits. |
| **Seeds / nondeterminism** | Not recorded. Replay detects divergence but cannot explain it. | Record seeds where users set them; flag known nondeterministic functions. |
| **Desktop app / UI** | Not started. Frontend framework not chosen (ADR-0001). | ADR-0002 (React vs Svelte 5), then a Tauri shell calling `vda-core`. |

## Known issues

- The first build compiles DuckDB from source (about 20 minutes on 2 cores). CI needs a build cache; the desktop release build pays this once.
- In the combined SQL shown to users, only the final step's `ORDER BY` is guaranteed. Earlier steps are nested and their row order is not, which matters only if a later custom SQL step depends on input row order without sorting. Execution itself is step by step and unaffected.
- A failed or cancelled run keeps the previous result for display, labelled stale. The UI must not present it as current.
- Python results containing values pyarrow cannot store (for example a column mixing numbers and text) fail with a clear message rather than being coerced.
