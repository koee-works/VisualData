# ADR-0010: Undo and history, comments, statistical tests, the Python environment, database sources and scheduled refresh

- **Status:** Accepted (2026-11-03)
- **Related:** ADR-0001 (reference engine as oracle; local-first), ADR-0002 (cache keys), ADR-0004 (code review and SQL sandbox), ADR-0009 (notebook export)

## Context

With the transforms, charts and checks in place, the remaining gaps were about working with an analysis over time and with other people:

- a wrong edit could not be taken back;
- there was no place for a question about a step;
- tests of significance needed Python;
- the Python interpreter was whatever `python3` happened to be;
- data that lives in a database had to be exported to a file first;
- refreshing a report meant opening the app.

## Decision

### Undo, redo and history

Every saved change already goes through one function in the app layer (`App::commit`). That function now records each change in two places.

- **Undo and redo** cover this session, in memory, for the last 100 changes.
  - Undo and redo are themselves saved changes.
  - Results are kept by key (ADR-0002), so undoing an edit makes the earlier results current again without rerunning.
- **History** is kept in the project's `history/` folder: one file per saved version, the last 500.
  - Versions outlive the session and travel with the project.
  - Restoring a version is a change like any other, so it can be undone.
  - A restore is refused if a source file it reads is no longer in the folder, since history keeps definitions, not data.
- **Each change is described by comparing the versions before and after**, not by each command: "Added Big orders", "Renamed source `regions` to `targets`", "Changed the query reading shop.sqlite", "Restored version 3 (Deleted …)". Commands added later are described without extra work.
- **Code review still applies (ADR-0004).** Approvals are kept by code fingerprint, so undoing back to code the user has not approved asks for review again, and redoing to code they wrote stays approved.

### Comments

Comments sit on a step, a source or the analysis, with replies.

- They are stored in the project's `comments.json` and travel with it.
- They are not part of the analysis definition: undo never removes a comment, and comments never outdate a result.
- A deleted step's comments are kept, and come back if the deletion is undone. A renamed source's comments follow it, through undo as well.
- The author is the computer's user name. Anyone can reply to or resolve a comment; only its author can delete it.
- Open comments show as a count on pipeline nodes and in exported reports.

### Statistical tests (`test` step)

| Test | Columns | Method |
|---|---|---|
| Compare two averages | a number, by a column with exactly two groups | Welch's t-test |
| Compare averages across groups | a number, by a grouping column | One-way analysis of variance |
| Are two categories related? | two category columns | Chi-square test of independence, no continuity correction; Cramér's V |
| Do two numbers move together? | two number columns | Pearson's r, with its t-test |

**How it is computed:**
- As with checks and charts (ADR-0009), the engines produce the summaries: counts, means and standard deviations per group, cross-tab counts, and sums of products centred on the means (a second pass, for precision).
- These are ordinary visual steps that both engines already agree on.
- Only the last step is new code: distribution functions (`stats.rs`: the regularized incomplete beta and gamma functions), turning the summaries into a statistic and a p-value.

**How it is checked:** `tests/stat_tests.rs` runs all four on data with missing values and unequal spreads. It compares both engines, and compares the results with SciPy to 1e-9 relative (p-values to 1e-8, correlation's to 1e-6).

**The result** is one row: the statistic, its degrees of freedom, the p-value, and a sentence reading it at the usual 5% level, for example "No clear difference in the average of units among the 4 groups of region (p = 0.344)". It is a table later steps can read.

**Validation explains what each test needs:** exactly two groups for a t-test (with the groups found), numbers or categories, and decimals grouped with `bucket()` first.

The exported Jupyter notebook computes the same tests with SciPy. `tests/jupyter.rs` runs it and compares every column, including the sentence.

### The Python environment

- **Viewing it:** the app shows the interpreter Python steps use, its exact executable and version, and which useful packages it has: pandas and pyarrow (required), plus numpy, scipy, statsmodels, scikit-learn, matplotlib and duckdb.
  - A missing required package is reported with the `pip install` command that fixes it.
- **Choosing another:** the app offers interpreters it finds: the project's `.venv`, the active virtual environment, conda and pyenv environments, and `python3` on the PATH. A path can also be typed.
  - The choice is checked before it is accepted, and saved per user in `settings.json` beside the approvals file.
  - `VDA_PYTHON`, when set, wins.
- **Effect on results:** the interpreter and package versions are already part of every Python step's key, so changing interpreter outdates exactly the Python steps and what reads them.

### Database sources

Sources can be SQLite or DuckDB database files, read as a table (the first one by default) or as one query.

- **Recognized** by extension, or by content for `.db` files.
- **Copied into the project's saved data** when read, like any other source.
- **Fingerprinted** by the file's content hash plus the table or query, so a changed database outdates exactly what reads it. A different query is a different copy. (Testing found this missing; it is fixed and covered.)
- **Queries are held to the rules of SQL steps (ADR-0004):**
  - **DuckDB files** are opened read-only on a separate connection that can read no other file and write only the result, and the query must be a single SELECT.
  - **SQLite files** are opened read-only, with attaching other databases disabled, and the statement must be read-only.
  - `tests/databases.rs` checks that deleting, attaching, copying out and reading other files are refused, and that nothing changed.
- **SQLite needed a new library:** `rusqlite` with SQLite compiled in, because DuckDB's SQLite extension would have to be downloaded at run time. SQLite rows are converted in memory (up to 20 million rows); DuckDB files stream.
- **Types follow the database:** declared types and values decide, so `'007'` stays text and `DATE` columns holding ISO text become dates.

### Scheduled refresh (`vda-run`)

A refresh is a command, `vda-run <project> [--export <step>=<file>]... [--notebook <file>]`. The computer's own scheduler runs it (cron, launchd or Task Scheduler), rather than a scheduler inside the app, which would only run while the app is open.

- **What it does:** re-reads the sources, runs what is outdated, records the run with trigger "scheduled" (shown in the app's run log), then writes the exports.
- **Safe by default:**
  - Exports are written only when every step succeeded, so a file never mixes old and new results.
  - Python code runs only if this user has approved it.
  - One refresh of a project runs at a time (`runs/refresh.lock`; a lock older than six hours is treated as abandoned).
- **Exit codes:** 0 all current; 1 a step failed; 2 bad project or arguments; 3 code waits for review.
- **In the app**, **Refresh on a schedule** writes the scheduler entry for this computer, which the user adds themselves, so nothing is installed behind their back:
  - a crontab line on Linux;
  - a launchd agent on macOS;
  - a `schtasks` command on Windows.

*Found while testing:* DuckDB cannot read a CSV whose line endings change partway (CRLF, then LF), and its error listed its whole search space in about 30 lines. The import now explains the likely causes in one sentence.

## Consequences

- The project folder gains `history/`, `comments.json` and, after scheduled runs, `runs/refresh.lock` (while running) and `runs/scheduled.log`. All are plain JSON or text.
- A packaged desktop build must ship `vda-run` beside the app (Tauri's `externalBin`). The app finds it there, or on the PATH. Packaging is not yet set up.
- Not included:
  - database servers such as Postgres and MySQL, which need credentials, network access, and somewhere to keep the secrets;
  - editing comments;
  - comparing two saved versions side by side;
  - multiple-comparison corrections and non-parametric tests (Mann–Whitney, Kruskal–Wallis), which are natural next options for the test step;
  - a schedule that the app installs itself.
