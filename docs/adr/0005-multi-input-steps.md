# ADR-0005: Several sources and multi-input steps

- **Status:** Accepted (2026-11-02)
- **Related:** ADR-0001 (cache keys, saved results), ADR-0003 (desktop frontend), ADR-0004 (SQL sandbox)

## Context

An analysis had one source and every step read exactly one input, so the pipeline was always a tree. Real questions combine data: orders against targets, events against a customer list. That needs more than one data file, and steps that read more than one table.

## Decision

**An analysis has a list of sources.** Each source has an id and a path. Ids are plain identifiers, so custom SQL can name them unquoted; they share one namespace with step ids, and `input` is reserved. The first source keeps the id `source`. In the app, **Add source** copies a CSV into the project, and its id comes from the file name (`regions.csv` → `regions`, made unique).

**A step has a list of inputs.** The first is its main input. Each kind says how many it takes:

| Step | Inputs | How it reads them |
|---|---|---|
| Filter, aggregate, chart | 1 | `input` |
| **Join** (visual) | 2 | left = first, right = second |
| Custom SQL | 1 or more | `input` is the first; every input is also a table named by its id |
| Python | 1 or more | `df` is the first; `dfs` is a dict of every input by id |

**Join semantics** (identical in DuckDB and the reference engine, checked by `tests/equivalence.rs`):

- Kinds: *only rows that match* (inner), *every row of the left* (left), *every row of both* (full).
- Keys are pairs of equal columns. Missing values never match. Keys must have the same type, or both be numbers.
- Output columns: all left columns, then the right side's other columns. The right key columns are dropped, because they equal the left keys. In a full join, a key comes from whichever side has it, and an integer key joined to a decimal key becomes decimal. A right column whose name is already taken gets `_<right input id>` added (`region_customers`).
- Row order is defined: left rows in order, each followed by its matches in right order; in a full join, unmatched right rows come last. The SQL numbers rows to guarantee this, so replay hashes stay stable.

**The graph is a DAG.** Validation checks input counts, unknown or repeated inputs, chart inputs and cycles through any input. Execution order is a depth-first topological order that keeps the listed order where it can.

**Cache keys over all inputs.** A step's key hashes its definition with the keys of all its inputs, or the content hashes of the sources it reads, in input order. A single-input step's key is byte-for-byte the format-1 key, so projects saved before this change keep their saved results (`tests/multi_input.rs` pins the formula). Changing one source makes only the steps that read it, directly or indirectly, outdated.

**Inspectable SQL is a graph of CTEs, one per step, named by step id.** Custom SQL that names another input by id works the same in the combined query as when run alone. A join's SQL lists columns, so it needs its inputs' schemas. Until its inputs have run, the SQL tab says so instead of showing a guess.

**`analysis.json` format 2.** It uses `"sources": [{id, path}]` and `"inputs": [...]` per step. Format 1 is still read: its `source` gets the id `source`, and each step's `input` becomes a one-item list. Run records list every source with its hash, and older records with one `source` still load.

## Interface

- The pipeline shows one tree per source. A step sits under its main input, and a step that reads more says so on its row ("with regions.csv").
- The join editor has three parts: which table to combine with, which rows to keep (three described choices), and the key pairs. It explains how columns are named and flags key pairs whose types can't match before the user saves.
- SQL and Python steps list what else they read and can add or remove inputs. The contract text names each input as the code sees it.

## Not done yet

- **Union / append** of tables with the same columns. Possible today in custom SQL.
- **Join on expressions or ranges**, such as dates within a window. Use custom SQL.
- **Non-CSV sources** (Parquet, Excel, databases).
- **Renaming a source id.** Ids are fixed once added, because custom SQL refers to them by name.
- **A node canvas.** The tree with "with …" lines covers two-input steps. A canvas becomes more useful when graphs get wide (several joins feeding one step).

## Options not chosen

- **Second input inside the join's definition** (`Join { right: ... }`), instead of step inputs. That was simpler at first, but every consumer of the graph (planner, staleness, review gating, UI) would need a special case. One `inputs` list keeps the graph uniform.
- **Joins as custom SQL only.** That pushes the most common combining task out of the visual mode, which the briefs aim to avoid.
- **Keep both key columns in the output.** They're equal by construction, so keeping both only adds noise, and in a full join the left copy is missing for unmatched right rows.
