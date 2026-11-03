//! Analyses with several sources and steps that read several inputs: a visual
//! join, custom SQL reading inputs by id, Python reading `dfs`, staleness
//! across sources, reopening, and compatibility with format-1 projects.

use std::fs;
use std::path::{Path, PathBuf};
use vda_core::duck::DuckDbEngine;
use vda_core::engine::Engine;
use vda_core::hash::Sha256;
use vda_core::model::*;
use vda_core::persist::{load_analysis, save_analysis};
use vda_core::plan::{plan, validate, Fingerprints};
use vda_core::python::PythonRunner;
use vda_core::session::{Output, RunStatus, Session, StepState};
use vda_core::table::Value;

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-multi-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue");
    fs::copy(samples.join("sales.csv"), dir.join("sales.csv")).unwrap();
    fs::copy(samples.join("regions.csv"), dir.join("regions.csv")).unwrap();
    dir
}

fn engine() -> Box<dyn Engine> {
    Box::new(DuckDbEngine::new().unwrap())
}

fn python(dir: &Path) -> Option<PythonRunner> {
    Some(PythonRunner::new(vda_core::python::default_interpreter(), dir.join(".work")))
}

fn step(id: &str, inputs: &[&str], kind: StepKind) -> Step {
    Step { id: id.into(), inputs: inputs.iter().map(|s| s.to_string()).collect(), label: None, kind }
}

/// Revenue by region joined with each region's manager and target, then a
/// SQL step and a Python step that each read two inputs.
fn analysis() -> Analysis {
    Analysis {
        parameters: vec![],
        name: "Revenue against target".into(),
        sources: vec![Source::primary("sales.csv"), Source::new("regions", "regions.csv")],
        steps: vec![
            step(
                "by_region",
                &["source"],
                StepKind::Aggregate {
                    group_by: vec!["region".into()],
                    measures: vec![Measure { func: AggFunc::Sum, column: Some("revenue".into()), alias: "revenue".into() }],
                },
            ),
            step(
                "with_target",
                &["by_region", "regions"],
                StepKind::Join { how: JoinKind::Full, on: vec![JoinKey::equal("region", "region")] },
            ),
            step(
                "missed",
                &["with_target", "regions"],
                StepKind::Sql {
                    sql: "SELECT region, manager, revenue, target, (SELECT COUNT(*) FROM regions) AS regions \
                          FROM input WHERE coalesce(revenue, 0) < target ORDER BY region"
                        .into(),
                },
            ),
            step(
                "ratio",
                &["with_target", "regions"],
                StepKind::Python {
                    code: "result = df.assign(ratio=(df['revenue'] / df['target']).round(3), regions=len(dfs['regions']))"
                        .into(),
                    timeout_ms: 60_000,
                },
            ),
        ],
        cells: vec![],
    }
}

fn rows(s: &mut Session, id: &str) -> Vec<Vec<Value>> {
    s.table(id).unwrap().rows
}

#[test]
fn join_sql_and_python_read_several_inputs() {
    let dir = project("run");
    let mut s = Session::new(analysis(), &dir, engine(), python(&dir)).unwrap();
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:?}");
    assert_eq!(run.sources.len(), 2);

    // Full join: four regions with orders, plus Central with a target and no
    // orders; orders without a region keep their own row.
    let joined = s.table("with_target").unwrap();
    let names: Vec<&str> = joined.schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["region", "revenue", "manager", "target"]);
    let regions: Vec<String> = joined.rows.iter().map(|r| r[0].to_string()).collect();
    assert!(regions.contains(&"Central".to_string()), "{regions:?}");
    assert_eq!(joined.rows.len(), 6, "East, North, South, West, missing region, Central");

    let missed = rows(&mut s, "missed");
    assert!(missed.iter().any(|r| r[0] == Value::Text("Central".into())));
    assert!(missed.iter().all(|r| r[4] == Value::Int(5)), "SQL read `regions` by id");

    let ratio = s.table("ratio").unwrap();
    let col = ratio.schema.index_of("regions").unwrap();
    assert!(ratio.rows.iter().all(|r| r[col] == Value::Int(5)), "Python read dfs['regions']");

    // The SQL shown for the join step runs (its inputs have results).
    let sql = s.pipeline_sql("missed").unwrap();
    assert!(sql.contains("FULL OUTER JOIN") && sql.contains("\"regions\" AS (SELECT * FROM read_csv("), "{sql}");
}

#[test]
fn changing_the_second_source_marks_only_its_readers_outdated() {
    let dir = project("stale");
    let mut s = Session::new(analysis(), &dir, engine(), python(&dir)).unwrap();
    assert!(s.run_all().unwrap().succeeded());
    fs::write(dir.join("regions.csv"), "region,manager,target\nEast,Priya Raman,1\n").unwrap();
    assert_eq!(s.state("by_region").unwrap(), StepState::Fresh, "reads only sales.csv");
    for id in ["with_target", "missed", "ratio"] {
        assert_eq!(s.state(id).unwrap(), StepState::Stale, "{id}");
    }
    let run = s.run_all().unwrap();
    assert_eq!(run.step("by_region").unwrap().status, RunStatus::Succeeded { cached: true });
    assert_eq!(run.step("with_target").unwrap().status, RunStatus::Succeeded { cached: false });
}

