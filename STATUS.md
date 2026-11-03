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

All 63 Rust tests pass (`cargo test --workspace`), plus 11 frontend unit tests and the desktop end-to-end scenario (below). The demo writes a complete project folder: `cargo run -p vda-core --example slice -- <dir>`.

## What exists

| Module | Purpose |
|---|---|
| `model` | Analysis, Source, Step (filter, aggregate, sql, python, chart), notebook Cells |
| `table` | Values and types: BOOLEAN, BIGINT, DOUBLE, VARCHAR, DATE, TIMESTAMP (no time zone, microseconds); strict ISO parsing; profiles |
| `compile` | Visual ops → DuckDB SQL (typed `DATE`/`TIMESTAMP` literals); schema validation with actionable messages; inspectable SQL chain |
| `duck` | DuckDB engine (v2.0.0, bundled with Parquet compiled in, no run-time extension downloads). Reads each input with `read_parquet`, writes each result with `COPY … TO`; memory limit and spill directory configurable; result types normalized to the core's six types |
| `dataset` | Disk-backed result handle: Parquet path, schema, row count, size, content hash |
| `reference` | Reference evaluator for visual ops with SQL NULL semantics. The correctness oracle for DuckDB (`tests/equivalence.rs`). |
| `plan` | Graph validation (unknown refs, duplicates, cycles, reading from a chart), execution order, chained cache keys |
| `artifacts` | Saved results: `<cache-key>.parquet` + `.meta.json` / `.vl.json`, the converted source, and the source fingerprint; atomic writes; damaged files treated as missing; pruning; throwaway stores for clean replay |
| `session` | Holds handles only. Run/caching (held, then saved, then execute), source converted once, paged previews, `Session::open` without rerunning or rehashing, fresh/stale/failed/blocked/cancelled state, run records, clean replay |
| `python` | Child-process runner: Parquet in/out with pandas nullable dtypes, `-I`, cleared env, private dir, timeout, cancel, memory cap (Unix), traceback summary, hard exit after errors |
| `chart` | Vega-Lite v5 spec, inline data capped at 5,000 rows with an explicit "first N of M" label |
| `notebook` | Markdown rendering: source profile (computed in SQL), cells in document order, SQL/code, state badges, first-page previews |
| `persist` | `analysis.json` format v1 (integers as strings to survive > 2^53; dates and timestamps as ISO strings) |

## Cross-engine equivalence

`tests/equivalence.rs` runs 29 visual-operation cases on both engines and requires identical schemas and rows (floats within 1e-12 relative): every comparison operator, every aggregate, NULL keys, two-key groups, booleans, quoted text, int-vs-float literals, empty results, global aggregates, and dates/timestamps (pre-1970 values, fractional seconds, date-vs-timestamp comparisons, grouping by date, MIN/MAX of timestamps). It also checks:

- both engines import CSV identically, including what stays text: columns mixing bare dates with timestamps, US-style dates, and impossible dates like `2026-02-30`;
- integer SUM overflow is an error in both, never a silent wrap; SUM of a date is rejected before execution;
- validation errors are worded identically on both;
- the combined SQL shown to users, run as one query in DuckDB, returns exactly what the steps return one by one.

## Desktop app

Svelte 5 in Tauri 2 ([ADR-0003](docs/adr/0003-frontend.md)), built and run on Linux (WebKitGTK 2.52).

| Part | State |
|---|---|
| Command layer (`crates/vda-app`) | Every UI action as a plain Rust function with serde types; 4 tests drive a full workflow without a window |
| Tauri shell | Commands, runs on a worker thread with per-step progress events, cancel, open dialogs, `--open <dir>`; strict content security policy (no eval) |
| Pipeline | Tree of steps with state bars (current, outdated hatched, failed, blocked, running), row counts, run times; add steps of each kind from the selected node |
| Step editors | Visual filter (typed values with inline errors), group and summarize, chart; SQL and Python with their input/output contract; save with Ctrl/Cmd+S; delete blocked while other steps depend on it |
| Results | Virtualized grid fetching 200-row pages as you scroll (scaled scrolling for tens of millions of rows); column types; inspectable SQL; charts via Vega-Lite; outdated banner with "Run now" |
| Source | Column profile (types, missing values, distinct counts, ranges) and data |
| Notebook | Markdown and step cells in document order, previews of each result, reordering; text is escaped before rendering |
| Not yet | Creating projects from the welcome screen is wired but was not exercised end to end (file dialogs cannot be scripted under Xvfb); no undo; no column resizing; keyboard navigation inside the grid |

