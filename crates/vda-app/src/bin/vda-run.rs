//! vda-run: refresh an analysis without opening the app, for schedules.
//!
//!     vda-run <project-folder> [--export <step>=<file>]... [--notebook <file.ipynb>] [--quiet]
//!
//! Re-reads the sources, runs what is outdated, records the run (shown as
//! "scheduled" in the app's run log), then writes the exports. Python code
//! runs only if you have approved it in the app.
//!
//! Exit codes: 0 every result is current; 1 a step failed (or a check that
//! stops was not met); 2 the project or the arguments are wrong; 3 Python
//! code waits for your review.

use std::path::PathBuf;
use std::process::ExitCode;
use vda_app::App;

const USAGE: &str = "usage: vda-run <project-folder> [--export <step>=<file>]... [--notebook <file.ipynb>] [--quiet]

Refreshes the analysis: re-reads its sources, runs what is outdated, and writes
the exports (.csv, .xlsx or .parquet, from a step's or source's id). Python code
runs only if you have approved it in the app.

Exit codes: 0 all current, 1 a step failed, 2 bad project or arguments, 3 code waits for review.";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut project: Option<PathBuf> = None;
    let mut exports: Vec<(String, PathBuf)> = Vec::new();
    let mut notebook: Option<PathBuf> = None;
    let mut quiet = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--quiet" | "-q" => quiet = true,
            "--export" => match args.next().and_then(|v| v.split_once('=').map(|(s, f)| (s.to_string(), PathBuf::from(f)))) {
                Some(e) => exports.push(e),
                None => return usage("--export needs <step>=<file>"),
            },
            "--notebook" => match args.next() {
                Some(f) => notebook = Some(PathBuf::from(f)),
                None => return usage("--notebook needs a file"),
            },
            s if s.starts_with('-') => return usage(&format!("unknown option {s}")),
            s if project.is_none() => project = Some(PathBuf::from(s)),
            s => return usage(&format!("unexpected argument {s}")),
        }
    }
    let Some(project) = project else { return usage("name the project folder") };

    let started = std::time::Instant::now();
    let mut app = App::default();
    if let Err(e) = app.open_project(&project) {
        eprintln!("vda-run: cannot open {}: {}", project.display(), e.message);
        return ExitCode::from(2);
    }
    let mut on_step = |e: vda_app::StepEvent| {
        if !quiet {
            let what = match (e.status.as_str(), e.cached) {
                ("succeeded", true) => "reused".to_string(),
                ("succeeded", false) => format!("ran in {} ms", e.duration_ms),
                (s, _) => s.to_string(),
            };
            let rows = e.rows.map(|r| format!(", {r} rows")).unwrap_or_default();
            println!("  {}: {what}{rows}{}", e.step_id, e.detail.as_ref().map(|d| format!(": {d}")).unwrap_or_default());
        }
    };
    let report = match app.refresh(&exports, notebook.as_deref(), &mut on_step) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("vda-run: {}", e.message);
            return ExitCode::from(2);
        }
    };
    for (path, rows) in &report.exported {
        if !quiet {
            println!("  wrote {path}{}", if *rows > 0 { format!(" ({rows} rows)") } else { String::new() });
        }
    }
    let secs = started.elapsed().as_secs_f64();
    match report.exit_code() {
        0 => {
            if !quiet {
                println!("vda-run: {}: all results current ({} ran, {} reused, {secs:.1} s)", project.display(), report.ran, report.reused);
            }
        }
        1 => {
            for (step, why) in &report.failed {
                eprintln!("vda-run: step `{step}` failed: {why}");
            }
            eprintln!("vda-run: nothing was exported, so no file mixes old and new results");
        }
        _ => eprintln!(
            "vda-run: Python code in {} waits for your review; open the project in the app and approve it",
            report.needs_review.join(", ")
        ),
    }
    ExitCode::from(report.exit_code() as u8)
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("vda-run: {problem}\n\n{USAGE}");
    ExitCode::from(2)
}
