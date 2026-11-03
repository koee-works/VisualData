# ADR-0004: Reviewing received code, sandboxing SQL

- **Status:** Accepted (2026-11-02)
- **Related:** ADR-0001 (local-first desktop; projects are folders people share), ADR-0003 (desktop frontend)

## Context

A project is a folder, and folders get shared: emailed, put on a drive, checked into git. Opening one should be as safe as opening a spreadsheet. Today it is not: a Python step runs as the user, with their files, credentials and network, and a custom SQL step can read any file DuckDB can open (`read_csv('~/.ssh/id_rsa')`) or write one (`COPY ... TO`). Pressing *Run analysis* on a received project would run whatever its author wrote.

The two kinds of code differ. Python cannot be contained by this app without an OS sandbox per platform, which is out of scope for now. DuckDB, on the other hand, has settings that confine it.

## Decision

**Python: nothing runs until the user approves the exact code.**

- An approval is the SHA-256 of the step's code. Changing any character makes it new code that needs approval again.
- Approvals are kept per user, outside every project (`~/.local/share/vda/approvals.json` on Linux, the platform's equivalent elsewhere, or `$VDA_APPROVALS`). A project cannot carry its own approvals, so copying a folder never copies trust. The file holds fingerprints and times only, never code.
- Code the user writes or edits in the app is approved as it is saved: writing it is reviewing it. Saving only a step's label or timeout leaves unreviewed code unreviewed.
- When a project with unapproved Python opens, a dialog shows each step's code in full with what it reads, and says plainly what Python can do. *Not now* is always available. The project stays fully usable: visual and SQL steps run; the unapproved step reports *needs review*, and steps after it are skipped with "waiting for your review".
- Saved results are data, not code. A result already in the project whose cache key matches is shown without approval, because showing it runs nothing.

**SQL: sandboxed, so no approval is needed.**

- Each project's DuckDB connection may only touch the project folder, DuckDB's spill directory, and the source file when it is kept elsewhere (`allowed_directories`, `allowed_paths`, `enable_external_access = false`), and then the configuration is locked (`lock_configuration = true`), so no query can lift it.
- A custom SQL step must be exactly one read-only query (`SELECT`, `WITH` or `FROM`). This is checked with DuckDB's own parser (`json_serialize_sql`) before the query is embedded in anything, so `COPY`, `ATTACH`, `INSTALL`, `SET`, `PRAGMA`, second statements and injection through a trailing `;` are refused with a message saying why. The JSON extension is compiled in for this; no extension is ever downloaded (ADR-0001).

## Consequences

- Sharing a project works without new steps for the recipient beyond reading the code once.
- Approvals are per user and per machine. Moving to a new computer means approving again; that is accepted for now.
- The review is only as good as the reader. The dialog is written for people who are not programmers, and says what is at stake rather than asking them to judge code quality.
- Visual steps compile to SQL inside the same sandbox, so a crafted visual step in `analysis.json` gains nothing (identifiers and literals are quoted by the compiler, and the sandbox would contain it anyway).

## Not done yet

- **Contain Python itself** (no network, read-only filesystem outside the project) with an OS sandbox: seccomp/Landlock on Linux, `sandbox-exec` on macOS, AppContainer on Windows. That would make review a second line of defence rather than the only one.
- **Revoking an approval** from the UI. `Approvals::revoke` exists in the core.
- **Signed projects**, so a team can trust code from colleagues without reading every change.

## Options not chosen

- **Approve a whole project.** Simpler, but an approved project that later changes (a `git pull`) would run new code silently. Approving code by fingerprint catches every change.
- **Store approvals in the project.** Anyone who can write the folder could pre-approve their own code.
- **Ask before every run.** People learn to click through prompts that appear every time; asking once per new piece of code keeps the question meaningful.
- **Review SQL as well.** Unnecessary once SQL cannot leave the project, and it would teach people to approve without reading.
