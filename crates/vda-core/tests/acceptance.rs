//! The shared team acceptance scenario from the v0.2.0 briefs, steps 1–7,
//! run against the DuckDB engine and a real Python interpreter.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use vda_core::compile::pipeline_sql;
use vda_core::csv::read_table;
use vda_core::demo::regional_revenue;
use vda_core::duck::DuckDbEngine;
use vda_core::engine::{Engine, ReferenceEngine};
use vda_core::model::{CmpOp, Predicate, StepKind, SOURCE_ID};
use vda_core::persist::{load_analysis, save_analysis};
use vda_core::python::PythonRunner;
use vda_core::session::{replay, Output, ReplayOutcome, RunRecord, RunStatus, Session, StepState};
use vda_core::table::{DataType, Value};

const ALL: [&str; 4] = ["valid_orders", "by_region", "share", "share_chart"];

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-acceptance-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue/sales.csv");
    std::fs::copy(sample, dir.join("sales.csv")).unwrap();
    dir
}

fn python(dir: &Path) -> Option<PythonRunner> {
    Some(PythonRunner::new(vda_core::python::default_interpreter(), dir.join(".work")))
}

fn engine() -> Box<dyn Engine> {
    Box::new(DuckDbEngine::new().unwrap())
}

fn session(dir: &Path) -> Session {
    Session::new(regional_revenue("sales.csv"), dir, engine(), python(dir)).unwrap()
}

fn state(s: &Session, id: &str) -> StepState {
    s.state(id).unwrap()
}

#[test]
fn step1_import_and_profile() {
    let dir = project("profile");
    let mut s = session(&dir);
    let src = s.source_dataset(SOURCE_ID).unwrap();
    assert_eq!(src.rows, 60);
    let profile = s.source_profile(SOURCE_ID).unwrap();
    let col = |n: &str| profile.iter().find(|p| p.name == n).unwrap();
    assert_eq!(col("units").dtype, DataType::Int);
    assert_eq!(col("revenue").dtype, DataType::Float);
    assert_eq!(col("region").nulls, 2);
    assert_eq!(col("revenue").nulls, 2);
}

#[test]
fn step2_visual_transform_has_inspectable_sql_and_matches_pandas() {
    let dir = project("visual");
    let mut s = session(&dir);
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:#?}");

    let sql = pipeline_sql(&s.analysis, "by_region").unwrap();
    assert!(sql.contains("read_csv('sales.csv'"), "{sql}");
    assert!(sql.contains("WHERE \"region\" IS NOT NULL AND \"revenue\" >= 20"), "{sql}");
    assert!(sql.contains("GROUP BY \"region\""), "{sql}");

    // Independent oracle: the same transformation written directly in pandas.
    let oracle = Command::new(vda_core::python::default_interpreter())
        .arg("-I")
        .arg("-c")
        .arg(
            "import pandas as pd, sys\n\
             df = pd.read_csv(sys.argv[1])\n\
             df = df[df['region'].notna() & (df['revenue'] >= 20)]\n\
             g = df.groupby('region').agg(orders=('order_id','size'), revenue=('revenue','sum'), avg_unit_price=('unit_price','mean')).reset_index()\n\
             print(g.to_csv(index=False), end='')",
        )
        .arg(dir.join("sales.csv"))
        .output()
        .unwrap();
    assert!(oracle.status.success(), "{}", String::from_utf8_lossy(&oracle.stderr));
    let expected = read_table(&String::from_utf8(oracle.stdout).unwrap()).unwrap();
    let actual = &s.table("by_region").unwrap();
    assert_eq!(actual.rows.len(), expected.rows.len());
    for (a, e) in actual.rows.iter().zip(&expected.rows) {
        assert_eq!(a[0], e[0]);
        assert_eq!(a[1], e[1]);
        for i in 2..4 {
            let (x, y) = (a[i].as_f64().unwrap(), e[i].as_f64().unwrap());
            assert!((x - y).abs() < 1e-9, "column {i}: {x} vs {y}");
        }
    }
}

#[test]
fn step3_and_4_python_step_chart_and_notebook() {
    let dir = project("python");
    let mut s = session(&dir);
    assert!(s.run_all().unwrap().succeeded());

    let share = &s.table("share").unwrap();
    let total: f64 = share.column_values(share.schema.index_of("revenue_share").unwrap()).filter_map(Value::as_f64).sum();
    assert!((total - 1.0).abs() < 1e-3, "shares sum to {total}");

    match s.output("share_chart").unwrap() {
        Output::Chart(spec) => {
            assert_eq!(spec.get("mark").unwrap().get("type").unwrap().as_str(), Some("bar"));
            assert_eq!(spec.get("data").unwrap().get("values").unwrap().as_arr().unwrap().len(), 4);
        }
        other => panic!("expected chart, got {other:?}"),
    }

    let md = vda_core::notebook::render_markdown(&mut s).unwrap();
    for needle in [
        "## Question",
        "visual filter",
        "custom Python — not editable with visual controls",
        "state: **fresh**",
        "Executable SQL",
        "## Caveats",
    ] {
        assert!(md.contains(needle), "notebook missing `{needle}`:\n{md}");
    }
    // Document order is preserved: the Python explanation precedes its step.
    assert!(md.find("custom Python because").unwrap() < md.find("### Step `share`").unwrap());
}

