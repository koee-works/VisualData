# First engineering slice — status

Updated: 2026-11-03. Decisions: [ADR-0001](docs/adr/0001-deployment-model.md) (local-first desktop; Tauri 2 + Rust core + embedded DuckDB + Python child process), [ADR-0002](docs/adr/0002-build-cache.md) (build cache), [ADR-0003](docs/adr/0003-frontend.md) (desktop frontend), [ADR-0004](docs/adr/0004-code-review-and-sandbox.md) (code review and SQL sandbox), [ADR-0005](docs/adr/0005-multi-input-steps.md) (several sources and multi-input steps), [ADR-0006](docs/adr/0006-formulas-and-core-transforms.md) (formulas and core transforms), [ADR-0007](docs/adr/0007-linter-and-console.md) (linter and console), [ADR-0008](docs/adr/0008-reshaping-and-window-calculations.md) (reshaping and window calculations), [ADR-0009](docs/adr/0009-charts-profiles-checks-parameters-export.md) (charts, column details, checks, parameters and notebook export), [ADR-0010](docs/adr/0010-history-collaboration-environment-refresh.md) (undo and history, comments, statistical tests, the Python environment, database sources, scheduled refresh).

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

All 162 Rust tests pass (`cargo test -p vda-core -p vda-app`), plus 55 frontend unit tests and the desktop end-to-end scenario (below). The demo writes a complete project folder: `cargo run -p vda-core --example slice -- <dir>`.

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

**End to end** (`apps/desktop/e2e/run_e2e.py`, the real app under Xvfb through tauri-driver, with a fresh approvals file), in two phases. First: open a project received from someone else → its Python code is shown for review → *Not now* → run: visual steps run, the Python step needs review and the chart waits → approve from the top bar → run → all current → inspect an aggregate's rows and SQL → edit the upstream filter → all four steps outdated, banner shown → rerun from the banner → notebook with text, tables and chart → chart step → Python step. Passes; screenshots in `docs/screenshots/`. Then a second project with two sources: run → the full join keeps a region with no orders, its SQL is a full outer join → switch to matching rows only → the join and the SQL after it go outdated → rerun → SQL that reads the second source by name → the second source's profile → a spreadsheet source with typed columns, stacked with a CSV by an append → rename a source in the app, rerun, and the SQL that names it follows → a formula error shown while typing and on save, fixed, run, and a top-2 sort after it → the console lists the linter's notes, one is dismissed, the run log shows the runs → a running total per region gets a rank added in the editor and reruns → a pivot with one column per product switches to counting rows → a parameter used in a formula: a new value outdates only that step and the sort after it → a check flags the 2 orders with no region, with examples, and passes the rows on → a histogram of revenue → the notebook saved as a report (no scripts, charts as SVG) and as a Jupyter notebook. Then a third project with a SQLite source: read by a query instead of a table → delete a step, undo, redo, undo (its result is reused) → history lists every version; restore the first and undo that → a comment and a reply, shown on the pipeline node → a statistical test → the Python environment and the schedule entry → after the app closes, `vda-run` refreshes the project, exports a file and the run is recorded as scheduled.

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

## History, comments, tests, environment, databases and refresh (ADR-0010)

