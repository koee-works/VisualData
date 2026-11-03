//! Parameters (one value used by several steps) and data quality checks.

use std::fs;
use std::path::{Path, PathBuf};
use vda_core::check::{evaluate, CheckReport};
use vda_core::duck::DuckDbEngine;
use vda_core::engine::{Engine, ReferenceEngine};
use vda_core::lint::{Linter, Severity};
use vda_core::model::*;
use vda_core::params::Parameter;
use vda_core::persist::{load_analysis, save_analysis};
use vda_core::python::PythonRunner;
use vda_core::session::{RunStatus, Session, StepState};
use vda_core::table::Value;

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-params-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue");
    fs::copy(samples.join("sales.csv"), dir.join("sales.csv")).unwrap();
    dir
}

fn step(id: &str, input: &str, kind: StepKind) -> Step {
    Step { id: id.into(), inputs: vec![input.into()], label: None, kind }
}

fn param(name: &str, value: Value) -> Parameter {
    Parameter { name: name.into(), value }
}

fn analysis(steps: Vec<Step>, parameters: Vec<Parameter>) -> Analysis {
    Analysis {
        name: "params".into(),
        sources: vec![Source::primary("sales.csv")],
        cells: steps.iter().map(|s| Cell::Step { step_id: s.id.clone() }).collect(),
        steps,
        parameters,
    }
}

fn session(dir: &Path, a: Analysis, engine: Box<dyn Engine>) -> Session {
    Session::new(a, dir, engine, Some(PythonRunner::new(vda_core::python::default_interpreter(), dir.join(".work")))).unwrap()
}

fn rows(s: &mut Session, id: &str) -> usize {
    s.table(id).unwrap().rows.len()
}

fn parameterized() -> Analysis {
    analysis(
        vec![
            step(
                "big",
                SOURCE_ID,
                StepKind::Filter {
                    predicates: vec![Predicate { column: "units".into(), op: CmpOp::Ge, value: Value::Null, param: Some("min_units".into()) }],
                },
            ),
            step("scaled", SOURCE_ID, StepKind::Derive { columns: vec![Derived { name: "adjusted".into(), formula: "revenue * @factor".into() }] }),
            step("east", SOURCE_ID, StepKind::Sql { sql: "SELECT * FROM input WHERE region = $region AND units >= $min_units".into() }),
            step(
                "py",
                SOURCE_ID,
                StepKind::Python {
                    code: "result = df[df['units'] >= params['min_units']].assign(seen=params['region'], since=str(params['since']))".into(),
                    timeout_ms: 30_000,
                },
            ),
        ],
        vec![
            param("min_units", Value::Int(5)),
            param("factor", Value::Float(1.5)),
            param("region", Value::Text("East".into())),
            param("since", Value::Date(vda_core::table::parse_date("2026-07-01").unwrap())),
        ],
    )
}

#[test]
fn parameters_reach_every_kind_of_step_and_outdate_only_their_readers() {
    let dir = project("reach");
    let mut s = session(&dir, parameterized(), Box::new(DuckDbEngine::new().unwrap()));
    s.approve_step("py").unwrap();
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:#?}");
    assert_eq!(rows(&mut s, "big"), 28);
    let py = s.table("py").unwrap();
    assert_eq!(py.rows.len(), 28);
    let seen = py.schema.index_of("seen").unwrap();
    let since = py.schema.index_of("since").unwrap();
    assert_eq!((py.rows[0][seen].clone(), py.rows[0][since].clone()), (Value::Text("East".into()), Value::Text("2026-07-01".into())));
    let scaled = s.table("scaled").unwrap();
    let (rev, adj) = (scaled.schema.index_of("revenue").unwrap(), scaled.schema.index_of("adjusted").unwrap());
    assert_eq!(scaled.rows[0][adj].as_f64().unwrap(), scaled.rows[0][rev].as_f64().unwrap() * 1.5);
    let east = s.table("east").unwrap();
    assert!(east.rows.iter().all(|r| r[2] == Value::Text("East".into())));

    // The SQL shown is the SQL run: values in place, the parameter named.
    let sql = s.pipeline_sql("east").unwrap();
    assert!(sql.contains("region = ('East' /* $region */) AND units >= (5 /* $min_units */)"), "{sql}");

    // A new threshold outdates the steps that use it; Python can read any
    // parameter, so it is outdated too. The formula step is not.
    s.analysis.parameters[0].value = Value::Int(8);
    for (id, state) in [("big", StepState::Stale), ("east", StepState::Stale), ("py", StepState::Stale), ("scaled", StepState::Fresh)] {
        assert_eq!(s.state(id).unwrap(), state, "{id}");
    }
    s.run_all().unwrap();
    assert!(rows(&mut s, "big") < 28);

    // Going back reuses the saved results.
    s.analysis.parameters[0].value = Value::Int(5);
    let again = s.run_all().unwrap();
    assert!(again.steps.iter().all(|r| matches!(r.status, RunStatus::Succeeded { cached: true })), "{again:#?}");
}