#[test]
fn step5_save_reopen_and_clean_replay() {
    let dir = project("replay");
    let mut s = session(&dir);
    let run = s.run_all().unwrap();
    assert!(run.succeeded());
    save_analysis(&s.analysis, &dir).unwrap();
    let path = s.save_run(&run).unwrap();

    // Reopen from disk.
    let reopened = load_analysis(&dir).unwrap();
    assert_eq!(reopened, s.analysis);
    let recorded = RunRecord::from_json(&vda_core::json::Json::parse(&std::fs::read_to_string(path).unwrap()).unwrap()).unwrap();
    assert_eq!(recorded, run);

    // Replay in a fresh session with no cached results.
    let report = replay(&dir, &recorded, engine(), python(&dir)).unwrap();
    assert!(report.environment_changes.is_empty(), "{:?}", report.environment_changes);
    assert!(report.fully_reproduced(), "{:?}", report.steps);
    assert!(report.replay_run.steps.iter().all(|r| r.status == RunStatus::Succeeded { cached: false }));
}

#[test]
fn step6_upstream_change_marks_downstream_stale_until_rerun() {
    let dir = project("stale");
    let mut s = session(&dir);
    let first = s.run_all().unwrap();
    for id in ALL {
        assert_eq!(state(&s, id), StepState::Fresh);
    }

    // A second run reuses everything.
    let again = s.run_all().unwrap();
    assert!(again.steps.iter().all(|r| r.status == RunStatus::Succeeded { cached: true }));

    // Renaming a step is display-only and must not invalidate anything.
    s.analysis.step_mut("by_region").unwrap().label = Some("Renamed".into());
    assert_eq!(state(&s, "share_chart"), StepState::Fresh);

    // Tighten the filter: it and everything downstream become stale.
    if let StepKind::Filter { predicates } = &mut s.analysis.step_mut("valid_orders").unwrap().kind {
        predicates[1] = Predicate { column: "revenue".into(), op: CmpOp::Ge, value: Value::Int(100) };
    }
    for id in ALL {
        assert_eq!(state(&s, id), StepState::Stale, "{id}");
    }
    // The outdated result is still available for display, labelled stale.
    assert!(s.output("by_region").is_some());

    let rerun = s.run_all().unwrap();
    assert!(rerun.steps.iter().all(|r| r.status == RunStatus::Succeeded { cached: false }));
    for id in ALL {
        assert_eq!(state(&s, id), StepState::Fresh);
    }
    let orders_before = first.step("valid_orders").unwrap().rows.unwrap();
    assert!(rerun.step("valid_orders").unwrap().rows.unwrap() < orders_before);

    // Replaying the first recording now reports changed inputs, not divergence.
    save_analysis(&s.analysis, &dir).unwrap();
    let report = replay(&dir, &first, engine(), python(&dir)).unwrap();
    assert!(report.steps.iter().all(|(_, o)| *o == ReplayOutcome::InputsChanged), "{:?}", report.steps);

    // Changing the source file also invalidates everything.
    let mut csv = std::fs::read_to_string(dir.join("sales.csv")).unwrap();
    csv.push_str("61,2026-10-30,North,Pro,2,49.0,98.0\n");
    std::fs::write(dir.join("sales.csv"), csv).unwrap();
    assert_eq!(state(&s, "valid_orders"), StepState::Stale);
}

