# ADR-0009: Charts, column details, checks, parameters and notebook export

- **Status:** Accepted (2026-11-03)
- **Related:** ADR-0001 (reference engine as oracle), ADR-0004 (code review), ADR-0006 (formulas), ADR-0007 (linter), ADR-0008 (reshaping)

## Context

The remaining gaps between this app and an analyst's everyday work were about *looking at* and *handing over* results, not transforming them:

- charts limited to bars, lines and points of raw rows (the first 5,000);
- a column profile limited to types, missing values, distinct counts and ranges;
- no way to state what the data must look like and be told when it does not;
- the same threshold or date typed into several steps;
- no way to share the notebook outside the app.

## Decision

Five features. The first three follow one rule from earlier slices: **anything computed over data is built from visual steps the engines already implement**. The cross-engine tests already hold those steps to the same results, so these features need no second implementation to keep DuckDB and the reference engine in agreement.

### Charts

- **Kinds:** bar, line, area, point, histogram and box plot.
- **Color:** bars, lines, areas and points can be split into series by a color column.
- **Summaries:** bars, lines, areas and points can show sum, average, median, count, minimum, maximum or distinct count per category.
- **Summaries cover every row.** They run as an aggregate step over the whole input before anything is inlined into the chart, so a chart of 50 million rows shows totals of all of them.
  - Histogram: a range aggregate picks a round bin width (about 20 bars), then `bucket()` and a count.
  - Box plot: minimum, quartiles, median and maximum per group.
- **The 5,000-row cap applies only to what is inlined**, and the title then says "first 5,000 of N groups (or rows)".
- Validation explains mismatches: a histogram or box plot needs a number column, color cannot repeat the bottom axis, and so on.

*Found while testing:* Vega-Lite 6 draws ranged bars (`x`/`x2`, which histograms use) with no width when `cornerRadiusEnd` is set in the config. The shared chart style no longer rounds bar ends. `chart.test.ts` would catch it coming back.

### Column details

Sources and step results show, beyond the basic profile:

- for numbers: mean, standard deviation, quartiles, zeros and negatives, and a 10-bar distribution;
- for text: blank values (empty or only spaces) and the shortest and longest lengths;
- for every type: the five most common values.

They come from calculate, aggregate, filter and sort steps run through the engine (`profile.rs`), and `tests/charts.rs` checks that both engines agree.

### Data quality checks (`check`)

A Check step holds rules and passes its input through unchanged.

| Rule | Means | Built from |
|---|---|---|
| Every row meets | A formula is true; a missing result counts as not met | Calculate `coalesce((formula), false)`, then filter |
| No two rows share | Key columns are unique | Count per key, then filter count > 1 |
| The number of rows is | Within optional bounds | Row count |

For each rule, the step records whether it passed, how many rows (or repeated keys) break it, and up to five examples. These show at the rule in the editor, in the console's Output, and in exported reports.

**Warn or stop:**
- *Warn* (the default) passes the rows on.
- *Stop* fails the step, so the steps after it wait, with a message listing the rules not met.

Rule formulas are checked like any formula before running.

### Parameters

Named values (number, text, true/false, date) set for the analysis.

- **Where they are used:**
  - formulas and check rules: `@name`;
  - filters: compare with a parameter instead of a fixed value;
  - SQL: `$name`, outside strings, quoted names, comments and dollar-quoted text;
  - Python: `params["name"]`.
- **Steps are bound before planning.** References are replaced by the values as literals. The engines, the inspectable SQL and cache keys only ever see bound steps. Both engines therefore agree by construction, and a new value outdates exactly the steps that use it.
- **Python can read any parameter.** A Python step's key includes all of them, so a new value outdates every Python step and what follows it. Projects without parameters keep their earlier keys.
- **Renaming a parameter** updates every formula, check rule, filter and SQL query that uses it. Python code is not changed, since that would ask for a new review (ADR-0004); the app names the Python steps to edit instead.
- **The linter reports** unknown references (before running) and unused parameters.
- Parameters are saved in `analysis.json` (`parameters`, an empty list when there are none). Files written before this field existed still open.

### Notebook export

From the notebook, **Export** offers:

- **Report:** one self-contained HTML page with the text cells, each step's first 20 rows, check results, charts drawn as SVG, and the code of SQL and Python steps.
  - It has no scripts, so it opens anywhere and prints as it is.
  - The UI builds it, because it already draws the charts.
  - Outdated and failed steps are marked.
- **Jupyter notebook:** an `.ipynb` file that reruns the analysis with DuckDB and pandas, outside the app.
  - Visual and SQL steps become the SQL the app runs, with parameters filled in, one DuckDB table per step.
  - Python steps get `df`, `dfs` and `params` as in the app.
  - Checks print their results, and a check that stops raises.
  - Text cells keep document order, and charts show their last specification.
  - `tests/jupyter.rs` executes the exported notebook and compares every step's result with the app's (CI installs DuckDB for Python for this).

With `VDA_SAVE_DIR` set, the app saves notebooks into that folder under their suggested names instead of asking. This is for scripted use and the end-to-end test: file dialogs cannot be answered under Xvfb, and Tauri does not let page scripts replace its `invoke` function. Only the file name of the suggestion is used, so it cannot point outside the folder.

## Consequences

- Every new data computation reuses steps that already have cross-engine coverage, so the oracle grows with no new evaluator code.
- Exported reports are snapshots: they show results as of the export and say when a step was outdated.
- The exported notebook needs DuckDB for Python, and reads the same source files from the project folder.
- Not included: faceted charts and dual axes; checks across two tables (for example, every key present in another table); parameters chosen from a list of a column's values; PDF export (print the report instead).
