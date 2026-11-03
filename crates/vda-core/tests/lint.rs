//! The linter: problems found without running, and warnings from the last run.

use std::fs;
use std::path::{Path, PathBuf};
use vda_core::duck::DuckDbEngine;
use vda_core::lint::{Diagnostic, Linter, Severity};
use vda_core::model::*;
use vda_core::python::PythonRunner;
use vda_core::session::Session;

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-lint-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue");
    fs::copy(samples.join("sales.csv"), dir.join("sales.csv")).unwrap();
    // A regions file with East twice, so joins on region multiply rows.
    fs::write(dir.join("regions.csv"), "region,manager\nEast,Priya\nEast,Sam\nWest,Diego\nNorth,Tom\nSouth,Lena\n").unwrap();
    dir
}

fn step(id: &str, inputs: &[&str], kind: StepKind) -> Step {
    Step { id: id.into(), inputs: inputs.iter().map(|s| s.to_string()).collect(), label: None, kind }
}

fn session(dir: &Path, steps: Vec<Step>) -> Session {
    let a = Analysis {
        parameters: vec![],
        name: "lint".into(),
        sources: vec![Source::primary("sales.csv"), Source::new("regions", "regions.csv")],
        cells: steps.iter().map(|s| Cell::Step { step_id: s.id.clone() }).collect(),
        steps,
    };
    Session::new(
        a,
        dir,
        Box::new(DuckDbEngine::new().unwrap()),
        Some(PythonRunner::new(vda_core::python::default_interpreter(), dir.join(".work"))),
    )
    .unwrap()
}

fn find<'a>(ds: &'a [Diagnostic], code: &str, step: &str) -> &'a Diagnostic {
    ds.iter()
        .find(|d| d.code == code && d.step.as_deref() == Some(step))
        .unwrap_or_else(|| panic!("no `{code}` for `{step}` in {ds:#?}"))
}

fn sql(s: &str) -> StepKind {
    StepKind::Sql { sql: s.into() }
}

fn python(code: &str) -> StepKind {
    StepKind::Python { code: code.into(), timeout_ms: 30_000 }
}

fn derive(name: &str, formula: &str) -> StepKind {
    StepKind::Derive { columns: vec![Derived { name: name.into(), formula: formula.into() }] }
}

#[test]
fn problems_are_found_before_anything_runs() {
    let dir = project("static");
    let mut s = session(
        &dir,
        vec![
            // Drops `region`, which the next step groups by.
            step("pick", &["source"], StepKind::Select { columns: vec![Pick { column: "revenue".into(), rename: None }] }),
            step(
                "by_region",
                &["pick"],
                StepKind::Aggregate { group_by: vec!["region".into()], measures: vec![] },
            ),
            step("calc", &["source"], derive("total", "units * [unit price]")),
            step("q1", &["source"], sql("SELEC * FROM input")),
            step("q2", &["source"], sql("SELECT * FROM input JOIN regions USING (region)")),
            step("py1", &["source"], python("result = df[\n")),
            step("py2", &["source"], python("out = df.copy()")),
            step("py3", &["source"], python("for i, row in df.iterrows():\n    pass\nresult = df")),
        ],
    );
    let ds = Linter::new().unwrap().lint(&mut s);

    let missing = find(&ds, "invalid-step", "by_region");
    assert_eq!(missing.severity, Severity::Error);
    assert!(missing.message.contains("column `region` not found (available: revenue)"), "{}", missing.message);

    let f = find(&ds, "formula", "calc");
    assert!(f.message.contains("no column `unit price`"), "{}", f.message);
    assert_eq!(f.field.as_deref(), Some("columns.0.formula"));

    assert!(find(&ds, "sql", "q1").message.contains("does not parse"));
    let t = find(&ds, "sql-table", "q2");
    assert!(t.message.contains("reads `regions`, which is not one of this step's inputs; add `regions` as an input"), "{}", t.message);

    let syntax = find(&ds, "python-syntax", "py1");
    assert_eq!(syntax.line, Some(1));
    assert!(find(&ds, "python-result", "py2").message.contains("never assigns"));
    let lp = find(&ds, "row-loop", "py3");
    assert_eq!((lp.severity, lp.line), (Severity::Warning, Some(1)));

    // Errors come first.
    let first_non_error = ds.iter().position(|d| d.severity != Severity::Error).unwrap_or(ds.len());
    assert!(ds[first_non_error..].iter().all(|d| d.severity != Severity::Error));
}

