# ADR-0006: Formulas and core transforms

- **Status:** Accepted (2026-11-03)
- **Related:** ADR-0001 (reference engine as oracle), ADR-0004 (sandbox), ADR-0005 (multi-input steps)

## Context

Analysts kept dropping into custom SQL for everyday work: calculated columns, keeping and renaming columns, sorting with a top N, medians and distinct counts, and getting a result out of the app. Each of these is a visual operation elsewhere, and in this app custom SQL is never turned back into visual controls. So every detour costs the analysis its visual form from that step on.

## Decision

### Formulas

A **Calculate columns** step (`derive`) adds or replaces columns, each defined by a formula. Formulas are a small language of their own, not SQL. It is parsed and type-checked in Rust against the input's columns, compiled to DuckDB SQL, and evaluated independently by the reference engine. `tests/equivalence.rs` runs about 95 formulas through both engines on data with missing values, negative numbers, zeros, pre-1970 dates and an integer near the 64-bit limit.

- **Syntax:**
  - Column names, or `[Name with spaces]`.
  - Numbers, `"text"` or `'text'`, `true`, `false`, `missing`.
  - `+ - * / %`, `&` (join text), comparisons, `and or not`, `is missing` / `is not missing`.
- **Functions:**
  - Logic: `if`, `coalesce`.
  - Numbers: `round floor ceil abs sqrt power`.
  - Text: `upper lower trim length contains starts_with ends_with replace substr concat`.
  - Types: `text format number integer date`.
  - Dates: `year quarter month day weekday hour minute date_trunc days_between add_days`.
- **Errors say where and what:**
  - "unexpected `price` at character 6; write names with spaces as [Unit price]"
  - "if(...) mixes text and a whole number"
  - "`+` needs numbers (join text with &)"
- **Each formula sees the columns before it**, including earlier ones in the same step. A formula named like an existing column replaces it in place, which is how missing values are filled (`coalesce(region, "Unknown")`) and types changed (`number(code)`).

**Semantics chosen where SQL engines differ or surprise.** Each rule is enforced in the generated SQL, not left to DuckDB's defaults:

| Case | Rule | Why |
|---|---|---|
| `x / 0`, `x % 0` | missing | DuckDB gives `inf`/`NaN` |
| decimal results that are not finite (`sqrt(-1)`, overflowing `power`) | missing | no `inf`/`NaN` ever reaches a result |
| integer `+ - *` past 64 bits | error | not a silent wrap |
| `/` | always a decimal | `7 / 2` is 3.5 |
| `%` on decimals | sign of the dividend, like integers | DuckDB's `fmod()` follows the divisor; `%` is used instead |
| `round(x, n)` | half away from zero | the same in both engines by construction |
| `integer(2.5)` | 3 (rounds like `round`) | DuckDB's double→integer cast rounds half to even |
| `date_trunc` of a date | a date | DuckDB returns a timestamp |
| literals | typed `BIGINT`/`DOUBLE` | DuckDB reads `2.5` as an exact decimal and small integers as 32-bit |
| `&` vs `concat` | `&` is missing if a side is; `concat` skips missing | both are useful; the difference is documented in the editor |
| `if` with a missing condition | the else branch | SQL `CASE` behaviour |
| `text()` of a decimal | rejected; use `format(x, decimals)` | float-to-text formatting differs between engines |

Known differences, documented rather than hidden: case changes of a few special characters (`ß` upper-cases to `ẞ` in DuckDB and `SS` in Rust), and `length` of text with combining marks.

### Other transforms

- **Choose and rename columns** (`select`): keeps the listed columns in order, optionally renamed. Duplicate output names are rejected.
- **Sort rows, keep the top** (`sort`): any number of keys, ascending or descending. Missing values go last in both directions, and ties keep input order (row numbers make the order total). An optional limit makes it a top-N step; a limit alone keeps the first N rows.
- **More aggregates:** median, any percentile (P1–P99, linear interpolation), distinct count, and sample standard deviation (missing for fewer than two values). They are stored as `MEDIAN`, `P90`, `COUNT_DISTINCT`, `STDDEV`.

All three are visual steps, so their SQL appears in the step's SQL tab. A calculate step's SQL appears once its input has run, because typing needs the input's columns.

### Export

Any source or **current** result can be exported to CSV, Excel (.xlsx) or Parquet. An outdated result is refused with the reason, so a file never silently disagrees with its step.

- Export runs on its own DuckDB connection, not the analysis engine's. That one is confined to the project folder (ADR-0004), and an export is the user's action with SQL the app writes.
- Files are written beside the target and renamed into place.
- Excel is written by `rust_xlsxwriter` in constant-memory mode: dates and times as Excel dates, integers beyond 2^53 as text so they stay exact, a bold frozen header row. Results over Excel's 1,048,575 rows are refused, with a pointer to CSV or Parquet.
- `tests/export.rs` checks that each format reads back to the same table.

## Not done yet (planned next)

- Window calculations (running totals, rank, previous value), removing duplicates by key, pivot/unpivot, binning.
- Exporting charts and the notebook.
- Live checking of formulas while typing. Today they are checked on save, with the message shown at the top of the editor.

## Options not chosen

- **SQL expressions in the formula box.** That would expose DuckDB's surprises (`7/2`, `inf`, implicit casts, 32-bit literals) and every function DuckDB has, including ones that read files. A small language is checkable, explainable and portable to another engine.
- **Excel-style formulas** (`=IF(A2>0,...)` with cell references). Analyses work on columns, not cells.
- **Export through the analysis engine.** It would need write access outside the project folder, which the sandbox exists to prevent.
