# Executive Brief — Visual Data Analysis Software

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
