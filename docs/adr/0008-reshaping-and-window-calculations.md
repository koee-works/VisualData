# ADR-0008: Reshaping and window calculations

- **Status:** Accepted (2026-11-03)
- **Related:** ADR-0001 (reference engine as oracle), ADR-0006 (formulas and core transforms), ADR-0007 (linter)

## Context

After the core transforms (ADR-0006), the most common reasons analysts still wrote custom SQL were:

- running totals, ranks, and comparisons with the previous row ("month over month");
- removing duplicates, often "the latest row per customer";
- reshaping between long and wide tables (a column per month, or months back into rows);
- grouping numbers into bands (ages 30–39) before summarizing.

These need window functions, `QUALIFY` and `PIVOT`, which are where hand-written SQL most often goes wrong: missing frames, ties, NULLs in partitions. As with every visual step, both engines implement the meaning, and the cross-engine tests check they agree.

## Decision

Four visual steps and one formula function. All keep the rule that a visual step's output order is defined, so cache keys and comparisons are stable.

### Window calculations (`window`)

The step adds columns calculated across rows, separately for each group (`partition_by`), in an order (`order_by`). Rows stay where they were in the input; only new columns are added.

| Calculation | Result | Notes |
|---|---|---|
| Running total | Same type as the column | Missing values are skipped; missing until the first value; integer overflow is an error |
| Row number | Whole number | 1, 2, 3, … in the order |
| Rank | Whole number | Ties in the order share a rank and the next one skips (1, 1, 3); with no order, every row is 1 |
| Previous / next value | Same type as the column | `n` rows back or ahead; missing at the edges |
| Moving average | Decimal | Over the current row and the `n − 1` before it, skipping missing values |
| Change from previous | Same numeric type | Integer overflow is an error |
| % change from previous | Decimal | Missing when the previous value is zero or missing |
| Share of total | Decimal | Value ÷ the group's total; missing when the total is zero |

Ties in the order keep input order (a hidden row number breaks them), except for rank. Missing values sort last in both directions. In SQL, every calculation uses an explicit frame (`ROWS BETWEEN …`), so DuckDB's default `RANGE` frame, which treats ties as one row, never applies.

New column names must not already exist. Replacing a column silently, as formulas do, would hide which value a later step sees.

The linter adds a note when an order-dependent calculation has no order. The result then depends on how earlier steps arranged the rows, which is defined but easy to forget.

### Keep one row per key (`dedupe`)

Rows with the same `keys` count as repeats. Missing key values match each other, as in `DISTINCT`. The first row in `order_by` is kept, with ties going to the earlier row. Kept rows stay in input order. With no keys, rows must match in every column. "The latest order per customer" is keys = customer, ordered by date, latest first.

### Unpivot (`unpivot`)

Each input row becomes one row per listed column. The other columns are kept, followed by a names column (text) and a values column. Missing values are kept: a missing measurement is not the same as no row. The listed columns must share a type. Whole numbers and decimals combine as decimals, and anything else is refused with the two types named. Turning values into text would make numbers sort and compare as text.

SQL is generated explicitly (a cross join with a `VALUES` list and `CASE`) rather than DuckDB's `UNPIVOT`, whose `INCLUDE NULLS` default and type rules would have to be matched.

### Pivot (`pivot`)

One column per value of `column`, one row per group of `rows` (sorted, missing last), each cell a summary of `value` (or the number of rows) for that group and value. Missing combinations are missing, or 0 for counts. Column order is value order.

- **The new columns are known only after the step runs.** The step reports no static schema, like custom SQL; the linter and editors use its last current result. Steps after it see the columns once it has run.
- **Only text, whole-number, true/false and date columns can be pivoted.** Their values have one written form, so column names are exact and the same in both engines. Decimals and timestamps are refused with a hint to group them first (`bucket()`, `date_trunc()`).
- **Missing pivot values are left out**, which is what DuckDB's `PIVOT` does. The linter warns from the last run with the number of rows dropped, and suggests filling them with `coalesce` first.
- **A value named like a row column gets `_1`**, as DuckDB names it; the reference engine does the same.
- **More than 100 new columns** gets a warning from the last run: usually a column with too many values was chosen.

DuckDB runs the pivot with `PIVOT … ON … USING … GROUP BY`, over a subquery that selects only the columns involved (otherwise `PIVOT` groups by every other column).

### `bucket(x, width)`

`bucket(x, width)` is the start of the `width`-wide band holding `x`, so `bucket(37, 10)` is 30 and `bucket(-1, 10)` is −10. It works on whole numbers when both arguments are whole, and on decimals otherwise. The width must be positive, or the result is missing. Combined with a summary or a pivot, it gives histograms and banded tables without custom SQL.

## Consequences

- `tests/reshape.rs` runs every calculation through both engines, on data with missing values in keys, orders and values, ties, a zero before a change, negative numbers and a repeated row, plus overflow and validation messages. It also runs the combined inspectable SQL of a dedupe → window → unpivot → pivot chain against the step-by-step results.
- Pivot is the first visual step whose columns depend on the data. Anything that needs columns before running (the editors, the linter, `pipeline_sql_with`) already handled custom SQL that way, so no new mechanism was needed.
- Pivoted column names come from data, so they can be awkward to use in formulas (`[2026-01-01]`). Renaming them is a Select step.
- Not included: windows over value ranges (the last 7 days rather than 7 rows), cumulative distinct counts, and pivoting several measures at once. Each can be added as an option of these steps.
