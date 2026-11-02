# Visual Data Analysis Software — Audience Briefs

**Corrected product definition:** An integrated data-analysis application that lets users explore, transform, analyze, and visualize data through a mix of visual interactions, code, and notebooks. Users should be able to choose the most useful way to work at each step without losing the context of their analysis.

The earlier generic project/task-management description was the wrong product framing. Projects may organize analyses, but task CRUD is not the purpose of this application.

## Context and confidence

- **Confirmed in the current request:** Visual data-analysis software combining visual, coding, and notebook workflows.
- **Recovered supporting context:** Earlier discussions covered DuckDB and CSV analysis, notebook environments, visual Python/SQL tools, and R visualization in Jupyter.[cite:136][cite:139][cite:138] These establish relevant interests, not a previously approved stack or complete specification.
- **Recommended below, not remembered commitments:** A shared analysis model, SQL/Python-first execution, linked visual/code/notebook views, reproducible runs, and a bounded first end-to-end workflow.
- **Still undecided:** Desktop/local versus hosted deployment; collaboration and tenancy; exact libraries; supported languages beyond the initial scope; AI features; target dataset sizes and performance budgets. No Obsidian integration, particular competitor, or unrelated prior SaaS architecture is assumed.

Version 0.2.0, dated 2026-11-02. These briefs supersede the earlier audience summaries. The previous full hand-off package has **not** been regenerated and should not be treated as the authoritative domain specification.

## Executive summary

### What the application is

A unified environment for doing data analysis—not simply a dashboard builder, a code editor, or a notebook host. A user can inspect a dataset visually, prepare it with guided operations, use code for a custom calculation, and explain the result in a notebook. The product goal is continuity across those activities, rather than forcing users to restart the analysis in separate tools.

The intended audience spans people who prefer visual exploration and analysts who need code-level control. Prior discussions of notebook environments, visual Python/SQL workflows, and R/Jupyter visualization support that direction; they do not establish validated market demand.[cite:136][cite:139][cite:138]

### Value and initial scope

The proposed value is less tool-switching, more transparent transformations, and easier reproduction and explanation of results. Measure that with task completion, successful replay, and whether another analyst can understand the work—not just the number of screens or charts shipped.

**Recommended first workflow:** Import a CSV → inspect types and missing values → visually filter or aggregate → inspect the corresponding SQL → add a custom Python step → chart the result → explain it with notebook text → save and rerun. This is proposed scope, not a claim that it already exists.

Keep the first release focused on this complete analytical workflow. Advanced modeling, many connectors, unrestricted plugin ecosystems, real-time co-editing, enterprise administration, and AI assistance should require explicit prioritization. AI is not assumed merely because the software assists analysis.

### Technical and delivery implications

The product needs a shared definition of an analysis, an execution layer for queries/code, and persistent records of inputs, steps, results, and runs. Visual controls, editors, and notebooks must operate on related analysis objects rather than become three disconnected mini-apps. Running user code also introduces a fundamentally different security and compute problem from a conventional CRUD application; Jupyter explicitly treats kernel execution as arbitrary code execution.[cite:143]

Do not carry forward the earlier 150-person-day estimate, generic CRUD latency targets, or Kubernetes recommendation as approved commitments. Re-estimate after proving cross-workflow continuity and choosing the execution/deployment model.

### Where departments come in

- **Product:** Defines analytical jobs, target users, supported operations, MVP boundaries, and success measures.
- **Design:** Makes visual/code/notebook transitions understandable; validates the experience with representative analysts.
- **Engineering:** Builds the shared model, editors, analytical execution, results, and reproducible persistence.
- **Data/analytics specialists:** Validate calculations, statistical assumptions, data types, examples, and expected analytical results.
- **QA:** Verifies result correctness, mode consistency, replay, stale-state handling, and cancellation—not merely UI behavior.
- **Platform and security:** Define execution isolation, data/credential access, resource limits, observability, and recovery appropriate to local or hosted operation.
- **Leadership and service/support owners:** Approve scope/cost, operational ownership, onboarding, and support expectations.

**First decision:** Approve a demonstrable mixed-workflow analysis before expanding the feature set or committing a launch date.

## Developer summary

