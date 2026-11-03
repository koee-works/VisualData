//! ADR-0004 from the user's side: a project made by someone else does not
//! run its Python until the user approves the code shown to them; code the
//! user writes in the app runs straight away; custom SQL needs no approval
//! because it cannot reach files outside the project.

use serde_json::json;
use std::fs;
use std::path::PathBuf;
use vda_app::App;

fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue/sales.csv")
}

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vda-review-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    let _ = fs::remove_file(&p);
    p
}

/// An app as one person's desktop: its own approvals file.
fn person(approvals: &std::path::Path) -> App {
    App::default().with_approvals_file(approvals.to_path_buf())
}

const CODE: &str = "result = df.groupby('region', as_index=False)['revenue'].sum()";

fn python_op(code: &str) -> serde_json::Value {
    json!({"type": "python", "timeout_ms": 30000, "code": code})
}

fn state(app: &mut App, id: &str) -> String {
    app.view().unwrap().steps.iter().find(|s| s.id == id).unwrap().state.clone()
}

/// Author a project as "someone else" and return its folder.
fn received_project(name: &str) -> PathBuf {
    let dir = temp(name);
    let mut author = person(&temp(&format!("{name}-author.json")));
    author.create_project(&dir, &sample(), "Shared with me").unwrap();
    author.add_step("source", None, &python_op(CODE)).unwrap();
    author
        .add_step("python_1", None, &json!({"type": "sql", "sql": "SELECT * FROM input ORDER BY revenue DESC"}))
        .unwrap();
    let view = author.view().unwrap();
    assert!(view.review.is_empty(), "code the author wrote in the app is approved for the author");
    author.run(&mut |_| {}).unwrap();
    author.close_project();
    // The recipient gets the definition only, not the author's saved results.
    fs::remove_dir_all(dir.join("artifacts")).unwrap();
    dir
}

#[test]
fn received_python_waits_for_review_and_approval_is_remembered() {
    let dir = received_project("received");
    let approvals = temp("received-me.json");
    let mut me = person(&approvals);

    let view = me.open_project(&dir).unwrap();
    assert_eq!(view.review.len(), 1);
    assert_eq!(view.review[0].step_id, "python_1");
    assert_eq!(view.review[0].code, CODE, "the exact code is shown");
    assert_eq!(state(&mut me, "python_1"), "needs review");

    let summary = me.run(&mut |_| {}).unwrap();
    let status = |id: &str| summary.steps.iter().find(|s| s.step_id == id).unwrap().clone();
    assert_eq!(status("python_1").status, "needs review");
    assert_eq!(status("sql_1").status, "skipped");
    assert!(status("sql_1").detail.unwrap().contains("waiting for your review"));

    assert_eq!(state(&mut me, "sql_1"), "waiting on review");

    let view = me.approve(&["python_1".into()]).unwrap();
    assert_eq!(state(&mut me, "python_1"), "not run", "approval takes effect without a rerun");
    assert_eq!(state(&mut me, "sql_1"), "not run");
    assert!(view.review.is_empty());
    let summary = me.run(&mut |_| {}).unwrap();
    assert!(summary.succeeded, "{summary:?}");

    // A later session (a restart) remembers the approval.
    me.close_project();
    let mut later = person(&approvals);
    assert!(later.open_project(&dir).unwrap().review.is_empty());
    assert_eq!(state(&mut later, "python_1"), "fresh");
}

#[test]
fn changed_code_needs_review_again() {
    let dir = received_project("tampered");
    let approvals = temp("tampered-me.json");
    let mut me = person(&approvals);
    me.open_project(&dir).unwrap();
    me.approve(&["python_1".into()]).unwrap();
    me.close_project();

    // The project file is edited outside the app after approval.
    let path = dir.join("analysis.json");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.replace("as_index=False", "as_index=False, sort=True")).unwrap();

    let mut me = person(&approvals);
    let view = me.open_project(&dir).unwrap();
    assert_eq!(view.review.len(), 1, "edited code is new code");
    assert!(view.review[0].code.contains("sort=True"));
}

#[test]
fn editing_only_the_label_does_not_approve_code() {
    let dir = received_project("label");
    let mut me = person(&temp("label-me.json"));
    me.open_project(&dir).unwrap();
    me.update_step("python_1", Some("Revenue by region".into()), &python_op(CODE)).unwrap();
    assert_eq!(state(&mut me, "python_1"), "needs review");

    // Rewriting the code is writing it.
    me.update_step("python_1", None, &python_op("result = df")).unwrap();
    assert_eq!(state(&mut me, "python_1"), "not run");
}

#[test]
fn received_sql_runs_without_review_but_only_inside_the_project() {
    let dir = temp("sql-sandbox");
    let secret = temp("sql-secret.csv");
    fs::write(&secret, "token\nhunter2\n").unwrap();
    let mut author = person(&temp("sql-author.json"));
    author.create_project(&dir, &sample(), "SQL").unwrap();
    let sql = format!("SELECT * FROM read_csv('{}')", secret.display());
    author.add_step("source", None, &json!({"type": "sql", "sql": sql})).unwrap();
    author.close_project();

    let mut me = person(&temp("sql-me.json"));
    let view = me.open_project(&dir).unwrap();
    assert!(view.review.is_empty(), "SQL is sandboxed, not reviewed");
    let summary = me.run(&mut |_| {}).unwrap();
    let step = &summary.steps[0];
    assert_eq!(step.status, "failed");
    let detail = step.detail.clone().unwrap();
    assert!(!detail.contains("hunter2"));
    assert!(detail.to_lowercase().contains("permission") || detail.contains("not allowed"), "{detail}");
}
