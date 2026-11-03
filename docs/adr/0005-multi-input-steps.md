# ADR-0005: Several sources and multi-input steps

- **Status:** Accepted (2026-11-02); extended 2026-11-03 with range joins, append, more source formats and renaming
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
| **Append** (visual) | 2 or more | stacked in order |
| Custom SQL | 1 or more | `input` is the first; every input is also a table named by its id |
| Python | 1 or more | `df` is the first; `dfs` is a dict of every input by id |

**Join semantics** (identical in DuckDB and the reference engine, checked by `tests/equivalence.rs`):

- Kinds: *only rows that match* (inner), *every row of the left* (left), *every row of both* (full).
- Conditions are pairs of columns with a comparison: `=` (a key), or `<`, `<=`, `>`, `>=`, `<>` for ranges ("sale day on or after the price's start and on or before its end"). A row pair matches when every condition holds. Missing values never match. Compared columns must have the same type, both be numbers, or both be dates/timestamps. Conditions on computed expressions remain custom SQL.
- Output columns: all left columns, then the right side's other columns. Right columns matched with `=` are dropped, because they equal the left keys; columns used in comparisons are kept. In a full join, a key comes from whichever side has it, and an integer key joined to a decimal key becomes decimal. A right column whose name is already taken gets `_<right input id>` added (`region_customers`).
- Row order is defined: left rows in order, each followed by its matches in right order; in a full join, unmatched right rows come last. The SQL numbers rows to guarantee this, so replay hashes stay stable.

**Append semantics** (also checked against the reference engine):

- Rows of every input in input order, each input's rows in order.
- Columns are matched by name: the first input's columns in its order, then columns that only later inputs have. An input without a column has it missing.
- Integers and decimals combine as decimals; dates and timestamps as timestamps. Any other type difference is an error naming the column and both inputs.
- *Remove repeated rows* keeps the first of identical rows (missing values compare equal here, as in SQL `DISTINCT`).
- *Record which table each row came from* adds a text column holding each row's input id. It counts towards "identical", so rows from different inputs never collapse.

**The graph is a DAG.** Validation checks input counts, unknown or repeated inputs, chart inputs and cycles through any input. Execution order is a depth-first topological order that keeps the listed order where it can.

**Cache keys over all inputs.** A step's key hashes its definition with the keys of all its inputs, or the content hashes of the sources it reads, in input order. With more than one input the input ids are included too, since a join names clashing columns after its right input and an append records input ids. A single-input step's key is byte-for-byte the format-1 key, so projects saved before this change keep their saved results (`tests/multi_input.rs` pins the formula). Changing one source makes only the steps that read it, directly or indirectly, outdated.

**Source formats.** Each source records its format, inferred from the file extension when added:

| Format | Files | Read by |
|---|---|---|
| CSV | `.csv`, `.txt` | DuckDB `read_csv` (and an independent reader in the reference engine) |
| Parquet | `.parquet`, `.pq` | adopted with core types (32-bit numbers widen) |
| JSON | `.json` (array of objects), `.jsonl`, `.ndjson` | DuckDB `read_json`; nested objects and lists become text |
| Spreadsheet | `.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.ods` | calamine, in Rust: DuckDB's Excel extension would be downloaded at run time, which ADR-0001 rules out |

A spreadsheet source reads one sheet, stored by name (the first sheet when added), with the first non-empty row as column names; column types are inferred like CSV. Error cells and empty cells are missing values, and fully empty rows are skipped. A source's key includes its format and sheet (CSV keys are unchanged), so choosing another sheet outdates the steps that read it.

**Renaming a source** updates every step input that names it, and the code of steps that read it: in SQL, the table references and qualified columns that DuckDB's parser locates (aliases, plain columns, strings and comments that spell the same word are untouched; a query with its own CTE of that name is left alone; only SQL that does not parse falls back to a token scan); in Python, `dfs["…"]` and `dfs.get("…")`. Python code the user had approved stays approved when only the name in it changes; code waiting for review keeps waiting.

**Inspectable SQL is a graph of CTEs, one per step, named by step id.** Custom SQL that names another input by id works the same in the combined query as when run alone. A join's SQL lists columns, so it needs its inputs' schemas. Until its inputs have run, the SQL tab says so instead of showing a guess.

**`analysis.json` format 2.** It uses `"sources": [{id, path}]` and `"inputs": [...]` per step. Format 1 is still read: its `source` gets the id `source`, and each step's `input` becomes a one-item list. Run records list every source with its hash, and older records with one `source` still load.

## Interface

- The pipeline shows one tree per source. A step sits under its main input, and a step that reads more says so on its row ("with regions.csv").
- The join editor has three parts: which table to combine with, which rows to keep (three described choices), and the conditions, each a pair of columns with a comparison in words ("is on or after / at least"). It explains how columns are named and flags pairs whose types can't be compared before the user saves.
- The append editor lists the tables in order (add, remove, at least two), with *remove repeated rows* and *record which table each row came from*, and says which columns not every table has.
- A source's page shows its format, lets the user rename it (the name SQL and Python use) and, for spreadsheets, choose the sheet.
- SQL and Python steps list what else they read and can add or remove inputs. The contract text names each input as the code sees it.

## Not done yet

- **Join conditions on computed expressions** (`a.day + INTERVAL 7 DAY >= b.day`): add a custom SQL step, or compute the column first.
- **Database sources** (Postgres, SQLite) and remote files. DuckDB's scanners for these are extensions that would be downloaded at run time.
- **Renaming steps.** Step ids are generated; labels can be changed freely.
- **A node canvas.** The tree with "with …" lines covers two-input steps. A canvas becomes more useful when graphs get wide (several joins feeding one step).

## Options not chosen

- **Second input inside the join's definition** (`Join { right: ... }`), instead of step inputs. That was simpler at first, but every consumer of the graph (planner, staleness, review gating, UI) would need a special case. One `inputs` list keeps the graph uniform.
- **Joins as custom SQL only.** That pushes the most common combining task out of the visual mode, which the briefs aim to avoid.
- **Keep both key columns in the output.** They're equal by construction, so keeping both only adds noise, and in a full join the left copy is missing for unmatched right rows.