### Core engineering goal

Build an analysis system with multiple coordinated interaction modes. The primary domain objects should be **Dataset/Source, Analysis, Step, Code or Cell, Visualization, Run, and Artifact**—not generic tasks. An analysis can be organized in a project, but data transformation and execution are the core domain.

### Recommended architecture

These are candidates to validate, not technology decisions recovered from the thread.

| Layer | Proposed responsibility and technology direction |
|---|---|
| Application UI | React/TypeScript; dataset grid/profile, visual operations, SQL/Python editor, notebook, charts, and run status |
| Analysis model | Versioned, structured steps with explicit inputs/outputs, parameters, dependency edges, schema information, and source bindings |
| Analytical runtime | DuckDB for relational/SQL work; an isolated Python runtime for custom analysis; R remains a possible extension, not assumed MVP scope |
| Interchange | Arrow/Parquet or explicit materialized artifacts between compatible execution boundaries; avoid shipping full tables through browser JSON |
| Control layer | API or local service for save/version/run/cancel/artifact operations; Node/TypeScript can serve this role without becoming the analytics engine |
| Persistence | Analysis metadata and run records separate from dataset/result storage; exact local versus hosted storage depends on deployment choice |

DuckDB's documented Python interfaces can read common data formats and query DataFrames and Arrow objects, making it a plausible SQL/Python bridge—not proof that the application's interoperability problem is solved.[cite:152][cite:145]

### Mode interoperability rules

1. **One analysis identity:** Visual operations, code steps, and notebook cells reference explicit dataset versions and result artifacts.
2. **Supported visual operations:** Compile an agreed operation model to SQL or another supported executable representation; compare results in tests.
3. **No fictional round-trip promise:** Arbitrary edited SQL/Python does not automatically become editable visual controls. Represent unsupported custom code as an explicit code step with declared inputs/outputs.
4. **Notebook compatibility is bounded:** Define the supported cell/output subset and import/export behavior. Jupyter notebooks carry source, outputs, metadata, and execution counts; file compatibility alone does not provide equivalent execution or dependency semantics.[cite:154]
5. **Visible execution state:** Track queued/running/succeeded/failed/cancelled and stale results. Invalidate downstream outputs when upstream inputs or steps change.
6. **Replay before reproducibility claims:** Record source versions/hashes where possible, step/code revisions, parameters, environment/dependencies, and random seeds where relevant. Replay from a clean runtime; disclose mutable external sources or nondeterministic dependencies.

For the MVP, prefer explicit registered inputs/outputs and documented execution order over attempting to infer every dependency in arbitrary user code. A stateful notebook session may exist, but clean replay is a separate acceptance condition.

### Safety, performance, and testing

Execute Python away from the application server's trust boundary. Restrict filesystem/network/secret access, cap CPU/memory/runtime, and provide cancellation and cleanup. A local child process separates lifecycle but is not by itself a security sandbox; hosted multi-user execution needs a deliberately reviewed isolation boundary. Treat imported notebook code and rich output as untrusted; Jupyter documents sanitization and output-trust concerns.[cite:143]

Bound preview size, virtualize tables, stream or page results, and label sampled versus full-data results. Key cached outputs by relevant source/step/environment revisions rather than a generic resource TTL. Define separate budgets for preview, query, Python run, chart rendering, and export after benchmarking representative data.

Test known analytical results, null/type/date semantics, row counts after joins, stale-output invalidation, cross-mode equivalence for supported operations, and clean replay. Add interruption, resource-exhaustion, malicious-code, and notebook-output tests appropriate to the execution model.

### Where departments come in

**Product + analysts** define valid operations and expected analytical semantics. **Design + frontend** agree how users inspect data, edit steps, understand execution, and recover from errors. **Frontend + backend/runtime engineers** agree schemas, run events, artifact formats, and mode boundaries. **QA + data specialists** own correctness fixtures and replay tests. **Platform + security** approve runtime isolation and quotas before accepting untrusted code. **Support** helps make run diagnostics and recovery actionable.

**First engineering slice:** One dataset through visual filtering, inspectable SQL, a Python step, a chart, notebook explanation, and saved clean replay. Mark every unimplemented conversion or format boundary explicitly.

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