#[test]
fn step7_controlled_failure_diagnose_and_recover() {
    let dir = project("failure");
    let mut s = session(&dir);
    assert!(s.run_all().unwrap().succeeded());

    // Break the Python step.
    if let StepKind::Python { code, .. } = &mut s.analysis.step_mut("share").unwrap().kind {
        *code = "result = df.assign(revenue_share=df['revenu'] / 1)".into();
    }
    let run = s.run_all().unwrap();
    match &run.step("share").unwrap().status {
        RunStatus::Failed { error } => assert!(error.contains("KeyError") && error.contains("revenu"), "{error}"),
        other => panic!("expected failure, got {other:?}"),
    }
    assert!(matches!(run.step("share_chart").unwrap().status, RunStatus::Skipped { .. }));
    // Upstream work is untouched and still fresh.
    assert_eq!(state(&s, "by_region"), StepState::Fresh);
    assert!(matches!(state(&s, "share"), StepState::Failed(_)));
    assert!(matches!(state(&s, "share_chart"), StepState::Blocked(_)));
    // The previous result survives for reference.
    assert!(s.output("share").is_some());

    // Fix it and recover without losing the analysis.
    if let StepKind::Python { code, .. } = &mut s.analysis.step_mut("share").unwrap().kind {
        *code = vda_core::demo::PYTHON_SHARE.into();
    }
    let fixed = s.run_all().unwrap();
    assert!(fixed.succeeded());
    // The filter and aggregate were reused, not recomputed.
    assert_eq!(fixed.step("by_region").unwrap().status, RunStatus::Succeeded { cached: true });
    // The fixed code is identical to the original, so even the Python result
    // is recognised as current again.
    assert_eq!(state(&s, "share_chart"), StepState::Fresh);
}

#[test]
fn step7_cancel_a_long_running_step() {
    let dir = project("cancel");
    let mut s = session(&dir);
    if let StepKind::Python { code, .. } = &mut s.analysis.step_mut("share").unwrap().kind {
        *code = "import time\ntime.sleep(60)\nresult = df".into();
    }
    let cancel = s.cancel_handle();
    let t0 = Instant::now();
    let handle = std::thread::spawn(move || {
        // Give the filter and aggregate time to finish, then cancel.
        std::thread::sleep(Duration::from_millis(1200));
        cancel.store(true, Ordering::SeqCst);
    });
    let run = s.run_all().unwrap();
    handle.join().unwrap();
    assert!(t0.elapsed() < Duration::from_secs(15));
    assert_eq!(run.step("by_region").unwrap().status, RunStatus::Succeeded { cached: false });
    assert_eq!(run.step("share").unwrap().status, RunStatus::Cancelled);
    assert!(matches!(run.step("share_chart").unwrap().status, RunStatus::Skipped { .. }));
    assert_eq!(state(&s, "share"), StepState::Cancelled);
}

