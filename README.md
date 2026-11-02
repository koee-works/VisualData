# Visual data analysis app

An integrated workspace for exploring, transforming, analyzing and visualizing data through visual controls, SQL/Python code, and notebooks, without losing the thread of the analysis.

- Product briefs (v0.2.0): [docs/briefs/](docs/briefs/)
- Deployment decision: [docs/adr/0001-deployment-model.md](docs/adr/0001-deployment-model.md)
- Current state of the first engineering slice: [STATUS.md](STATUS.md)

## Layout

```
crates/vda-core/          Rust core: model, compiler, planner, sessions, DuckDB engine, Python runner
  src/                    see STATUS.md for a module map
  tests/acceptance.rs     the 7-step shared acceptance scenario (DuckDB engine)
  tests/equivalence.rs    visual ops: reference engine vs DuckDB, and displayed SQL vs step results
  tests/python_runner.rs  timeout, cancel, memory cap, env isolation, exactness
  examples/slice.rs       runs the slice end to end and writes a project folder
examples/regional-revenue/sales.csv   sample dataset (60 orders, with missing values)
docs/adr/                 architecture decision records
```

## Run it

Requirements: stable Rust (built with 1.97), a C++ compiler, and `python3` with pandas and pyarrow on `PATH`.
The first build compiles DuckDB from source (the `bundled` feature), which takes several minutes.

```sh
cargo test
cargo run -p vda-core --example slice -- ./regional-revenue-project
```

The example writes:

```
regional-revenue-project/
  analysis.json              the analysis definition (format v1)
  sales.csv                  source
  runs/run-<ms>.json         run record: environment, source hash, per-step keys, status, content hashes
  notebook.md                notebook render with SQL, code, state and previews
  charts/share_chart.vl.json Vega-Lite spec (paste into https://vega.github.io/editor)
```

## Core ideas in the code

- **One analysis identity.** Visual steps, code steps and notebook cells all refer to step ids in one `Analysis`.
- **Cache keys chain.** A step's key hashes its definition, its upstream key (or the source file hash), the engine version, and for Python the interpreter and pandas versions. Any upstream change alters every downstream key, which is how stale results are detected.
- **Honest boundaries.** Visual steps compile to SQL. Custom SQL and Python are never turned back into visual controls; they are labelled as custom steps with explicit input (`input` / `df`) and output (`result`).
- **Replay is a separate check.** Replay runs the saved analysis in a clean session and compares each step's content hash with the recording, distinguishing *reproduced*, *diverged* (nondeterminism) and *inputs changed*.