#[test]
fn parameters_mean_the_same_in_both_engines() {
    let mut a = parameterized();
    a.steps.retain(|s| matches!(s.kind, StepKind::Filter { .. } | StepKind::Derive { .. }));
    a.cells.clear();
    let (d1, d2) = (project("duck"), project("ref"));
    let mut duck = session(&d1, a.clone(), Box::new(DuckDbEngine::new().unwrap()));
    let mut reference = session(&d2, a, Box::new(ReferenceEngine::new()));
    assert!(duck.run_all().unwrap().succeeded());
    assert!(reference.run_all().unwrap().succeeded());
    for id in ["big", "scaled"] {
        assert_eq!(duck.table(id).unwrap(), reference.table(id).unwrap(), "{id}");
    }
}

#[test]
fn parameters_and_checks_are_saved_with_the_project() {
    let dir = project("save");
    let mut a = parameterized();
    a.steps.push(step(
        "checks",
        "big",
        StepKind::Check {
            stop: true,
            rules: vec![
                Rule::Expect { formula: "units >= @min_units".into() },
                Rule::Unique { columns: vec!["order_id".into()] },
                Rule::RowCount { min: Some(1), max: None },
            ],
        },
    ));
    save_analysis(&a, &dir).unwrap();
    assert_eq!(load_analysis(&dir).unwrap(), a);
}

#[test]
fn unknown_and_unused_parameters_are_reported() {
    let dir = project("lint");
    let mut a = parameterized();
    a.parameters.retain(|p| p.name != "region" && p.name != "factor");
    a.parameters.push(param("spare", Value::Bool(true)));
    let mut s = session(&dir, a, Box::new(DuckDbEngine::new().unwrap()));
    let ds = Linter::new().unwrap().lint(&mut s);
    let find = |code: &str, step: Option<&str>| {
        ds.iter().find(|d| d.code == code && d.step.as_deref() == step).unwrap_or_else(|| panic!("no {code} for {step:?}: {ds:#?}"))
    };
    assert!(find("unknown-parameter", Some("east")).message.contains("`$region`, which does not exist"));
    assert!(find("formula", Some("scaled")).message.contains("there is no parameter `@factor`"));
    let unused = find("unused-parameter", None);
    assert_eq!(unused.severity, Severity::Info);
    assert!(unused.message.contains("`spare`"));
    assert!(ds.iter().all(|d| !(d.code == "unused-parameter" && d.message.contains("min_units"))));
}

fn checks(stop: bool) -> StepKind {
    StepKind::Check {
        stop,
        rules: vec![
            Rule::Expect { formula: "units > 0".into() },
            Rule::Expect { formula: "revenue < @cap".into() },
            Rule::Unique { columns: vec!["order_id".into()] },
            Rule::Unique { columns: vec!["region".into(), "product".into()] },
            Rule::RowCount { min: Some(100), max: None },
            Rule::Expect { formula: "region is not missing".into() },
        ],
    }
}

