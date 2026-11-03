# Visual data analysis app

An integrated workspace for exploring, transforming, analyzing and visualizing data through visual controls, SQL/Python code, and notebooks, without losing the thread of the analysis.

- Product briefs (v0.2.0): [docs/briefs/](docs/briefs/)
- Decisions: [ADR-0001 deployment model](docs/adr/0001-deployment-model.md), [ADR-0002 build cache](docs/adr/0002-build-cache.md), [ADR-0003 desktop frontend](docs/adr/0003-frontend.md), [ADR-0004 code review and SQL sandbox](docs/adr/0004-code-review-and-sandbox.md)
- Current state of the first engineering slice: [STATUS.md](STATUS.md)

## Layout

```
apps/desktop/             desktop app: Svelte 5 UI (src/), Tauri 2 shell (src-tauri/), end-to-end test (e2e/)
crates/vda-app/           commands the UI calls, as plain Rust functions with serde types
crates/vda-core/          Rust core: model, compiler, planner, sessions, DuckDB engine, Python runner
  src/                    see STATUS.md for a module map
  tests/acceptance.rs     the 7-step shared acceptance scenario, saved results, reopen (DuckDB engine)
  tests/equivalence.rs    visual ops: reference engine vs DuckDB, and displayed SQL vs step results
  tests/python_runner.rs  timeout, cancel, memory cap, env isolation, type preservation
  tests/large.rs          disk-backed results: paging, spilling, source converted once
  tests/sandbox.rs        custom SQL: project files only, one read-only query
  examples/slice.rs       runs the slice end to end and writes a project folder
  examples/bench.rs       large-data benchmark (see STATUS.md for results)
examples/regional-revenue/sales.csv   sample dataset (60 orders, with missing values)
docs/adr/                 architecture decision records
.github/workflows/ci.yml  CI with build caching (ADR-0002)
```

## The desktop app

![The workspace after editing a filter: everything downstream is marked outdated](docs/screenshots/4-outdated.png)

One window: the pipeline on the left (each step's state as a coloured bar, hatched when outdated), the selected step's editor on top, and its result below, paged from disk. Visual steps have forms; SQL and Python steps are code with their input and output stated. The Notebook tab arranges text and step results for reading. More screenshots in [docs/screenshots](docs/screenshots).

Linux needs WebKitGTK (`libwebkit2gtk-4.1-dev` and the other [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)); macOS and Windows need nothing extra.

```sh
cd apps/desktop
npm install
npm run tauri dev                  # the app, with hot reload
npm run dev                        # the UI alone in a browser, against an in-memory mock (?demo=1 seeds an analysis)
npm run check && npm test          # types and unit tests
```

### Opening projects from other people

Python steps run as you, so Python code you didn't write in the app doesn't run until you've read and approved it. Opening such a project shows the code first; until it is approved, that step and the steps after it wait, and everything else works. Approvals are kept per user outside every project (`VDA_APPROVALS` overrides the location), so a project can't approve itself, and changing a single character of approved code asks again. SQL steps need no approval: they can only run one read-only query and only read files inside the project folder. See [ADR-0004](docs/adr/0004-code-review-and-sandbox.md).

![Python code from a received project, shown for review before it can run](docs/screenshots/0-review.png)

Open a project at startup with `vda-desktop --open <folder>`. The end-to-end test drives the real app through WebDriver: see `apps/desktop/e2e/` and the `desktop` CI job.

## Run the core

Requirements: stable Rust (built with 1.97), a C++ compiler, and Python 3 with pandas and pyarrow. The tests use `python3`, or whatever `VDA_PYTHON` names.

```sh
cargo test
cargo run -p vda-core --example slice -- ./regional-revenue-project
```

### Build cache (recommended)

The first build compiles DuckDB from source, about 17 minutes on two cores. Anything that resets the build (clippy, `cargo clean`, a new worktree, a profile change) repeats it unless the C++ objects are cached. Install [sccache](https://github.com/mozilla/sccache) once and opt in per shell:

```sh
cargo install sccache --locked
export RUSTC_WRAPPER=sccache SCCACHE_CACHE_SIZE=20G
# List every checkout or worktree root so they share cache entries:
export SCCACHE_BASEDIRS="$HOME/code/vda:$HOME/code/vda-wt"
```

With a warm cache, rebuilding DuckDB from scratch took 38 seconds instead of 17 minutes, and a second checkout at another path built in 53 seconds. Without `SCCACHE_BASEDIRS`, a checkout at a new path misses the cache almost entirely. sccache reads that variable when its server starts; run `sccache --stop-server` after changing it.

CI does the same automatically and also caches the compiled dependency tree. See [ADR-0002](docs/adr/0002-build-cache.md).

### What the example writes

```
regional-revenue-project/
  analysis.json                     the analysis definition (format v1)
  sales.csv                         source
  artifacts/<cache-key>.parquet     saved step results (tables), with .meta.json beside each
  artifacts/source-<key>.parquet    the source, converted once
  artifacts/<cache-key>.vl.json     saved step results (charts)
  runs/run-<ms>.json                run record: environment, source hash, per-step keys, status, content hashes
  notebook.md                       notebook render with SQL, code, state and previews
  charts/share_chart.vl.json        Vega-Lite spec (paste into https://vega.github.io/editor)
```

Reopening the folder with `Session::open` shows every current result without rerunning anything.

### Large data

Results stay on disk as Parquet; a session holds only handles and reads previews a page at a time. At 50 million rows (2.3 GB CSV) a full run took 19 s and the whole process peaked at 490 MB. To reproduce:

```sh
cargo run --release -p vda-core --example bench -- 10000000 ./bench-project 1024
```

## Core ideas in the code

- **One analysis identity.** Visual steps, code steps and notebook cells all refer to step ids in one `Analysis`.
- **Cache keys chain.** A step's key hashes its definition, its upstream key (or the source file hash), the engine version, and for Python the interpreter, pandas and pyarrow versions. Any upstream change alters every downstream key, which is how stale results are detected.
- **Saved results are addressed by key.** A file named after the current key is by construction the current result, so reopening a project or undoing an edit reuses work without any bookkeeping.
- **Honest boundaries.** Visual steps compile to SQL. Custom SQL and Python are never turned back into visual controls; they are labelled as custom steps with explicit input (`input` / `df`) and output (`result`).
- **Replay is a separate check.** Replay runs the saved analysis in a clean session that ignores saved results, and compares each step's content hash with the recording, distinguishing *reproduced*, *diverged* (nondeterminism) and *inputs changed*.
