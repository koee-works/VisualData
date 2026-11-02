//! Runs the first engineering slice end to end and writes a project folder.
//!
//!     cargo run -p vda-core --example slice -- <output-dir>
//!
//! Output: analysis.json, runs/<id>.json, notebook.md, charts/share_chart.vl.json

use std::path::{Path, PathBuf};
use vda_core::compile::pipeline_sql;
use vda_core::demo::regional_revenue;
use vda_core::duck::DuckDbEngine;
use vda_core::persist::{load_analysis, save_analysis};
use vda_core::python::PythonRunner;
use vda_core::session::{replay, Output, Session};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out: PathBuf = std::env::args().nth(1).unwrap_or_else(|| "regional-revenue-project".into()).into();
    std::fs::create_dir_all(&out)?;
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue/sales.csv");
    std::fs::copy(sample, out.join("sales.csv"))?;
    let python = || Some(PythonRunner::new(vda_core::python::default_interpreter(), out.join(".work")));

    let mut session = Session::new(regional_revenue("sales.csv"), &out, Box::new(DuckDbEngine::new()?), python());
    println!("engine: duckdb {} | python: {}", DuckDbEngine::new()?.version(), session.python_fingerprint().unwrap_or("unavailable"));

    let run = session.run_all()?;
    for s in &run.steps {
        println!("  {:<12} {:<10} rows={:<4} {} ms", s.step_id, s.status.name(), s.rows.map(|r| r.to_string()).unwrap_or("-".into()), s.duration_ms);
    }
    println!("\nInspectable SQL for `by_region`:\n{}\n", pipeline_sql(&session.analysis, "by_region")?);

    save_analysis(&session.analysis, &out)?;
    let run_path = session.save_run(&run)?;
    if let Some(Output::Chart(spec)) = session.output("share_chart") {
        std::fs::create_dir_all(out.join("charts"))?;
        std::fs::write(out.join("charts/share_chart.vl.json"), spec.to_string_pretty())?;
    }
    std::fs::write(out.join("notebook.md"), vda_core::notebook::render_markdown(&mut session)?)?;

    // Reopen from disk and replay in a clean session.
    assert_eq!(load_analysis(&out)?, session.analysis);
    let report = replay(&out, &run, Box::new(DuckDbEngine::new()?), python())?;
    println!("replay of {}: {}", run_path.display(), if report.fully_reproduced() { "all steps reproduced" } else { "differences found" });
    for (id, outcome) in &report.steps {
        println!("  {id:<12} {outcome:?}");
    }
    println!("\nwrote {}", out.display());
    Ok(())
}
