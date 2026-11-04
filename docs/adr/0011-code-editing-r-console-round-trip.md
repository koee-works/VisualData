# ADR-0011: Code editing, R steps, a console beside code steps, and round trips with Jupyter and RStudio

- **Status:** Accepted (2026-11-03)
- **Related:** ADR-0001 (child processes, local-first), ADR-0002 (cache keys), ADR-0004 (code review), ADR-0007 (linter), ADR-0009 (notebook export)

## Context

The next audience is data scientists. Most of them work in RStudio or Jupyter, and they judge a tool by how writing code feels. Before this ADR the app fell short in six ways:

- Code was typed into plain text boxes.
- There was no R at all.
- Each try ran the whole step again, reloading its inputs.
- There were no plots or printed output to look at while working.
- Errors were not placed on their line.
- There was no way to bring code into the user's own tools and back.

## Decision

### A real code editor

CodeMirror 6 replaces every text box that holds code: SQL, Python and R steps, formulas, check rules and database queries. It was chosen over Monaco because it is smaller and works well with Svelte.

- **Features:** colouring, bracket matching, multiple cursors, find and replace, comment toggle, and optional Vim keys.
- **Completion knows what the step reads:**
  - `df["…"]`, `dfs["…"]` and `params[…]` in Python;
  - `df$…`, `dfs$…`, `params$…` and the dplyr verbs in R;
  - table and column names and `$parameters` in SQL;
  - columns, `@parameters` and the formula functions in formulas.
- **The formula function list** lives in `formula-functions.json`. A Rust test keeps it equal to the engine's list.
- **Keys follow the tools people come from:**

  | Key | Action | Source |
  |---|---|---|
  | Ctrl+Enter | Run the line or selection in the console; the cursor moves on | RStudio |
  | Ctrl+Shift+Enter | Run all the code in the console | RStudio |
  | Shift+Enter | Save and run the step | Jupyter's "run cell" |
  | Alt+− | Type `<-` | RStudio |
  | Ctrl+Shift+M | Type `|>` | RStudio |

  In SQL, both Ctrl+Enter and Shift+Enter save and run the step.
- **Run errors are placed on their line** and underlined. The line comes from:
  - Python: the traceback line, plus the quoted name to find the column;
  - DuckDB: `LINE n` and the caret beneath it, used only when DuckDB did not shorten the line;
  - R: the top-level expression that failed.

### R steps

`StepKind::R` sits beside SQL and Python with the same contract: `df`, `dfs` by id, `params`, and assign to `result`.

- **It runs as `Rscript --vanilla` in a child process**, with a cleared environment, a private folder, a timeout, cancellation and a memory limit. This is the same lifecycle isolation as Python. It is not a sandbox.
- **Data crosses as CSV plus a file of exact column types, so base R is enough.** The Arrow package for R is often missing, and it could not be installed here.
  - Numbers are written with 15 significant digits, or 17 when 15 would not read back exactly.
  - Missing values are written as `\N`, and empty text stays text.
  - Timestamps are UTC with microseconds.
  - Whole numbers become R integers when every value fits; past about 2.1 billion they become doubles.
- **The user's package libraries are passed through,** found once with their normal environment. That is where RStudio installs packages, so `library(dplyr)` works.
  - Package start-up banners are hidden.
  - Warnings are kept with their line.
- **Like Python, R steps are reviewed when they arrive from elsewhere.** Code the user writes in the app is approved as it is saved.
- **The cache key includes R's version and the versions of the reported packages.**
- **Elsewhere R steps behave like Python:** renames of `dfs$id`, the notebook text, and Jupyter export. The exported notebook embeds the app's own R wrapper, and `tests/jupyter.rs` checks that it reproduces the step exactly.
- **An R installation dialog** shows the version, libraries and packages, and lets the user choose another `Rscript`. The choice is saved per user, and `VDA_R` overrides it.

### A console beside each code step

A console is a child process that keeps the step's inputs loaded between runs, so trying a line costs only the line. In tests a short line answers in 0.3 ms in Python and about 50 ms in R.

- **It uses its own small protocol, not a Jupyter kernel.** Requests are framed on stdin, and each reply is one marked line on stdout. The language side is `console.py` and `console.R`.
  - It needs nothing beyond the Python or R the user already has. ipykernel was not installed here, and many analysts lack it.
  - What the code prints is captured into the reply, so it cannot be mistaken for one.
- **Each run returns:**
  - printed output, messages and warnings;
  - the last value: a data frame as a table, anything else as text, like Jupyter's last expression or the R console;
  - the error and its line;
  - plots: matplotlib with the Agg backend, or a PNG device per run in R;
  - an Environment listing, in RStudio's words for R ("4 obs. of 2 variables").
- **Interrupt** sends SIGINT. The code stops and the console keeps its state; this is tested in both languages. If the code does not stop within three seconds, the console is closed.
- **The step is still what counts.** After the step runs, the console writes its own `result` and the two are compared, ignoring row order. If they differ, the user is told that the console may hold something the code no longer makes. This answers the familiar notebook problem of hidden state.
- **One console is open at a time**, for the step being edited. It closes when another project opens.

### Round trips with Jupyter and RStudio

**Open in Jupyter** writes a Python step as a notebook:
- The first code cell is tagged `vda-setup` and loads the step's real inputs and parameters.
- The remaining code cells are the step's code.

**Open in RStudio** writes an R step as an `.R` file:
- The first line sources a setup that loads `df`, `dfs` and `params` into the session.
- The step's code follows a marker line.

Both files open in the computer's app for that file type.

The app watches only files it wrote in the project's `.work/edit/` folder:
- **An edit saved there is the user's own.** It updates the step without review, and the step becomes outdated.
- **An edit made in the app is written back** to the file.
- **Code arriving in `analysis.json` from elsewhere,** such as a colleague's change, still waits for review.

## Consequences

- R users can keep their packages and habits, and Python users keep Jupyter, while the analysis keeps one definition with its caching, history and review.
- R steps run base R's CSV reader on every run. That is slower than Arrow on very large inputs, but they typically follow an aggregate.
- The console runs code the user types with their permissions, as steps do (ADR-0004's limits apply).

## Not done yet

- Quarto (`.qmd`) or R Markdown export of the whole analysis.
- Using Arrow for R data exchange when the package is installed.
- Keeping plots drawn during a saved step's own run (chart steps cover saved charts).
- Language servers (Pyright, the R languageserver) for hover help and type checking. Ruff for lint and format.
- Interrupting the console on Windows; it is closed instead.
- Importing edits from an exported whole-analysis notebook (only single steps round-trip).
