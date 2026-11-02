# Visual data analysis app

An integrated workspace for exploring, transforming, analyzing and visualizing data through visual controls, SQL/Python code, and notebooks, without losing the thread of the analysis.

- Product briefs (v0.2.0): [docs/briefs/](docs/briefs/)
- Decisions: [ADR-0001 deployment model](docs/adr/0001-deployment-model.md), [ADR-0002 build cache](docs/adr/0002-build-cache.md)
- Current state of the first engineering slice: [STATUS.md](STATUS.md)

## Layout

```
crates/vda-core/          Rust core: model, compiler, planner, sessions, DuckDB engine, Python runner
  src/                    see STATUS.md for a module map
  tests/acceptance.rs     the 7-step shared acceptance scenario, saved results, reopen (DuckDB engine)
  tests/equivalence.rs    visual ops: reference engine vs DuckDB, and displayed SQL vs step results
  tests/python_runner.rs  timeout, cancel, memory cap, env isolation, type preservation
  examples/slice.rs       runs the slice end to end and writes a project folder
examples/regional-revenue/sales.csv   sample dataset (60 orders, with missing values)
docs/adr/                 architecture decision records
.github/workflows/ci.yml  CI with build caching (ADR-0002)
```

## Run it

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
  artifacts/<cache-key>.parquet     saved step results (tables)
  artifacts/<cache-key>.vl.json     saved step results (charts)
  runs/run-<ms>.json                run record: environment, source hash, per-step keys, status, content hashes
  notebook.md                       notebook render with SQL, code, state and previews
  charts/share_chart.vl.json        Vega-Lite spec (paste into https://vega.github.io/editor)
```

Reopening the folder with `Session::open` shows every current result without rerunning anything.

## Core ideas in the code

- **One analysis identity.** Visual steps, code steps and notebook cells all refer to step ids in one `Analysis`.
- **Cache keys chain.** A step's key hashes its definition, its upstream key (or the source file hash), the engine version, and for Python the interpreter, pandas and pyarrow versions. Any upstream change alters every downstream key, which is how stale results are detected.
- **Saved results are addressed by key.** A file named after the current key is by construction the current result, so reopening a project or undoing an edit reuses work without any bookkeeping.
- **Honest boundaries.** Visual steps compile to SQL. Custom SQL and Python are never turned back into visual controls; they are labelled as custom steps with explicit input (`input` / `df`) and output (`result`).
- **Replay is a separate check.** Replay runs the saved analysis in a clean session that ignores saved results, and compares each step's content hash with the recording, distinguishing *reproduced*, *diverged* (nondeterminism) and *inputs changed*.
