//! Renders the analysis notebook (document order) with each step's code or
//! inspectable SQL, its state, and a bounded result preview.
//!
//! This is a review/export rendering for the slice. The desktop UI will render
//! the same cells interactively. `jupyter.rs` exports a runnable `.ipynb`.

use crate::model::{Cell, StepKind};
use crate::session::{Output, Session, StepState};
use crate::Error;

const PREVIEW_ROWS: u64 = 10;

pub fn render_markdown(session: &mut Session) -> Result<String, Error> {
    let analysis = session.analysis.clone();
    let mut out = format!("# {}\n\n", analysis.name);

    // Source profiles: types and missing values.
    for src in &analysis.sources {
        let source = session.source_dataset(&src.id)?;
        let named = if analysis.sources.len() > 1 {
            format!(" `{}`", src.id)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "**Source{named}:** `{}`, {} rows\n\n",
            src.path, source.rows
        ));
        out.push_str(
            "| column | type | missing | distinct | min | max |\n|---|---|---|---|---|---|\n",
        );
        for p in session.source_profile(&src.id)? {
            let fmt = |v: &Option<crate::table::Value>| {
                v.as_ref().map(|v| v.to_string()).unwrap_or_default()
            };
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
    }

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
                    StepKind::Sql { .. } => {
                        "custom SQL — not editable with visual controls".to_string()
                    }
                    StepKind::Python { .. } => {
                        "custom Python — not editable with visual controls".to_string()
                    }
                    StepKind::R { .. } => "custom R — not editable with visual controls".to_string(),
                    k => format!("visual {}", k.type_name()),
                };
                out.push_str(&format!(
                    "### Step `{}`{}\n\n_{mode} · reads `{}` · state: **{}**_\n\n",
                    step.id,
                    step.label
                        .as_ref()
                        .map(|l| format!(" — {l}"))
                        .unwrap_or_default(),
                    step.inputs.join("`, `"),
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
                        let names = step.inputs.join("`, `");
                        out.push_str(&if step.inputs.len() == 1 {
                            "Input `df` → output `result`\n\n```python\n".to_string()
                        } else {
                            format!("Inputs `df` and `dfs` (`{names}`) → output `result`\n\n```python\n")
                        });
                        out.push_str(code.trim());
                        out.push_str("\n```\n\n");
                    }
                    StepKind::R { code, .. } => {
                        let names = step.inputs.join("`, `");
                        out.push_str(&if step.inputs.len() == 1 {
                            "Input `df` → output `result`\n\n```r\n".to_string()
                        } else {
                            format!("Inputs `df` and `dfs` (`{names}`) → output `result`\n\n```r\n")
                        });
                        out.push_str(code.trim());
                        out.push_str("\n```\n\n");
                    }
                    StepKind::Chart { .. } => {}
                    _ => {
                        // A join's SQL needs its inputs' columns; until they have
                        // run there is nothing executable to show.
                        if let Ok(sql) = session.pipeline_sql(step_id) {
                            out.push_str("<details><summary>Executable SQL</summary>\n\n```sql\n");
                            out.push_str(&sql);
                            out.push_str("\n```\n\n</details>\n\n");
                        }
                    }
                }
                match session.output(step_id).cloned() {
                    Some(Output::Table(ds)) => {
                        let page = session.preview(step_id, 0, PREVIEW_ROWS)?;
                        out.push_str(&page.preview(ds.rows));
                        out.push('\n');
                    }
                    Some(Output::Chart(_)) => {
                        out.push_str(&format!(
                            "Chart specification: `charts/{step_id}.vl.json`\n\n"
                        ));
                    }
                    None => {}
                }
            }
        }
    }
    Ok(out)
}
