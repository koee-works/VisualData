//! Renders the analysis notebook (document order) with each step's code or
//! inspectable SQL, its state, and a bounded result preview.
//!
//! This is a review/export rendering for the slice. The desktop UI will render
//! the same cells interactively. `.ipynb` export is NOT IMPLEMENTED.

use crate::compile;
use crate::model::{Cell, StepKind};
use crate::session::{Output, Session, StepState};
use crate::Error;

const PREVIEW_ROWS: usize = 10;

pub fn render_markdown(session: &mut Session) -> Result<String, Error> {
    let analysis = session.analysis.clone();
    let mut out = format!("# {}\n\n", analysis.name);

    // Source profile: types and missing values.
    let source = session.source_table()?.clone();
    out.push_str(&format!("**Source:** `{}`, {} rows\n\n", analysis.source.path, source.rows.len()));
    out.push_str("| column | type | missing | distinct | min | max |\n|---|---|---|---|---|---|\n");
    for p in source.profile() {
        let fmt = |v: &Option<crate::table::Value>| v.as_ref().map(|v| v.to_string()).unwrap_or_default();
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            p.name,
            p.dtype.name(),
            p.nulls,
            p.distinct,
            fmt(&p.min),
            fmt(&p.max)
        ));
    }
    out.push('\n');

    for cell in &analysis.cells {
        match cell {
            Cell::Markdown { text } => {
                out.push_str(text.trim());
                out.push_str("\n\n");
            }
            Cell::Step { step_id } => {
                let step = analysis.step(step_id).unwrap();
                let state = session.state(step_id)?;
                let mode = match &step.kind {
                    StepKind::Sql { .. } => "custom SQL — not editable with visual controls".to_string(),
                    StepKind::Python { .. } => "custom Python — not editable with visual controls".to_string(),
                    k => format!("visual {}", k.type_name()),
                };
                out.push_str(&format!(
                    "### Step `{}`{}\n\n_{mode} · reads `{}` · state: **{}**_\n\n",
                    step.id,
                    step.label.as_ref().map(|l| format!(" — {l}")).unwrap_or_default(),
                    step.input,
                    state.label()
                ));
                match &state {
                    StepState::Failed(e) | StepState::Blocked(e) => {
                        out.push_str(&format!("> **{}:** {}\n\n", state.label(), e))
                    }
                    StepState::Stale => out.push_str(
                        "> **Outdated:** this result was produced before an upstream change. Rerun to update it.\n\n",
                    ),
                    _ => {}
                }
                match &step.kind {
                    StepKind::Python { code, .. } => {
                        out.push_str("Input `df` → output `result`\n\n```python\n");
                        out.push_str(code.trim());
                        out.push_str("\n```\n\n");
                    }
                    StepKind::Chart { .. } => {}
                    _ => {
                        let sql = compile::pipeline_sql(&analysis, step_id)?;
                        out.push_str("<details><summary>Executable SQL</summary>\n\n```sql\n");
                        out.push_str(&sql);
                        out.push_str("\n```\n\n</details>\n\n");
                    }
                }
                match session.output(step_id) {
                    Some(Output::Table(t)) => {
                        out.push_str(&t.preview(PREVIEW_ROWS));
                        out.push('\n');
                    }
                    Some(Output::Chart(_)) => {
                        out.push_str(&format!("Chart specification: `charts/{step_id}.vl.json`\n\n"));
                    }
                    None => {}
                }
            }
        }
    }
    Ok(out)
}
