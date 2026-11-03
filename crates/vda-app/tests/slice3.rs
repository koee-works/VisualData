//! Parameters, checks, column details and exports through the commands.

use serde_json::json;
use std::fs;
use std::path::PathBuf;
use vda_app::App;

fn samples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue")
}

fn fresh(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vda-slice3-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    let _ = fs::remove_file(&p);
    p
}

#[test]
fn parameters_checks_details_and_exports() {
    let dir = fresh("all");
    let mut app = App::default().with_approvals_file(fresh("approvals.json"));
    app.create_project(&dir, &samples().join("sales.csv"), "Slice 3").unwrap();
    let v = app.set_parameters(&json!([{"name": "min_units", "value": {"int": "3"}}])).unwrap();
    assert_eq!(v.parameters, json!([{"name": "min_units", "value": {"int": "3"}}]));
    let bad = app.set_parameters(&json!([{"name": "2x", "value": true}])).unwrap_err();
    assert!(bad.message.contains("starting with a letter"), "{}", bad.message);

    app.add_step(&["source".into()], None, &json!({"type": "filter", "predicates": [
        {"column": "units", "op": ">=", "value": null, "param": "min_units"}]})).unwrap();
    app.add_step(&["filter_1".into()], None, &json!({"type": "check", "stop": false, "rules": [
        {"rule": "expect", "formula": "units >= @min_units"},
        {"rule": "rows", "min": 100, "max": null}]})).unwrap();
    let summary = app.run(&mut |_| {}).unwrap();
    assert!(summary.succeeded);
    let report = app.check_report("check_1").unwrap().unwrap();
    assert_eq!(report["results"][0]["passed"], json!(true));
    assert_eq!(report["results"][1]["passed"], json!(false));
    let problems = app.problems().unwrap();
    assert!(problems.items.iter().any(|p| p.code == "check-failed" && p.field.as_deref() == Some("rules.1")));

    // Renaming follows into the filter and the rule.
    let (v, python) = app.rename_parameter("min_units", "least").unwrap();
    assert!(python.is_empty());
    let check = v.steps.iter().find(|s| s.id == "check_1").unwrap();
    assert_eq!(check.op["rules"][0]["formula"], json!("units >= @least"));
    assert_eq!(v.steps[0].op["predicates"][0]["param"], json!("least"));
    assert_eq!(v.steps[0].state, "fresh", "the same value: still current");

    let details = app.column_details("filter_1").unwrap();
    let units = details.as_array().unwrap().iter().find(|c| c["name"] == "units").unwrap();
    assert!(units["mean"].as_f64().unwrap() >= 3.0);
    assert_eq!(units["histogram"].as_array().unwrap().len(), 10);

    let nb = dir.join("analysis.ipynb");
    app.export_notebook(&nb).unwrap();
    let text = fs::read_to_string(&nb).unwrap();
    assert!(text.contains("\"nbformat\": 4"));
    assert!(app.save_report(&dir.join("report.txt"), "<html></html>").unwrap_err().message.contains(".html"));
    app.save_report(&dir.join("report.html"), "<html>ok</html>").unwrap();
    assert_eq!(fs::read_to_string(dir.join("report.html")).unwrap(), "<html>ok</html>");
}