**End to end** (`apps/desktop/e2e/run_e2e.py`, the real app under Xvfb through tauri-driver): open a project → all steps not run → run → all current → inspect an aggregate's rows and SQL → edit the upstream filter → all four steps outdated, banner shown → rerun from the banner → notebook with text, tables and chart → chart step → Python step. Passes; screenshots in `docs/screenshots/`.

Found and fixed while building it: an effect loop in the grid (reading the page cache while clearing it), Vega cloning Svelte's reactive proxies, charts widening the window, and long decimals (now six places on screen, full value on hover).

## Large-data baseline

Measured 2026-11-02 with `cargo run --release -p vda-core --example bench -- <rows> <dir> <memory_limit_mb>` on a 2-core Linux machine. The analysis: filter (keeps 97% of rows) → aggregate to 350 groups → Python share calculation → chart, plus a custom SQL step ranking 100,000 customers.

| | 10M rows | 50M rows |
|---|---|---|
| CSV size | 447 MB | 2,277 MB |
| DuckDB memory limit | 1,024 MB | 256 MB |
| Import CSV → Parquet (once per source version) | 6.3 s | 32.7 s |
| Source as Parquet | 133 MB | 663 MB |
| Full run, all steps | 3.9 s | 19.2 s |
| — filter keeping 9.7M / 48.5M rows | 2.9 s | 15.4 s |
| — aggregate | 0.12 s | 0.61 s |
| — Python step (on the 350-row aggregate) | 0.28 s | 0.31 s |
| — custom SQL over 100,000 customers | 0.57 s | 2.9 s |
| 50-row preview from the middle of the filtered result | 15 ms | 52 ms |
| Profile of all 8 source columns (exact distinct counts) | 1.8 s | 9.1 s |
| Rerun with nothing changed | 0 ms | 0 ms |
| Reopen project, all 5 steps fresh | 355 ms | 328 ms |
| **Peak memory of the whole process** | **384 MB** | **490 MB** |
| Saved results on disk | 261 MB | 1,305 MB |

What this shows:

- **Memory no longer grows with data.** Five times the data raised peak memory by about a quarter. The 256 MB setting limits DuckDB's buffer pool, not the whole process, which is why peak memory exceeds it.
- **Time scales linearly**, at roughly 3 million rows per second for a filter that writes most of its input back out.
- **Reopening is constant time**: the remaining ~330 ms is mostly starting Python once to read its version for cache keys.
- **Disk is the cost.** Every step's result is a file, so a filter that keeps most rows nearly duplicates the source. At 50M rows the saved results took 1.3 GB.

Proposed budgets (to confirm with product and design): preview under 100 ms, reopen under 500 ms, and a progress indicator for any step expected to exceed 1 s. These figures are from a 2-core machine; typical analyst laptops are faster.

Before this change, results were in-memory tables. The regional-revenue sample was the largest dataset tested, and a session needed memory proportional to every held result.

## Fixed in this round

- **Integer sums silently became floats on disk.** Parquet has no 128-bit integer type, and DuckDB's `SUM` over BIGINT returns one, so sums were stored as DOUBLE and overflow went undetected. Results with such columns are now cast to BIGINT before writing; overflow is an error again (caught by the cross-engine tests).
- **Reopening a large project rehashed the source** (1.9 s at 10M rows). The source fingerprint is now saved with the results (355 ms).
- **Out-of-memory Python steps hung until their timeout.** After a `MemoryError`, Python's shutdown deadlocked in pyarrow's thread pool, so the step sat for the full timeout (30 s by default) instead of failing at once. Found through an intermittent test failure; reproduced by catching the hung process. The wrapper now flushes its error and exits immediately, and the memory test asserts the failure arrives within 10 s. Twenty consecutive stress runs had no hang.
- **A test changed a process-wide environment variable** while other tests started child processes in parallel (a data race; `set_var` is `unsafe` in newer Rust editions for this reason). The test now inspects the child's whole environment instead.