| Part | Behaviour | Tests |
|---|---|---|
| Undo and history | Undo/redo for the session (100 changes, toolbar and Ctrl/Cmd+Z); every saved version in `history/` (500), described by what changed; restore, itself undoable; review applies to restored code. | `crates/vda-app/tests/slice4.rs` (`undo_redo_and_history_…`, `undoing_back_to_received_code_…`); `workspace.test.ts`; e2e steps 26–27 |
| Comments | On steps, sources and the analysis, with replies; resolve; only the author deletes; kept in `comments.json`, untouched by undo, follow renamed sources; counts on pipeline nodes and in reports. | `comments_are_kept_with_the_project_…`; `workspace.test.ts`; e2e step 28 |
| Statistical tests | Welch's t-test, one-way ANOVA, chi-square of independence, Pearson correlation; summaries from the engines, p-values from `stats.rs`; one row with a sentence; in exported notebooks via SciPy. | `tests/stat_tests.rs` (both engines and SciPy), `stats.rs` (unit), `tests/jupyter.rs`; e2e step 29 |
| Python environment | Interpreter, version and packages, with the `pip install` that fixes a missing one; interpreters found on the computer; choice saved per user; `VDA_PYTHON` wins. | `tests/python_runner.rs` (`the_environment_lists_…`), `the_python_environment_is_described_…`; e2e step 30 |
| Database sources | SQLite and DuckDB files by table or read-only query; copied on read and fingerprinted by content plus table or query; queries cannot change the database or read other files. | `tests/databases.rs` (types, sandbox, both engines, outdating, re-query), `a_database_becomes_a_source_…`; e2e step 25 |
| Scheduled refresh | `vda-run` reruns what is outdated, exports only after a fully successful run, one refresh at a time, exit codes 0/1/2/3, run log marks scheduled runs; the app writes the cron, launchd or schtasks entry. | `vda_run_refreshes_a_project_on_a_schedule`, `schedules_are_written_…`; e2e steps 30–31 |

Fixed while testing: a database source read by a query reused its table's copy; a CSV with mixed line endings gave a 30-line error.

## Charts, column details, checks, parameters and export (ADR-0009)

| Part | Behaviour | Tests |
|---|---|---|
| Charts | Bar, line, area, point, histogram, box plot; color series; sum/average/median/count/min/max/distinct count per category, computed over every row before the 5,000-row inline cap. | `tests/charts.rs` (`both_engines_draw_the_same_charts`, `summaries_use_every_row_not_the_first_five_thousand`, `charts_explain_what_does_not_fit`); `chart.test.ts`; e2e step 23 |
| Column details | Mean, spread, quartiles, zeros, negatives, blank text, text lengths, top 5 values, 10-bar distribution, for sources and results. | `column_details_agree_across_engines` |
| Checks | Rules: a formula true for every row, unique keys, row count bounds; failing counts and 5 examples; warn and pass rows on, or stop. | `tests/params_checks.rs` (`checks_report_…`, `a_check_that_warns_…`, `check_rules_are_validated_…`); `workspace.test.ts`; e2e step 22 |
| Parameters | `@name` in formulas and rules, filter values, `$name` in SQL, `params` in Python; bound before planning so a new value outdates only its readers (and Python steps); rename follows everywhere but Python; unknown and unused ones reported. | `parameters_reach_every_kind_of_step_…`, `parameters_mean_the_same_in_both_engines`, `params.rs` (unit), `crates/vda-app/tests/slice3.rs`; e2e step 21 |
| Notebook export | Self-contained HTML report (tables, check results, SVG charts, code; no scripts) and a Jupyter notebook that reruns the analysis with DuckDB and pandas. | `tests/jupyter.rs` (runs the exported notebook and compares every step), `workspace.test.ts`; e2e step 24 |

Fixed while testing: histograms drew no bars in the app (a chart style setting), and saving a report failed when a parameter held a decimal.

## Reshaping and window calculations (ADR-0008)

| Part | Behaviour | Tests |
|---|---|---|
| Window calculations | Running total, row number, rank, previous/next value, moving average, change and % change from previous, share of total; per group, in an order; rows stay in place. Explicit frames, ties in input order, missing values last, integer overflow an error. | `tests/reshape.rs` (`window_calculations_agree`, `window_errors_agree`) |
| Keep one row per key | Keys (or every column), first row in an order, missing keys equal, input order kept. | `keeping_one_row_per_key_agrees` |
| Unpivot | Listed columns become name/value rows, other columns kept, missing values kept, one value type (whole numbers and decimals combine). | `unpivot_agrees` |
| Pivot | A column per value (text, whole number, true/false, date), a row per group, any summary; columns known after the run; missing pivot values left out, with a warning. | `pivot_agrees` (including no rows and a name clash), `tests/lint.rs` |
| `bucket(x, width)` | Start of the band holding x; whole numbers or decimals; missing for a width that is not positive. | `bucket_agrees` |
| Inspectable SQL | A dedupe → window → unpivot → pivot chain as one query gives the step-by-step results. | `inspectable_sql_of_reshaping_steps_executes_to_the_same_result` |
| UI | Editors for the four steps, sharing an order list with Sort; a note when a window has no order. | `workspace.test.ts`; e2e steps 19–20, `docs/screenshots/14-window.png`, `15-pivot.png` |

