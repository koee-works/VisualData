//! Disk-backed results: sessions hold handles, previews are paged, the source
//! is converted once, charts over big results are labelled partial, and the
//! engine works under a memory limit by spilling to disk.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use vda_core::duck::{DuckDbEngine, EngineOptions};
use vda_core::model::*;
use vda_core::python::{default_interpreter, PythonRunner};
use vda_core::session::{Output, RunStatus, Session};
use vda_core::table::{DataType, Value};

/// `rows` orders across 1,000 customers and 7 regions, deterministic.
fn orders_csv(rows: usize) -> String {
    let mut s = String::from("order_id,customer,region,amount\n");
    for i in 0..rows {
        let amount = if i % 101 == 0 { String::new() } else { format!("{}.{:02}", (i * 37) % 500, i % 100) };
        writeln!(s, "{i},c{},r{},{amount}", (i * 7919) % 1000, i % 7).unwrap();
    }
    s
}

fn project(name: &str, rows: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-large-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("orders.csv"), orders_csv(rows)).unwrap();
    dir
}

fn analysis(steps: Vec<Step>) -> Analysis {
    Analysis { name: "orders".into(), sources: vec![Source::primary("orders.csv")], steps, cells: vec![] }
}

fn step(id: &str, input: &str, kind: StepKind) -> Step {
    Step { id: id.into(), inputs: vec![input.into()], label: None, kind }
}

fn by_customer() -> Step {
    step(
        "by_customer",
        SOURCE_ID,
        StepKind::Aggregate {
            group_by: vec!["customer".into()],
            measures: vec![
                Measure { func: AggFunc::Count, column: None, alias: "orders".into() },
                Measure { func: AggFunc::Sum, column: Some("amount".into()), alias: "total".into() },
            ],
        },
    )
}

fn session(dir: &Path, steps: Vec<Step>, opts: EngineOptions) -> Session {
    let python = Some(PythonRunner::new(default_interpreter(), dir.join(".work")));
    Session::new(analysis(steps), dir, Box::new(DuckDbEngine::with_options(opts).unwrap()), python).unwrap()
}

#[test]
fn previews_are_paged_from_disk() {
    let dir = project("paging", 20_000);
    let mut s = session(&dir, vec![step("all", SOURCE_ID, StepKind::Filter { predicates: vec![] })], EngineOptions::default());
    assert!(s.run_all().unwrap().succeeded());
    let ds = s.output("all").unwrap().as_dataset().unwrap().clone();
    assert_eq!(ds.rows, 20_000);
    assert!(ds.path.exists() && ds.bytes > 0);

    // Pages are contiguous, in order, and together equal the whole result.
    let mut paged = Vec::new();
    let mut offset = 0;
    while offset < ds.rows {
        let page = s.preview("all", offset, 7_000).unwrap();
        assert!(page.rows.len() <= 7_000);
        paged.extend(page.rows);
        offset += 7_000;
    }
    assert_eq!(paged.len(), 20_000);
    assert_eq!(paged[12_345][0], Value::Int(12_345));
    assert_eq!(paged, s.table("all").unwrap().rows);
    assert!(s.preview("all", 20_000, 10).unwrap().rows.is_empty());
}

#[test]
fn source_is_converted_once() {
    let dir = project("source-once", 5_000);
    let mut s = session(&dir, vec![by_customer()], EngineOptions::default());
    s.run_all().unwrap();
    let source = s.source_dataset(SOURCE_ID).unwrap();
    let converted_at = std::fs::metadata(&source.path).unwrap().modified().unwrap();
    vda_core::persist::save_analysis(&s.analysis, &dir).unwrap();

    // Editing a step, rerunning, and reopening all reuse the converted source.
    if let StepKind::Aggregate { group_by, .. } = &mut s.analysis.step_mut("by_customer").unwrap().kind {
        group_by[0] = "region".into();
    }
    assert!(s.run_all().unwrap().succeeded());
    let mut reopened = Session::open(&dir, Box::new(DuckDbEngine::new().unwrap()), None).unwrap();
    assert_eq!(reopened.source_dataset(SOURCE_ID).unwrap(), source);
    assert_eq!(std::fs::metadata(&source.path).unwrap().modified().unwrap(), converted_at);

    // Profile is computed in SQL over the file.
    let profile = reopened.source_profile(SOURCE_ID).unwrap();
    let amount = profile.iter().find(|p| p.name == "amount").unwrap();
    assert_eq!(amount.dtype, DataType::Float);
    assert_eq!(amount.nulls, 5_000usize.div_ceil(101));
    assert_eq!(profile.iter().find(|p| p.name == "customer").unwrap().distinct, 1_000);
}

