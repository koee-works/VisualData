//! Drives the command layer the way the UI does: create a project from a CSV,
//! add visual, SQL, Python and chart steps, run with progress, page results,
//! edit and see staleness, reopen.

use serde_json::json;
use std::path::PathBuf;
use vda_app::{App, Cell};

fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue/sales.csv")
}

/// Approvals go to a temporary file, never the user's real one.
fn test_app() -> App {
    App::default().with_approvals_file(fresh_dir("approvals").with_extension("json"))
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn filter_op(min: i64) -> serde_json::Value {
    json!({"type": "filter", "predicates": [
        {"column": "region", "op": "IS NOT NULL", "value": null},
        {"column": "revenue", "op": ">=", "value": {"int": min.to_string()}}
    ]})
}

fn aggregate_op() -> serde_json::Value {
    json!({"type": "aggregate", "group_by": ["region"], "measures": [
        {"func": "COUNT", "column": null, "alias": "orders"},
        {"func": "SUM", "column": "revenue", "alias": "revenue"}
    ]})
}

#[test]
fn mixed_workflow_through_commands() {
    let dir = fresh_dir("workflow");
    let mut app = test_app();
    let view = app.create_project(&dir, &sample(), "Regional revenue").unwrap();
    assert_eq!(view.sources[0].rows, 60);
    assert!(dir.join("sales.csv").exists(), "the CSV is copied into the project");

    let profile = app.source_profile("source").unwrap();
    let region = profile.iter().find(|p| p.name == "region").unwrap();
    assert_eq!((region.dtype.as_str(), region.missing), ("VARCHAR", 2));

    app.add_step(&["source".to_string()], Some("Valid orders".into()), &filter_op(20)).unwrap();
    app.add_step(&["filter_1".to_string()], None, &aggregate_op()).unwrap();
    app.add_step(&["aggregate_1".to_string()],
        None,
        &json!({"type": "python", "timeout_ms": 30000,
                "code": "result = df.assign(share=df['revenue'] / df['revenue'].sum())"}),
    )
    .unwrap();
    app.add_step(&["python_1".to_string()], None, &json!({"type": "chart", "mark": "bar", "x": "region", "y": "share", "title": null}))
        .unwrap();
    let view = app
        .add_step(&["filter_1".to_string()], None, &json!({"type": "sql", "sql": "SELECT region, max(revenue) AS biggest FROM input GROUP BY region ORDER BY biggest DESC"}))
        .unwrap();
    let ids: Vec<&str> = view.steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["filter_1", "aggregate_1", "python_1", "chart_1", "sql_1"]);
    assert!(view.steps.iter().all(|s| s.state == "not run"));
    assert_eq!(view.steps[0].dependents, ["aggregate_1", "sql_1"]);
    assert_eq!(view.cells.as_array().unwrap().len(), 5, "each step gets a notebook cell");
    assert!(view.steps[1].sql.as_deref().unwrap().contains("GROUP BY \"region\""));

    // Run, with one progress event per step in execution order.
    let mut events = Vec::new();
    let summary = app.run(&mut |e| events.push(e)).unwrap();
    assert!(summary.succeeded, "{summary:#?}");
    assert_eq!(events.len(), 5);
    assert!(dir.join("runs").read_dir().unwrap().count() == 1, "run record saved");

    let view = app.view().unwrap();
    assert!(view.steps.iter().all(|s| s.state == "fresh"));
    assert_eq!(view.steps[0].rows, Some(53));

    // Pages carry typed cells; integers stay exact strings.
    let page = app.page("aggregate_1", 0, 10).unwrap();
    assert_eq!(page.total_rows, 4);
    assert_eq!(page.columns[1].name, "orders");
    assert_eq!(page.rows[0][0], Cell::Text("East".into()));
    assert_eq!(page.rows[0][1], Cell::Int("19".into()));
    let src = app.source_page("source", 0, 3).unwrap();
    assert_eq!((src.rows.len(), src.total_rows), (3, 60));
    assert!(matches!(src.rows[0][1], Cell::Date(ref d) if d == "2026-07-03"));
    assert!(app.page("filter_1", 0, 1_000_000).unwrap().rows.len() <= vda_app::MAX_PAGE_ROWS as usize);

    let spec = app.chart("chart_1").unwrap();
    assert_eq!(spec["mark"]["type"], "bar");

    // Editing the filter makes everything downstream stale, and pages say so.
    app.update_step("filter_1", Some("Valid orders".into()), &filter_op(100), None).unwrap();
    let view = app.view().unwrap();
    assert!(view.steps.iter().all(|s| s.state == "stale"));
    assert!(app.page("aggregate_1", 0, 10).unwrap().stale);

    // Reopen from disk: edits were saved as they happened.
    app.close_project();
    let view = app.open_project(&dir).unwrap();
    assert_eq!(view.steps.len(), 5);
    assert_eq!(view.steps[0].op["predicates"][1]["value"]["int"], "100");
}

#[test]
fn invalid_edits_are_rejected_with_a_reason() {
    let dir = fresh_dir("invalid");
    let mut app = test_app();
    app.create_project(&dir, &sample(), "x").unwrap();

    let bad_column = json!({"type": "filter", "predicates": [{"column": "regoin", "op": "=", "value": "East"}]});
    let err = app.add_step(&["source".to_string()], None, &bad_column).unwrap_err().message;
    assert!(err.contains("regoin") && err.contains("available"), "{err}");

    let err = app.add_step(&["nope".to_string()], None, &aggregate_op()).unwrap_err().message;
    assert!(err.contains("unknown step"), "{err}");

    app.add_step(&["source".to_string()], None, &filter_op(20)).unwrap();
    app.add_step(&["filter_1".to_string()], None, &aggregate_op()).unwrap();
    let err = app.remove_step("filter_1").unwrap_err().message;
    assert!(err.contains("aggregate_1"), "{err}");

    let err = app.update_step("filter_1", None, &json!({"type": "sql", "sql": "SELECT 1"}), None).unwrap_err().message;
    assert!(err.contains("cannot change"), "{err}");

    let err = app.add_step(&["source".to_string()], None, &json!({"type": "frobnicate"})).unwrap_err().message;
    assert!(err.contains("unknown step type"), "{err}");

    // Nothing invalid was saved.
    let view = app.view().unwrap();
    assert_eq!(view.steps.len(), 2);
    app.remove_step("aggregate_1").unwrap();
    assert_eq!(app.view().unwrap().cells.as_array().unwrap().len(), 1);
}

#[test]
fn notebook_cells_can_be_edited() {
    let dir = fresh_dir("cells");
    let mut app = test_app();
    app.create_project(&dir, &sample(), "x").unwrap();
    app.add_step(&["source".to_string()], None, &filter_op(20)).unwrap();
    let view = app
        .set_cells(&json!([
            {"type": "markdown", "text": "## Question\nWhich regions matter?"},
            {"type": "step", "step": "filter_1"}
        ]))
        .unwrap();
    assert_eq!(view.cells[0]["text"], "## Question\nWhich regions matter?");
    let err = app.set_cells(&json!([{"type": "step", "step": "missing"}])).unwrap_err().message;
    assert!(err.contains("unknown step"), "{err}");
}

#[test]
fn creating_over_an_existing_project_is_refused() {
    let dir = fresh_dir("exists");
    let mut app = test_app();
    app.create_project(&dir, &sample(), "x").unwrap();
    assert!(app.create_project(&dir, &sample(), "y").unwrap_err().message.contains("already contains"));
}
