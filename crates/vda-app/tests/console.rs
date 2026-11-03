//! The console's commands: problems, dismissing, checking drafts, run log.

use serde_json::json;
use std::fs;
use std::path::PathBuf;
use vda_app::App;

fn samples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue")
}

fn fresh(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vda-console-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    let _ = fs::remove_file(&p);
    p
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn problems_dismissals_drafts_and_the_run_log() {
    let dir = fresh("all");
    let mut app = App::default().with_approvals_file(fresh("all-approvals.json"));
    app.create_project(&dir, &samples().join("sales.csv"), "Console").unwrap();
    app.add_step(&ids(&["source"]), None, &json!({"type": "python", "timeout_ms": 30000,
        "code": "print('rows in:', len(df))\nfor _, r in df.iterrows():\n    pass\nresult = df"})).unwrap();
    app.add_step(&ids(&["python_1"]), None, &json!({"type": "sql", "sql": "SELECT * FROM input JOIN nowhere USING (x)"})).unwrap();

    let p = app.problems().unwrap();
    let codes: Vec<(&str, &str)> = p.items.iter().map(|d| (d.severity.as_str(), d.code.as_str())).collect();
    assert_eq!(codes[0], ("error", "sql-table"), "{codes:?}");
    assert!(codes.contains(&("warning", "row-loop")), "{codes:?}");
    assert!(!p.items[0].dismissable);

    // Dismissing a warning hides it, for everyone who opens the project.
    let p = app.dismiss(Some("python_1".into()), "row-loop", true).unwrap();
    assert_eq!(p.dismissed, 1);
    assert!(p.items.iter().all(|d| d.code != "row-loop"));
    app.close_project();
    app.open_project(&dir).unwrap();
    assert_eq!(app.problems().unwrap().dismissed, 1);
    assert_eq!(app.restore_dismissed().unwrap().dismissed, 0);

    // A draft is checked as typed, before it is saved or even added.
    let d = app.check_draft(None, &ids(&["source"]), &json!({"type": "derive", "columns": [{"name": "t", "formula": "revenue +"}]})).unwrap();
    assert!(d[0].message.contains("ends too early"), "{d:?}");
    assert_eq!(d[0].field.as_deref(), Some("columns.0.formula"));
    let ok = app.check_draft(None, &ids(&["source"]), &json!({"type": "derive", "columns": [{"name": "t", "formula": "revenue * 2"}]})).unwrap();
    assert!(ok.is_empty(), "{ok:?}");

    // The run log keeps what Python printed.
    let mut live = Vec::new();
    app.run(&mut |e| live.push(e)).unwrap();
    assert!(live[0].output.as_deref().unwrap().contains("rows in: 60"));
    let log = app.run_log(10).unwrap();
    assert_eq!(log.len(), 1);
    assert!(!log[0].succeeded);
    assert_eq!(log[0].steps[0].output.as_deref(), Some("rows in: 60\n"));
    assert_eq!(log[0].steps[1].status, "failed");
}