## Linter and console (ADR-0007)

| Part | Behaviour | Tests |
|---|---|---|
| Static checks | Columns carried through the graph without running; invalid steps, formula errors with positions, SQL that does not parse or reads a table that is not an input, Python syntax errors (`ast`, no user code run) and missing `result`, row-by-row Python loops. | `tests/lint.rs` (`problems_are_found_before_anything_runs`) |
| From the last run | Joins that multiplied rows or matched none, filters that kept nothing, columns missing in every row, bar charts with too many bars; notes for unused steps, outdated results, code waiting for review; failures. | `tests/lint.rs` (`the_last_run_explains_surprising_results`, `failures_unused_steps_and_drafts`) |
| Drafts | A step being edited is checked unsaved; problems show at the formula, line or editor. | `tests/lint.rs`, `crates/vda-app/tests/console.rs`, e2e step 17 |
| Python output | stdout and stderr kept (64 KB each), also on failure; in run records and progress events. | `tests/python_runner.rs` (`printed_output_and_warnings_are_kept_even_when_the_step_fails`), `console.rs` |
| Console | Problems (links, counts, dismissals saved in `dismissed.json`), Output, Run log. | `workspace.test.ts`, `console.rs`, e2e step 18, `docs/screenshots/13-console.png` |

## Formulas and core transforms (ADR-0006)

| Part | Behaviour | Tests |
|---|---|---|
| Calculate columns | Formulas with columns, literals, arithmetic, comparisons, logic, `is missing`, and 33 functions (logic, numbers, text, type changes, dates). Checked against the input's columns with errors that say where and what; each formula sees earlier ones; a formula can replace a column. Division by zero and non-finite results are missing; integer overflow is an error. | `formula.rs` (unit), `tests/equivalence.rs` (`formulas_agree`: ~95 formulas on both engines; overflow; chaining and replacing; combined SQL) |
| Choose and rename columns | Keep listed columns in order, optionally renamed. | `select_and_sort_agree` |
| Sort and top N | Several keys, either direction, missing values last, ties in input order, optional limit. | `select_and_sort_agree` |
| More summaries | Median, percentiles P1–P99, distinct count, standard deviation. | `statistical_aggregates_agree` |
| Export | Sources and current results to CSV, Excel or Parquet; outdated results refused; Excel row limit explained. | `tests/export.rs` (round trip of all three), `crates/vda-app/tests/sources.rs` |
| UI | Editors for the three steps, a function reference, percentile choice in summaries, an Export menu on results. | `workspace.test.ts`; e2e step 17 (formula error, fix, run, top 2), `docs/screenshots/11-formula-error.png`, `12-formula.png` |

## Several sources and multi-input steps (ADR-0005)