#[test]
fn checks_report_what_fails_with_examples_and_agree_across_engines() {
    let dir = project("check");
    let a = analysis(vec![step("quality", SOURCE_ID, checks(false))], vec![param("cap", Value::Int(300))]);
    let s = session(&dir, a.clone(), Box::new(DuckDbEngine::new().unwrap()));
    let bound = vda_core::params::bind_step(&a.steps[0], &a.parameters);
    let mut reports: Vec<CheckReport> = Vec::new();
    for mut engine in [Box::new(DuckDbEngine::new().unwrap()) as Box<dyn Engine>, Box::new(ReferenceEngine::new())] {
        let src = engine.import_csv(&dir.join("sales.csv"), &dir.join(format!("in-{}.parquet", reports.len()))).unwrap();
        let mut n = 0;
        let mut scratch = || {
            n += 1;
            dir.join(format!("scratch-{}-{n}.parquet", reports.len()))
        };
        reports.push(evaluate(engine.as_mut(), &bound, &a.steps[0], &src, &mut scratch).unwrap());
    }
    assert_eq!(reports[0], reports[1], "the engines agree");
    drop(s);
    let r = &reports[0];
    let passed: Vec<bool> = r.results.iter().map(|x| x.passed).collect();
    // A missing value does not count as meeting a rule.
    assert_eq!(passed, [false, false, true, false, false, false]);
    assert_eq!(r.results[0].message, "`units > 0` is not true for 2 of 60 rows");
    assert_eq!(r.results[1].message, "`revenue < @cap` is not true for 7 of 60 rows");
    assert_eq!(r.results[1].examples.len(), 5);
    assert_eq!(r.results[1].columns[0], "order_id");
    assert!(r.results[3].message.starts_with("one row per region, product:"), "{}", r.results[3].message);
    assert_eq!(*r.results[3].columns.last().unwrap(), "rows");
    assert_eq!(r.results[4].message, "60 rows (at least 100 rows expected)");
    assert_eq!(r.results[5].message, "`region is not missing` is not true for 2 of 60 rows");
    assert!(r.results[5].examples[0].contains(&"missing".to_string()));
}

#[test]
fn a_check_that_warns_passes_rows_on_and_one_that_stops_blocks_what_follows() {
    let dir = project("modes");
    let after = step("after", "quality", StepKind::Sort { by: vec![], limit: Some(3) });
    let mut s = session(
        &dir,
        analysis(vec![step("quality", SOURCE_ID, checks(false)), after.clone()], vec![param("cap", Value::Int(300))]),
        Box::new(DuckDbEngine::new().unwrap()),
    );
    let run = s.run_all().unwrap();
    assert!(run.succeeded());
    assert_eq!(rows(&mut s, "quality"), 60);
    assert!(run.step("quality").unwrap().output.as_deref().unwrap().contains("FAILED: 60 rows (at least 100 rows expected)"));
    let ds = Linter::new().unwrap().lint(&mut s);
    let warned: Vec<_> = ds.iter().filter(|d| d.code == "check-failed").collect();
    assert_eq!(warned.len(), 5);
    assert_eq!(warned[1].field.as_deref(), Some("rules.1"));
    assert!(warned.iter().all(|d| d.from_last_run && d.severity == Severity::Warning));
    // Reopened: the report is still there.
    assert_eq!(s.check_report("quality").unwrap().failed().count(), 5);

    if let StepKind::Check { stop, .. } = &mut s.analysis.step_mut("quality").unwrap().kind {
        *stop = true;
    }
    let run = s.run_all().unwrap();
    match &run.step("quality").unwrap().status {
        RunStatus::Failed { error } => assert!(error.contains("5 of 6 checks failed: `units > 0` is not true for 2 of 60 rows; `revenue < @cap` is not true for 7 of 60 rows;"), "{error}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(run.step("after").unwrap().status, RunStatus::Skipped { .. }));

    // With only the rule the data meets, it passes and the next step runs.
    if let StepKind::Check { rules, .. } = &mut s.analysis.step_mut("quality").unwrap().kind {
        rules.retain(|r| matches!(r, Rule::Unique { columns } if columns.len() == 1));
    }
    assert!(s.run_all().unwrap().succeeded());
}

#[test]
fn check_rules_are_validated_before_running() {
    let dir = project("bad-rules");
    let mut s = session(
        &dir,
        analysis(
            vec![step(
                "quality",
                SOURCE_ID,
                StepKind::Check { stop: false, rules: vec![Rule::Expect { formula: "units * 2".into() }, Rule::RowCount { min: Some(5), max: Some(1) }] },
            )],
            vec![],
        ),
        Box::new(DuckDbEngine::new().unwrap()),
    );
    let ds = Linter::new().unwrap().lint(&mut s);
    let e = ds.iter().find(|d| d.code == "invalid-step").unwrap();
    assert!(e.message.contains("rule 1 must be true or false for each row (like `units > 0`), but gives whole number"), "{}", e.message);
}