#[test]
fn the_last_run_explains_surprising_results() {
    let dir = project("data");
    let mut s = session(
        &dir,
        vec![
            step(
                "joined",
                &["source", "regions"],
                StepKind::Join { how: JoinKind::Left, on: vec![JoinKey::equal("region", "region")] },
            ),
            step(
                "none",
                &["source"],
                StepKind::Filter { predicates: vec![Predicate { param: None, column: "units".into(), op: CmpOp::Gt, value: vda_core::table::Value::Int(1000) }] },
            ),
            step("ratio", &["source"], derive("per_unit", "revenue / (units - units)")),
            step("bars", &["source"], StepKind::Chart { mark: Mark::Bar, x: "order_id".into(), y: "revenue".into(), title: None, color: None, summary: None }),
            step("withmissing", &["regions"], derive("product", "if(region = \"North\", format(1 / 0, 0), \"p\")")),
            step("wide", &["withmissing"], StepKind::Pivot { rows: vec![], column: "product".into(), value: None, func: AggFunc::Count }),
            step(
                "running",
                &["source"],
                StepKind::Window {
                    partition_by: vec![],
                    order_by: vec![],
                    columns: vec![
                        WindowColumn { name: "total".into(), func: WindowFunc::RunningSum, column: Some("revenue".into()), n: None },
                        WindowColumn { name: "share".into(), func: WindowFunc::ShareOfTotal, column: Some("revenue".into()), n: None },
                    ],
                },
            ),
        ],
    );
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{:#?}", run);
    let ds = Linter::new().unwrap().lint(&mut s);

    let j = find(&ds, "join-multiplies", "joined");
    assert!(j.from_last_run && j.message.contains("gave 80 rows from 60"), "{}", j.message);
    assert!(find(&ds, "no-rows", "none").message.contains("kept none of 60 rows"));
    let m = find(&ds, "all-missing", "ratio");
    assert_eq!(m.field.as_deref(), Some("columns.0.formula"));
    assert!(find(&ds, "chart-categories", "bars").message.contains("draws 60 bars"));
    assert!(ds.iter().all(|d| d.code != "unused"), "every step is in the notebook");
    let p = find(&ds, "pivot-missing", "wide");
    assert!(p.message.starts_with("1 of 5 rows have no `product`"), "{}", p.message);
    let w = find(&ds, "window-order", "running");
    assert_eq!(w.severity, Severity::Info);
    assert!(w.message.starts_with("`total` follows the rows' current order"), "{}", w.message);

    // After an edit: the result is outdated, and data warnings wait for a new run.
    if let StepKind::Join { how, .. } = &mut s.analysis.step_mut("joined").unwrap().kind {
        *how = JoinKind::Inner;
    }
    let ds = Linter::new().unwrap().lint(&mut s);
    assert!(ds.iter().any(|d| d.code == "outdated" && d.message.starts_with("1 result is outdated")));
    assert!(ds.iter().all(|d| !(d.code == "join-multiplies" && d.step.as_deref() == Some("joined"))));
}

#[test]
fn failures_unused_steps_and_drafts() {
    let dir = project("misc");
    let mut s = session(&dir, vec![step("boom", &["source"], sql("SELECT nope FROM input"))]);
    s.analysis.steps.push(step("lonely", &["source"], StepKind::Sort { by: vec![], limit: Some(3) }));
    s.run_all().unwrap();
    let mut linter = Linter::new().unwrap();
    let ds = linter.lint(&mut s);
    assert!(find(&ds, "failed", "boom").message.contains("nope"));
    assert_eq!(find(&ds, "unused", "lonely").severity, Severity::Info);

    // A draft is checked without being saved, against its inputs' columns.
    let draft = step("lonely", &["source"], derive("x", "if(units > 1, \"many\", 1)"));
    let d = linter.check_draft(&mut s, &draft);
    assert_eq!(d.len(), 1, "{d:#?}");
    assert!(d[0].message.contains("mixes text and a whole number"));
    assert!(matches!(s.analysis.step("lonely").unwrap().kind, StepKind::Sort { .. }), "nothing was saved");
    let d = linter.check_draft(&mut s, &step("lonely", &["source"], derive("x", "1 +")));
    assert_eq!(d[0].column, None);
    let d = linter.check_draft(&mut s, &step("lonely", &["source"], derive("x", "units $ 2")));
    assert_eq!(d[0].column, Some(7), "{:?}", d[0]);
}
