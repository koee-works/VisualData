# ADR-0012: A Clean step: finding missing values and filling them in

- **Status:** Accepted (2026-11-04)
- **Related:** ADR-0005 (multi-input steps), ADR-0006 (formulas and the cross-engine rule), ADR-0009 (column details, checks, notebook export)

## Context

Cleaning a data set usually goes like this:

1. Find the empty cells, and the values that really mean "missing" (`N/A`, `-999`, blank text).
2. Fill them some way: the mean or median, the most common value, a random known value, the value before or after, or a prediction from other columns.
3. Check which way did least harm.

Before this ADR, the app could count missing values in a column's details and flag rows with a Check step. Filling them meant writing `coalesce()` by hand in a formula, one column at a time, with no way to tell whether the result was any good.

## Decision

### One step, one rule per column

A **Clean** step lists the columns it cleans. For each one, it says:

- **What else counts as missing:**
  - the step's **markers**, shared by every column;
  - a **"missing when" formula**, such as `[age] < 0`;
  - for text columns, a **conversion** to numbers, whole numbers or dates. Values that don't convert count as missing.
- **How to fill:**

| Method | Fills with | Groups |
|---|---|---|
| Leave missing | Nothing: only find (and flag) them | |
| Mean, median | The average, or the middle value (interpolated as DuckDB's `median`); whole numbers are rounded | yes |
| Most common | The most frequent value; ties go to the smallest | yes |
| A fixed value | A constant, read as the column's type | |
| Random known value | A known value drawn by a seeded 32-bit hash of the row number, so reruns give the same draws | yes |
| Previous, next | The last or next known value in a chosen order, then row order | yes |
| Regression | A least-squares line on chosen number columns, with an intercept | |
| Nearest neighbours | The `k` most similar rows by standardized distance: their average, or their most common value for text | |
| Drop the row | Removes rows where it's missing | |

Two more settings apply to the whole step. `flag` adds a `<column>_missing` column per cleaned column. `seed` fixes the random draws.

Each column is filled from the values *as found* in every column, never from another column's filled values. So the order of the rules doesn't matter.

### Same result in DuckDB and the reference engine

Following ADR-0006, the step has two implementations, and the cross-engine tests require them to agree: SQL for DuckDB (`clean::sql`) and Rust over a table (`clean::run`).

- **Finding is built from formulas.** Markers, conditions and conversions become formula expressions (`lower(trim(x)) = lower(trim("n/a"))`, `number(trim(x))`), which already mean the same in both engines.
- **Filling uses window aggregates and small joins**, written to match the Rust code exactly:
  - grouped `avg` and `median`;
  - the most common value, with a deterministic tie-break;
  - `last_value` or `first_value … IGNORE NULLS`;
  - the hash arithmetic for random draws.
- **Fitted fills are resolved first.** A regression is fitted, or the neighbours are found, in Rust, on the values found, before the step runs (`clean::resolve`). The step is then rewritten with internal fills: `Linear` holds the coefficients, and `Rows` holds a row number → value list, applied with a `VALUES` join. Both engines apply these the same way. The fitted forms are never saved.
- **The limits are stated in the interface:**
  - a regression is fitted on at most 50,000 rows, evenly spread;
  - neighbours compare with at most 5,000 rows and fill at most 20,000 values, and say so when there are more.

### What it found, and whether it did well

- **The report.** A run records, per column, how many values were missing and why (empty, marker, condition, conversion), how many were filled and how many are left. It also records how each was filled ("the median, 4.5", or the fitted line with its R²). The editor shows this under the column, and the console's Output tab shows it too.
- **Finding gaps before a run.** The editor lists the input's columns with their missing counts. It suggests markers it sees among the common values (`N/A`, `unknown`, `-999`, blank text). It also points out text columns that are really numbers, and reads them as numbers when they're added.
- **Comparing methods.** This tests the fill methods on the column's own data.
  1. Take the first 20,000 rows, as found.
  2. Hide 10% of the known values, at most 2,000, picked by the same seeded hash.
  3. Fill them with every method that suits the type, using the reference implementation, so it measures exactly what the step would do.
  4. Score each method:
     - numbers: the typical error (MAE) and RMSE;
     - other types: the share filled exactly right;
     - every type: the share of the true values' spread the fills keep. Filling every gap with one value keeps none.

  The closest method is marked, and **Use** applies it.

### Jupyter export

The exported notebook runs the same SQL. For fitted fills, it writes in the coefficients and neighbour values as the app fitted them to the step's input, with a comment saying so. `tests/jupyter.rs` reruns the notebook with Python and DuckDB and requires the same table.

## Consequences

- The app can now clean the usual exports, such as survey files with `-999` ages and incomes stored as text with "N/A". The checks are explicit and repeatable.
- Fitted fills depend on the data. If the input changes, the step reruns and refits, as any other step would.
- Not done yet:
  - categorical predictors (one-hot) for regression and neighbours;
  - multiple imputation;
  - filling from other columns' filled values (chained equations);
  - outlier detection beyond what a "missing when" formula can express.