#[test]
fn chart_over_a_large_result_is_labelled_partial() {
    let dir = project("chart", 12_000);
    let chart = step(
        "scatter",
        SOURCE_ID,
        StepKind::Chart { mark: Mark::Point, x: "order_id".into(), y: "amount".into(), title: Some("Amounts".into()) },
    );
    let mut s = session(&dir, vec![chart], EngineOptions::default());
    assert!(s.run_all().unwrap().succeeded());
    let Some(Output::Chart(spec)) = s.output("scatter") else { panic!("no chart") };
    assert_eq!(spec.get("title").unwrap().as_str(), Some("Amounts (first 5000 of 12000 rows)"));
    assert_eq!(spec.get("data").unwrap().get("values").unwrap().as_arr().unwrap().len(), 5_000);
}

#[test]
fn works_under_a_small_memory_limit() {
    // 400k rows grouped into 400k distinct keys with a 64 MB cap: DuckDB has
    // to spill to the temp directory instead of failing.
    let dir = project("spill", 400_000);
    let temp = dir.join("spill");
    let opts = EngineOptions { memory_limit_mb: Some(64), temp_dir: Some(temp.clone()), allowed_paths: None };
    let per_order = step(
        "per_order",
        SOURCE_ID,
        StepKind::Aggregate {
            group_by: vec!["order_id".into(), "customer".into()],
            measures: vec![Measure { func: AggFunc::Max, column: Some("amount".into()), alias: "amount".into() }],
        },
    );
    let mut s = session(&dir, vec![per_order, by_customer()], opts);
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:#?}");
    assert_eq!(run.step("per_order").unwrap().rows, Some(400_000));
    assert_eq!(run.step("by_customer").unwrap().rows, Some(1_000));
    // Sums over integers stay integers across the Parquet boundary.
    let t = s.preview("by_customer", 0, 1).unwrap();
    assert_eq!(t.schema.field("orders").unwrap().dtype, DataType::Int);
}

#[test]
fn python_reads_the_upstream_file_and_types_survive() {
    let dir = project("python", 30_000);
    let py = step(
        "top",
        "by_customer",
        StepKind::Python {
            code: "result = df.sort_values('total', ascending=False).head(5).reset_index(drop=True)".into(),
            timeout_ms: 60_000,
        },
    );
    let mut s = session(&dir, vec![by_customer(), py], EngineOptions::default());
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:#?}");
    let top = s.table("top").unwrap();
    assert_eq!(top.rows.len(), 5);
    assert_eq!(top.schema.field("orders").unwrap().dtype, DataType::Int);
    // A rerun reuses everything from disk.
    let again = s.run_all().unwrap();
    assert!(again.steps.iter().all(|r| r.status == RunStatus::Succeeded { cached: true }));
}

#[test]
fn reopening_does_not_rehash_an_unchanged_source() {
    let dir = project("fingerprint", 2_000);
    let s = session(&dir, vec![by_customer()], EngineOptions::default());
    let real = s.source_hash(SOURCE_ID).unwrap();
    let saved = dir.join("artifacts").join("source-fingerprint.json");
    assert!(saved.exists());

    // Plant a marker hash: a new session that returns it did not reread the file.
    let text = std::fs::read_to_string(&saved).unwrap().replace(&real, "marker");
    std::fs::write(&saved, text).unwrap();
    let reopened = session(&dir, vec![by_customer()], EngineOptions::default());
    assert_eq!(reopened.source_hash(SOURCE_ID).unwrap(), "marker");

    // Any change to size or modification time makes it hash again.
    let mut csv = std::fs::read_to_string(dir.join("orders.csv")).unwrap();
    csv.push_str("99999,c1,r1,1.00\n");
    std::fs::write(dir.join("orders.csv"), csv).unwrap();
    let after_edit = session(&dir, vec![by_customer()], EngineOptions::default());
    let new_hash = after_edit.source_hash(SOURCE_ID).unwrap();
    assert!(new_hash != "marker" && new_hash != real);
}
