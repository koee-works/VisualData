# ADR-0007: Linter and console

- **Status:** Accepted (2026-11-03)
- **Related:** ADR-0004 (code review), ADR-0005 (multi-input steps), ADR-0006 (formulas)

## Context

Problems used to surface one at a time: as a banner when a step was saved, or as a failure after a run. There was nowhere to see everything wrong or questionable in an analysis, nothing warned about results that ran but were probably not what was meant, and what Python steps printed was thrown away.

## Decision

A **linter** in the core (`lint.rs`) and a **console** docked under the result, with three tabs.

### What is checked

**Without running anything** (errors stop a step from running):

| Check | How |
|---|---|
| A step cannot work on the columns it will receive (a column dropped or renamed upstream, a summary of a text column, a join key of the wrong type) | Columns are carried from the sources through every visual step by its compiler, and taken from current results of SQL and Python steps. So an upstream edit is reported at every step it breaks, before a run. |
| A formula does not parse or type-check | Reported at that formula, with the character it points at |
| SQL does not parse, or is not one read-only query | DuckDB's parser (the same check the sandbox uses) |
| SQL reads a table that is not one of the step's inputs | The tables in DuckDB's parse tree, less the query's own CTEs. The hint says to add it as an input when it exists in the analysis. |
| Python has a syntax error, or never assigns `result` | `ast.parse` in an isolated interpreter (`-I`, empty environment, five-second limit). Parsing runs no user code, so code waiting for review is checked too. Results are cached by the code's hash. |

**Warnings** (it runs, but probably not as meant):

- **From the code:** Python that works through rows one at a time (`iterrows`, `itertuples`, `apply(axis=1)`).
- **From the last run**, labelled as such and only for current results; results over 5 million rows are not profiled:
  - a join that gave more rows than its left side (repeated keys on the right)
  - a join or filter that kept no rows
  - a calculated column that is missing in every row (often a division by a column of zeros)
  - a bar chart with more than 50 bars

**Notes:**
- steps that nothing reads and the notebook does not show
- how many results are outdated
- code waiting for review

### The console

- **Problems** lists everything, errors first. Each item links to its step and part (formula 2, line 3, character 7).
  - Warnings and notes can be dismissed per step. Dismissals are saved in the project (`dismissed.json`), so an accepted many-to-many join does not keep coming back for anyone, and they can be shown again.
  - The tab label shows the counts; a step with an error says "Has a problem" in the pipeline.
- **Output** shows what each step printed in the current or last run.
- **Run log** lists recent runs from `runs/`, newest first: when, the outcome, what ran, what was reused, and per step its status, rows, time and reason.

### Checking while editing

The step being edited is sent to the linter as a draft, unsaved, 300 ms after the last change. Problems appear where they are:
- under the formula, with the position marked
- under SQL or Python code, with the line
- above the editor for the rest

This replaces "checked on save" from ADR-0006.

### Python output

The Python runner now keeps stdout and stderr (warnings, tracebacks), 64 KB each, also when the step fails. Output printed before an error is often what explains it.
- The wrapper flushes stdout before exiting. It exits with `os._exit` because of the out-of-memory hang fixed earlier, which would otherwise lose printed output.
- Output is stored per step in the run record and sent with each step's progress event.

## Not done yet

- Rules users can turn off for a whole project, or add.
- Checks for SQL beyond parsing and table names, such as columns a query reads that its input does not have. DuckDB's binder knows this, but needs the inputs registered.
- A warning that `SELECT *` feeds a step that depends on particular columns.

## Options not chosen

- **Running the steps to find problems.** That is slow on large data, and for unreviewed Python it would run code the user has not approved. Static checks plus the last run's results cover the common mistakes without either.
- **A Python linter (pyflakes, ruff) as a dependency.** It would need installing next to pandas in the user's environment. `ast` is always there, and the two Python checks that matter most here (syntax, `result`) need nothing else.
- **Problems only in the step editor.** That hides problems in steps you are not looking at, which are exactly the ones an upstream edit breaks.
