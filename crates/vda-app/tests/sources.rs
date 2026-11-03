//! Several sources and multi-input steps through the commands the UI calls.

use serde_json::json;
use std::fs;
use std::path::PathBuf;
use vda_app::App;

fn samples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue")
}

fn fresh(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vda-sources-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    let _ = fs::remove_file(&p);
    p
}

fn app(name: &str) -> App {
    App::default().with_approvals_file(fresh(&format!("{name}-approvals.json")))
}

fn ids(v: &[impl AsRef<str>]) -> Vec<String> {
    v.iter().map(|s| s.as_ref().to_string()).collect()
}

fn by_region() -> serde_json::Value {
    json!({"type": "aggregate", "group_by": ["region"], "measures": [
        {"func": "SUM", "column": "revenue", "alias": "revenue"}
    ]})
}

fn join(how: &str, left: &str, right: &str) -> serde_json::Value {
    json!({"type": "join", "how": how, "on": [{"left": left, "right": right}]})
}

#[test]
fn add_a_source_join_it_and_run() {
    let dir = fresh("join");
    let mut app = app("join");
    app.create_project(&dir, &samples().join("sales.csv"), "Targets").unwrap();
    let view = app.add_source(&samples().join("regions.csv")).unwrap();
    let ids_now: Vec<&str> = view.sources.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids_now, ["source", "regions"]);
    assert_eq!(view.sources[1].rows, 5);
    assert!(dir.join("regions.csv").exists(), "copied into the project");
    assert!(!view.sources[1].external);

    app.add_step(&ids(&["source"]), None, &by_region()).unwrap();
    let view = app.add_step(&ids(&["aggregate_1", "regions"]), Some("With targets".into()), &join("left", "region", "region")).unwrap();
    let j = view.steps.iter().find(|s| s.id == "join_1").unwrap();
    assert_eq!(j.inputs, ["aggregate_1", "regions"]);
    assert_eq!(j.mode, "visual");
    assert!(j.sql.is_none(), "a join's SQL appears once its inputs have run");
    assert_eq!(view.steps[0].dependents, ["join_1"]);

    // SQL reading two inputs by id.
    app.add_step(
        &ids(&["join_1", "regions"]),
        None,
        &json!({"type": "sql", "sql": "SELECT region, revenue, target, (SELECT COUNT(*) FROM regions) AS n FROM input ORDER BY region NULLS LAST"}),
    )
    .unwrap();

    let summary = app.run(&mut |_| {}).unwrap();
    assert!(summary.succeeded, "{summary:?}");
    let view = app.view().unwrap();
    let j = view.steps.iter().find(|s| s.id == "join_1").unwrap();
    assert!(j.sql.as_deref().unwrap().contains("LEFT JOIN"));
    let cols: Vec<&str> = j.columns.as_ref().unwrap().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(cols, ["region", "revenue", "manager", "target"]);
    let page = app.page("sql_1", 0, 10).unwrap();
    assert_eq!(page.total_rows, 5, "four regions and the orders without one");

    // The source's own pages and profile.
    assert_eq!(app.source_page("regions", 0, 10).unwrap().total_rows, 5);
    assert_eq!(app.source_profile("regions").unwrap().len(), 3);
}

#[test]
fn join_edits_are_checked_and_can_change_inputs() {
    let dir = fresh("edit");
    let mut app = app("edit");
    app.create_project(&dir, &samples().join("sales.csv"), "Edit").unwrap();
    app.add_source(&samples().join("regions.csv")).unwrap();
    app.add_step(&ids(&["source"]), None, &by_region()).unwrap();
    app.add_step(&ids(&["aggregate_1", "regions"]), None, &join("inner", "region", "region")).unwrap();
    app.run(&mut |_| {}).unwrap();

    // Both inputs' columns are known now, so a bad key is rejected on save.
    let err = app.update_step("join_1", None, &join("inner", "region", "target"), None).unwrap_err().message;
    assert!(err.contains("cannot join VARCHAR column `region` with BIGINT column `target`"), "{err}");

    // Join the source directly instead of the aggregate.
    let view = app.update_step("join_1", None, &join("inner", "region", "region"), Some(ids(&["source", "regions"]))).unwrap();
    let j = view.steps.iter().find(|s| s.id == "join_1").unwrap();
    assert_eq!(j.inputs, ["source", "regions"]);
    assert_eq!(j.state, "stale", "the earlier result is kept, marked outdated");

    let err = app.update_step("join_1", None, &join("inner", "region", "region"), Some(ids(&["source"]))).unwrap_err().message;
    assert!(err.contains("reads 2 inputs, not 1"), "{err}");
    let err = app.update_step("join_1", None, &by_region(), None).unwrap_err().message;
    assert!(err.contains("cannot change"), "{err}");
}

#[test]
fn sources_are_named_from_files_and_removed_only_when_unused() {
    let dir = fresh("names");
    let mut app = app("names");
    app.create_project(&dir, &samples().join("sales.csv"), "Names").unwrap();
    let odd = fresh("odd").with_extension("d");
    fs::create_dir_all(&odd).unwrap();
    let odd_file = odd.join("2026 Sales-Targets.csv");
    fs::copy(samples().join("regions.csv"), &odd_file).unwrap();
    let view = app.add_source(&odd_file).unwrap();
    assert_eq!(view.sources[1].id, "data_2026_sales_targets");

    // The same file name again gets a new id and a new file.
    let other = fresh("other").with_extension("d");
    fs::create_dir_all(&other).unwrap();
    fs::copy(samples().join("regions.csv"), other.join("2026 Sales-Targets.csv")).unwrap();
    let view = app.add_source(&other.join("2026 Sales-Targets.csv")).unwrap();
    assert_eq!(view.sources[2].id, "data_2026_sales_targets_2");
    assert_eq!(view.sources[2].path, "data_2026_sales_targets_2.csv");

    app.add_step(&ids(&["data_2026_sales_targets"]), None, &json!({"type": "sql", "sql": "SELECT * FROM input"})).unwrap();
    let err = app.remove_source("data_2026_sales_targets").unwrap_err().message;
    assert!(err.contains("sql_1"), "{err}");
    let view = app.remove_source("data_2026_sales_targets_2").unwrap();
    assert_eq!(view.sources.len(), 2);
    assert!(app.remove_source("nope").is_err());

    // Reopening finds every source.
    app.close_project();
    let view = app.open_project(&dir).unwrap();
    assert_eq!(view.sources.len(), 2);
}
