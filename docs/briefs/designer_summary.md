# Designer Brief — Visual Data Analysis Software

**Corrected product definition:** An integrated data-analysis application that lets users explore, transform, analyze, and visualize data through a mix of visual interactions, code, and notebooks. Users should be able to choose the most useful way to work at each step without losing the context of their analysis.

The earlier generic project/task-management description was the wrong product framing. Projects may organize analyses, but task CRUD is not the purpose of this application.

## Context and confidence

- **Confirmed in the current request:** Visual data-analysis software combining visual, coding, and notebook workflows.
- **Recovered supporting context:** Earlier discussions covered DuckDB and CSV analysis, notebook environments, visual Python/SQL tools, and R visualization in Jupyter.[cite:136][cite:139][cite:138] These establish relevant interests, not a previously approved stack or complete specification.
- **Recommended below, not remembered commitments:** A shared analysis model, SQL/Python-first execution, linked visual/code/notebook views, reproducible runs, and a bounded first end-to-end workflow.
- **Still undecided:** Desktop/local versus hosted deployment; collaboration and tenancy; exact libraries; supported languages beyond the initial scope; AI features; target dataset sizes and performance budgets. No Obsidian integration, particular competitor, or unrelated prior SaaS architecture is assumed.

Version 0.2.0, dated 2026-11-02. These briefs supersede the earlier audience summaries. The previous full hand-off package has **not** been regenerated and should not be treated as the authoritative domain specification.

## Designer summary

### Experience goal

Design an analytical workspace where people can **see the data, understand the transformations, use code when useful, and explain the result** without losing their place. The central challenge is continuity between visual exploration, coding, and notebook storytelling—not designing a project-management shell.

A proposed journey is: open/import data → inspect columns, types, missing values, and distributions → prepare or transform → analyze with visual tools or code → inspect results → create charts → document and save the analysis.

### Proposed workspace

- **Data/source browser:** Datasets, fields, types, source status, and reusable outputs.
- **Analysis area:** Visual operations or a workflow graph; use a node canvas only if research shows it helps users understand dependencies.
- **Code surface:** SQL/Python source, diagnostics, explicit input/output bindings, and run controls.
- **Notebook surface:** Narrative cells alongside executable steps and result/chart references, with visible execution and freshness state.
- **Result inspector:** Preview table, profile, chart, errors, lineage, and the distinction between sampled and complete results.
- **Run/status area:** Progress, cancel, resource/error feedback, and rerun guidance.

Panels, tabs, or linked views are proposed layout options—not a requirement to display everything simultaneously. Use progressive disclosure so visual-first users are not overwhelmed and code-first analysts are not blocked.

### Critical interaction rules

**Mode switches preserve context.** Keep the selected dataset, step, result, and relevant filters understandable when moving between visual controls, code, and notebooks. Showing generated SQL should help users understand a transformation rather than silently create an unrelated copy.

**Custom code has an honest boundary.** Show whether code remains editable through visual controls. If it cannot be represented visually, label it as a custom-code step and explain its inputs/outputs. Do not imply arbitrary code can always be converted back into visual blocks.

**Execution is part of the design.** Distinguish preview from full run, execution order from document order, fresh results from stale ones, and cancellation from failure. Editing an upstream step should visibly affect dependent results. Avoid presenting old charts as current.

**Analytical meaning must remain visible.** Make type conversion, null handling, aggregation, join behavior, units, and sample size inspectable. Chart defaults need analyst review; use explicit labels and do not turn a technically valid chart into an unsupported analytical claim.

### Where departments come in

- **Product:** Confirms target analytical jobs, language, workflow priority, and scope.
- **Analysts/data specialists:** Review operations, chart choices, interpretation, and whether output is useful and mathematically correct.
- **Frontend:** Co-designs grids, editors, visual controls, notebook interactions, focus behavior, and performance constraints.
- **Backend/runtime engineering:** Explains available operations, execution semantics, cancellation, errors, and conversion boundaries before states are finalized.
- **QA:** Converts these states into analytical task, keyboard, screen-reader, stale-result, and recovery scenarios.
- **Security/platform:** Reviews source permissions, code trust, rich outputs, resource limits, and credential visibility.
- **Support:** Validates onboarding, error explanations, and diagnostic hand-off.

**Design acceptance:** A representative user can complete the mixed workflow, explain what each step did, distinguish current from outdated output, and reopen/rerun the analysis. Deliver screen/state specifications, component variants, accessibility behavior, content, and implementation review—not just a canvas mockup.

## Shared team acceptance

Use this proposed scenario as the team's first product-level test:

1. Import a dataset and inspect its schema/profile.
2. Apply a visual transformation and inspect its executable meaning.
3. Add a custom code step using the prior result.
4. Visualize the output and explain it in a notebook.
5. Save, reopen, and replay with recorded inputs/environment.
6. Change an upstream step; show stale downstream results until rerun.
7. Encounter a controlled failure; diagnose, cancel or correct, and recover without losing the analysis.

**Product** approves the user outcome; **design** validates comprehension; **engineering** delivers the connected system; **analysts and QA** validate analytical correctness; **platform/security** validate safe execution; **support/service ownership** validates diagnosability and recovery. These responsibilities overlap throughout discovery, implementation, and release rather than occurring as isolated departmental hand-offs.