#[test]
fn custom_sql_step_runs_on_duckdb() {
    let dir = project("sql");
    let mut s = session(&dir);
    s.analysis.steps.push(vda_core::model::Step {
        id: "top".into(),
        inputs: vec!["share".into()],
        label: Some("Top two regions".into()),
        kind: StepKind::Sql {
            sql: "WITH ranked AS (SELECT *, row_number() OVER (ORDER BY revenue DESC) AS rank FROM input)\n\
                  SELECT region, revenue, revenue_share, rank FROM ranked WHERE rank <= 2 ORDER BY rank"
                .into(),
        },
    });
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:#?}");
    let top = &s.table("top").unwrap();
    assert_eq!(top.rows.len(), 2);
    assert_eq!(top.rows[0][0], Value::Text("East".into()));
    assert_eq!(top.schema.field("rank").unwrap().dtype, DataType::Int);
    // Custom SQL after a Python step: the inspectable SQL starts from the
    // Python step's materialized output and labels the step as custom.
    let sql = pipeline_sql(&s.analysis, "top").unwrap();
    assert!(sql.contains("materialized output of Python step `share`") && sql.contains("custom SQL"), "{sql}");

    // A broken query fails that step with DuckDB's diagnosis, nothing else.
    if let StepKind::Sql { sql } = &mut s.analysis.step_mut("top").unwrap().kind {
        *sql = "SELECT regoin FROM input".into();
    }
    let run = s.run_all().unwrap();
    match &run.step("top").unwrap().status {
        RunStatus::Failed { error } => assert!(error.contains("regoin") && error.contains("step `top`"), "{error}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(state(&s, "share_chart"), StepState::Fresh);
}

#[test]
fn reference_engine_still_rejects_custom_sql() {
    let dir = project("sql-ref");
    let mut s = Session::new(regional_revenue("sales.csv"), &dir, Box::new(ReferenceEngine::new()), python(&dir)).unwrap();
    s.analysis.steps.push(vda_core::model::Step {
        id: "top".into(),
        inputs: vec!["by_region".into()],
        label: None,
        kind: StepKind::Sql { sql: "SELECT * FROM input LIMIT 2".into() },
    });
    let run = s.run_all().unwrap();
    match &run.step("top").unwrap().status {
        RunStatus::Failed { error } => assert!(error.contains("needs the DuckDB engine"), "{error}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn switching_engines_marks_results_stale() {
    let dir = project("engine-switch");
    let mut s = session(&dir);
    let run = s.run_all().unwrap();
    assert!(run.succeeded());
    // The engine version is part of every key, so a run record from one engine
    // replayed on another reports changed inputs rather than a false match.
    save_analysis(&s.analysis, &dir).unwrap();
    let report = replay(&dir, &run, Box::new(ReferenceEngine::new()), python(&dir)).unwrap();
    assert!(report.environment_changes.iter().any(|c| c.starts_with("engine:")), "{:?}", report.environment_changes);
    assert!(report.steps.iter().all(|(_, o)| *o == ReplayOutcome::InputsChanged));
}

fn set_min_revenue(s: &mut Session, min: i64) {
    if let StepKind::Filter { predicates } = &mut s.analysis.step_mut("valid_orders").unwrap().kind {
        predicates[1] = Predicate { column: "revenue".into(), op: CmpOp::Ge, value: Value::Int(min) };
    }
}

#[test]
fn reopening_a_project_shows_saved_results_without_rerunning() {
    let dir = project("reopen");
    let mut s = session(&dir);
    assert!(s.run_all().unwrap().succeeded());
    save_analysis(&s.analysis, &dir).unwrap();

    let mut reopened = Session::open(&dir, engine(), python(&dir)).unwrap();
    for id in ALL {
        assert_eq!(state(&reopened, id), StepState::Fresh, "{id}");
        // Identical content after the round trip through Parquet, dates included.
        assert_eq!(reopened.output(id), s.output(id), "{id}");
    }
    let valid = reopened.output("valid_orders").unwrap().as_dataset().unwrap();
    assert_eq!(valid.schema.field("order_date").unwrap().dtype, DataType::Date);
    let run = reopened.run_all().unwrap();
    assert!(run.steps.iter().all(|r| r.status == RunStatus::Succeeded { cached: true }), "{run:#?}");
    assert!(run.warnings.is_empty());
}

#[test]
fn undoing_an_edit_reuses_the_earlier_saved_results() {
    let dir = project("undo");
    let mut s = session(&dir);
    s.run_all().unwrap();
    set_min_revenue(&mut s, 100);
    assert!(s.run_all().unwrap().steps.iter().all(|r| r.status == RunStatus::Succeeded { cached: false }));
    // Back to the original definition: the results in memory are now stale,
    // but the earlier ones are on disk and are reused, Python included.
    set_min_revenue(&mut s, 20);
    assert_eq!(state(&s, "share"), StepState::Stale);
    let t0 = Instant::now();
    let run = s.run_all().unwrap();
    assert!(run.steps.iter().all(|r| r.status == RunStatus::Succeeded { cached: true }), "{run:#?}");
    assert!(t0.elapsed() < Duration::from_secs(5));
}

#[test]
fn unreadable_saved_result_is_recomputed() {
    let dir = project("corrupt");
    let mut s = session(&dir);
    let first = s.run_all().unwrap();
    save_analysis(&s.analysis, &dir).unwrap();
    let key = &first.step("by_region").unwrap().cache_key;
    std::fs::write(dir.join("artifacts").join(format!("{key}.parquet")), b"not parquet").unwrap();

    let mut reopened = Session::open(&dir, engine(), python(&dir)).unwrap();
    assert_eq!(state(&reopened, "by_region"), StepState::NotRun);
    assert_eq!(state(&reopened, "share"), StepState::Fresh);
    let run = reopened.run_all().unwrap();
    assert_eq!(run.step("by_region").unwrap().status, RunStatus::Succeeded { cached: false });
    assert_eq!(run.step("by_region").unwrap().content_hash, first.step("by_region").unwrap().content_hash);
    assert_eq!(run.step("share").unwrap().status, RunStatus::Succeeded { cached: true });
}

#[test]
fn pruning_keeps_only_current_results() {
    let dir = project("prune");
    let mut s = session(&dir);
    s.run_all().unwrap();
    set_min_revenue(&mut s, 100);
    s.run_all().unwrap();
    // Step results only: not their metadata files, not the converted source.
    let files = || {
        std::fs::read_dir(dir.join("artifacts"))
            .unwrap()
            .filter_map(|e| e.unwrap().file_name().into_string().ok())
            .filter(|n| !n.ends_with(".meta.json") && !n.starts_with("source-"))
            .count()
    };
    assert_eq!(files(), 8);
    assert_eq!(s.prune_saved_results().unwrap(), 4);
    assert_eq!(files(), 4);
    for id in ALL {
        assert_eq!(state(&s, id), StepState::Fresh);
    }
}

#[test]
fn replay_ignores_saved_results() {
    let dir = project("replay-clean");
    let mut s = session(&dir);
    let run = s.run_all().unwrap();
    save_analysis(&s.analysis, &dir).unwrap();
    let report = replay(&dir, &run, engine(), python(&dir)).unwrap();
    assert!(report.fully_reproduced());
    assert!(report.replay_run.steps.iter().all(|r| r.status == RunStatus::Succeeded { cached: false }));
}