| Part | Behaviour | Tests |
|---|---|---|
| Sources | Any number of sources, each with an id custom SQL can use: CSV, Parquet, JSON and spreadsheets (one sheet, chosen by name). **Add source** copies the file in and names it from the file name. A source can be renamed (inputs, SQL table references and Python `dfs[...]` follow; approved code stays approved) and removed once no step reads it. | `tests/source_formats.rs`, `tests/multi_input.rs` (renaming), `crates/vda-app/tests/sources.rs`, `rename.rs` (unit), `workspace.test.ts` |
| Join | Visual inner, left or full join on conditions between columns: `=` keys and `<`, `<=`, `>`, `>=`, `<>` for ranges. Missing values never match; integer/decimal and date/timestamp pairs can be compared. Right columns whose names clash get `_<input id>`. Output order is defined. | `tests/equivalence.rs` (`joins_agree`, `range_joins_agree`: price periods with a gap and an overlap, range-only full joins, date = timestamp; `join_validation_messages_match`, `join_conditions_must_compare`) |
| Append | Visual: stack two or more tables by column name, widening integer/decimal and date/timestamp; optionally drop repeated rows and record each row's input. | `tests/equivalence.rs` (`appends_agree`, combined SQL of an append) |
| SQL and Python with several inputs | SQL reads the first input as `input` and each input by id; Python gets `df` and `dfs`. | `tests/multi_input.rs`, `tests/equivalence.rs` (combined SQL with a join and a second input) |
| Graph and keys | Input counts, repeated inputs and cycles through any input are explained. Keys cover all inputs; single-input keys are unchanged from format 1, so old projects keep saved results. Changing one source outdates only its readers. | `tests/multi_input.rs` |
| Format 2 | `sources` and `inputs`; format 1 files and run records still load. | `tests/multi_input.rs` (`format_1_projects_open_and_keep_their_saved_results`, round trip) |
| UI | One tree per source, "with …" on multi-input steps, join editor with comparisons, append editor, extra inputs on code steps, rename and sheet choice on a source's page. | e2e steps 10–16, `docs/screenshots/8-join.png`, `9-sql-two-inputs.png`, `10-append.png` |

## Opening other people's projects (ADR-0004)

| Part | Behaviour | Tests |
|---|---|---|
| Python review | Python code runs only once its exact text (SHA-256) is approved. Approvals live per user outside every project; code written or edited in the app is approved as it is saved; a label-only save keeps unreviewed code unreviewed. Unapproved steps report *needs review*, steps after them *waiting on review*; everything else runs. | `review.rs` (unit), `crates/vda-app/tests/review.rs` (received project, approval remembered across restarts, edited file asks again, label-only save), `workspace.test.ts` |
| Trust all in this session | *Trust all until I quit* runs every Python step in every project until the app closes. Memory only, records no approvals, shown in the top bar with *Ask again*. | `trusting_all_is_never_saved`, `trusting_all_lasts_for_this_app_only`, `workspace.test.ts` |
| Review UI | Opening a project with unapproved code shows a dialog with each step's full code, what it reads, and what Python can do; *Not now* keeps the project usable. A top-bar button reopens it; the step editor has an inline *Approve*. | e2e steps 1–3, `docs/screenshots/0-review.png` |
| SQL sandbox | DuckDB may touch only the project folder, its spill directory and an external source file, then its configuration is locked. Custom SQL must be one read-only query, checked with DuckDB's parser before it runs. | `crates/vda-core/tests/sandbox.rs` (reads outside the project, `../`, globs, `COPY`, `ATTACH`, `INSTALL`, `SET`, `PRAGMA`, injected second statements), `review.rs` (received SQL cannot read a secret file) |

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
| **Python containment** | Approved Python runs with the user's full permissions; review is the only safeguard. | OS sandbox per platform (Landlock/seccomp, `sandbox-exec`, AppContainer): no network, read-only outside the project (ADR-0004). |
| **Managing approvals** | Approvals cannot be listed or revoked in the UI (`Approvals::revoke` exists in the core); they do not move between computers. | An approvals page in settings; signed projects for teams. |
| **Analysis functions, next** | Window calculations, duplicates by key, pivot/unpivot, binning; chart and notebook export. | ADR-0006, "Not done yet". |
| **Linter, further** | Project-wide rule settings; checking the columns custom SQL reads. | ADR-0007, "Not done yet". |
| **Combining data, further** | Join conditions on computed expressions need custom SQL; database and remote sources are not available (they need DuckDB extensions downloaded at run time). | ADR-0005, "Not done yet". |
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