## Explicit boundaries — not implemented

| Boundary | Current behaviour | Next step |
|---|---|---|
| **Disk use of pass-through steps** | Every step writes a full result file, so filters that keep most rows nearly duplicate their input (1.3 GB of results at 50M rows). | Store simple filters and projections as views over their input instead of files, and prune superseded results automatically. |
| **Python on large inputs** | A Python step loads its whole input into pandas, bounded by the Python memory cap (2 GB default). Fine after an aggregate; too big for tens of millions of raw rows. | Warn before running Python on a large input; consider chunked or Arrow-native execution later. |
| **Progress and cancellation inside DuckDB** | Cancel takes effect between steps and inside Python, not in the middle of a long DuckDB query (the 15 s filter at 50M rows). | Run queries on a worker thread and call DuckDB's interrupt; report progress. |
| **Other temporal types** | TIMESTAMPTZ, TIME and INTERVAL results are cast to VARCHAR; nanosecond timestamps are truncated to microseconds; DECIMAL becomes DOUBLE. CSV import recognizes ISO formats only. | Add time-zone-aware timestamps when a real dataset needs them; let users override inferred types on import. |
| **Artifact growth** | Saved results accumulate as definitions change. `prune_saved_results` exists but nothing calls it automatically. | Prune on save or on close, keeping results referenced by the current definition and the latest run record. |
| **Parallelism** | DuckDB runs with `threads = 1` so floating-point aggregation order, and therefore replay hashes, are deterministic. | Tolerance-aware replay comparison for floats, then raise the thread count. |
| **Hashing / JSON** | **TEMPORARY** in-house SHA-256 (verified against NIST vectors) and JSON. | `sha2`, `serde_json`. The on-disk formats stay the same. |
| **Notebook** | Markdown export only. `.ipynb` import/export not implemented. | Define the supported cell/output subset first (developer brief, rule 4). |
| **Trust prompt** | Not implemented. Any analysis that is opened can run its Python and SQL. | "Trust this analysis" gate before running code from analyses not created locally (ADR-0001). |
| **SQL file access** | Custom SQL can read any file the user can (`read_csv('/any/path')`). Acceptable for the user's own analyses under ADR-0001. | Restrict with DuckDB's `enable_external_access` / allowed paths for untrusted analyses, together with the trust prompt. |
| **Windows** | Not run. The memory cap is Unix only (`ulimit -v`); CI runs Windows as allowed-to-fail. | Job Object limits, then make the Windows job required. |
| **Seeds / nondeterminism** | Not recorded. Replay detects divergence but cannot explain it. | Record seeds where users set them; flag known nondeterministic functions. |
| **Desktop: macOS and Windows** | Built and tested on Linux only. | Add macOS and Windows to the desktop CI job; produce signed bundles. |
| **Desktop: reading during a run** | Other commands wait for a run to finish (one lock per project). | Read-only connections per request if long runs need live inspection. |
| **Desktop: step labels** | A custom step name is kept when the step changes, so it can stop describing the step (e.g. "at least 20" after changing to 100). | Offer to update the name when the definition changes, or show the description alongside. |

## Known issues

- Cold builds compile DuckDB from source (17 minutes on 2 cores without dependency debug info; 38 s when sccache is warm). See ADR-0002. The CI workflow in `.github/workflows/ci.yml` has not run yet.
- In the combined SQL shown to users, only the final step's `ORDER BY` is guaranteed. Earlier steps are nested and their row order is not, which matters only if a later custom SQL step depends on input row order without sorting. Execution itself is step by step and unaffected.
- A failed or cancelled run keeps the previous result for display, labelled stale. The UI must not present it as current.
- Each new `Session` starts Python once to read its version (part of the cache key); about 300 ms of the reopen time.
- Python results containing values pyarrow cannot store (for example a column mixing numbers and text) fail with a clear message rather than being coerced.