#[test]
fn reopening_keeps_results_and_the_format_round_trips() {
    let dir = project("reopen");
    save_analysis(&analysis(), &dir).unwrap();
    let text = fs::read_to_string(dir.join("analysis.json")).unwrap();
    assert!(text.contains("\"format_version\": 2") && text.contains("\"inputs\""), "{text}");
    assert_eq!(load_analysis(&dir).unwrap(), analysis());

    let mut s = Session::open(&dir, engine(), python(&dir)).unwrap();
    assert!(s.run_all().unwrap().succeeded());
    drop(s);
    let s = Session::open(&dir, engine(), python(&dir)).unwrap();
    for id in ["by_region", "with_target", "missed", "ratio"] {
        assert_eq!(s.state(id).unwrap(), StepState::Fresh, "{id}");
        assert!(matches!(s.output(id), Some(Output::Table(_))));
    }
}

#[test]
fn a_failed_input_skips_steps_that_read_it() {
    let dir = project("blocked");
    let mut a = analysis();
    // The join's second input fails: so do the join and everything after it.
    a.steps.insert(0, step("bad_regions", &["regions"], StepKind::Sql { sql: "SELECT nope FROM input".into() }));
    a.steps.iter_mut().find(|s| s.id == "with_target").unwrap().inputs[1] = "bad_regions".into();
    let mut s = Session::new(a, &dir, engine(), python(&dir)).unwrap();
    let run = s.run_all().unwrap();
    assert!(matches!(run.step("bad_regions").unwrap().status, RunStatus::Failed { .. }));
    assert_eq!(run.step("by_region").unwrap().status, RunStatus::Succeeded { cached: false });
    match &run.step("with_target").unwrap().status {
        RunStatus::Skipped { reason } => assert!(reason.contains("bad_regions"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn format_1_projects_open_and_keep_their_saved_results() {
    let dir = project("v1");
    let v1 = r#"{
      "format_version": 1,
      "name": "Old project",
      "source": {"path": "sales.csv", "format": "csv"},
      "steps": [
        {"id": "by_region", "input": "source", "label": null,
         "op": {"type": "aggregate", "group_by": ["region"],
                "measures": [{"func": "COUNT", "column": null, "alias": "orders"}]}}
      ],
      "cells": [{"type": "step", "step": "by_region"}]
    }"#;
    fs::write(dir.join("analysis.json"), v1).unwrap();
    let a = load_analysis(&dir).unwrap();
    assert_eq!(a.sources, vec![Source::primary("sales.csv")]);
    assert_eq!(a.steps[0].inputs, vec![SOURCE_ID.to_string()]);

    // A single-input step's cache key is computed exactly as in format 1, so
    // results saved by earlier versions are still found.
    let sources = [(SOURCE_ID.to_string(), "abc".to_string())].into_iter().collect();
    let keys = plan(&a, &Fingerprints { sources: &sources, engine: "duckdb x", python: None }).unwrap().keys;
    let mut h = Sha256::new();
    for part in [
        "vda-key-v1",
        "duckdb x",
        "source:abc",
        &vda_core::persist::step_kind_to_json(&a.steps[0].kind).to_string_compact(),
    ] {
        h.update(part.as_bytes());
        h.update(b"\n");
    }
    assert_eq!(keys["by_region"], h.hex_digest());

    let mut s = Session::open(&dir, engine(), python(&dir)).unwrap();
    assert!(s.run_all().unwrap().succeeded());
}

#[test]
fn invalid_graphs_are_explained() {
    let mut a = analysis();
    a.steps[1].inputs = vec!["by_region".into()];
    assert!(validate(&a).unwrap_err().to_string().contains("reads 2 inputs, not 1"));

    let mut a = analysis();
    a.steps[1].inputs = vec!["by_region".into(), "by_region".into()];
    assert!(validate(&a).unwrap_err().to_string().contains("twice"));

    let mut a = analysis();
    a.sources[1].id = "by_region".into();
    assert!(validate(&a).unwrap_err().to_string().contains("duplicate"));

    let mut a = analysis();
    a.sources[1].id = "my regions".into();
    assert!(validate(&a).unwrap_err().to_string().contains("letters, digits"));

    // Two inputs can form a cycle through either one.
    let mut a = analysis();
    a.steps[0].inputs = vec!["missed".into()];
    assert!(validate(&a).unwrap_err().to_string().contains("cycle"));
}

#[test]
fn renaming_a_source_rewrites_inputs_and_code_and_reruns_what_it_changes() {
    use vda_core::rename::rename_source;
    let dir = project("rename");
    let mut s = Session::new(analysis(), &dir, engine(), python(&dir)).unwrap();
    assert!(s.run_all().unwrap().succeeded());
    let key = |s: &Session, id: &str| s.plan().unwrap().keys[id].clone();
    let before = ["by_region", "with_target", "missed", "ratio"].map(|id| key(&s, id));

    let mut a = s.analysis.clone();
    let renamed = rename_source(&mut a, "regions", "targets").unwrap();
    assert_eq!(renamed.code_changed, ["missed", "ratio"]);
    assert_eq!(
        a.step("missed").unwrap().kind,
        StepKind::Sql {
            sql: "SELECT region, manager, revenue, target, (SELECT COUNT(*) FROM targets) AS regions \
                  FROM input WHERE coalesce(revenue, 0) < target ORDER BY region"
                .into()
        },
        "the table reference is renamed; the column alias `regions` is not"
    );
    s.analysis = a;
    let after = ["by_region", "with_target", "missed", "ratio"].map(|id| key(&s, id));
    assert_eq!(before[0], after[0], "a step that does not read the source keeps its result");
    assert!(before[1..].iter().zip(&after[1..]).all(|(b, a)| b != a), "readers are rerun");
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:?}");

    let mut a = s.analysis.clone();
    assert!(rename_source(&mut a, "targets", "by_region").unwrap_err().to_string().contains("already used"));
    assert!(rename_source(&mut a, "targets", "two words").unwrap_err().to_string().contains("cannot be a source name"));
    assert!(rename_source(&mut a, "nope", "x").is_err());
}
