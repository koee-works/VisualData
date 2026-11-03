//! Undo and history, comments, and the Python environment through the commands.

use serde_json::json;
use std::fs;
use std::path::PathBuf;
use vda_app::App;

fn samples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue")
}

fn fresh(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vda-slice4-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    let _ = fs::remove_file(&p);
    p
}

fn app(name: &str) -> (App, PathBuf) {
    let dir = fresh(name);
    let mut app = App::default().with_approvals_file(fresh(&format!("{name}-approvals.json"))).with_settings_file(fresh(&format!("{name}-settings.json")));
    app.create_project(&dir, &samples().join("sales.csv"), "Slice 4").unwrap();
    (app, dir)
}

fn filter(units: i64) -> serde_json::Value {
    json!({"type": "filter", "predicates": [{"column": "units", "op": ">=", "value": {"int": units.to_string()}}]})
}

#[test]
fn undo_redo_and_history_restore_earlier_versions() {
    let (mut app, dir) = app("undo");
    let v = app.add_step(&["source".into()], Some("Big orders".into()), &filter(3)).unwrap();
    assert_eq!(v.undo.as_deref(), Some("Added Big orders"));
    assert_eq!(v.redo, None);
    app.update_step("filter_1", Some("Big orders".into()), &filter(5), None).unwrap();
    app.run(&mut |_| {}).unwrap();
    let rows_at_5 = app.view().unwrap().steps[0].rows;

    // Undo the edit: the filter is back at 3 and its result outdated.
    let v = app.undo().unwrap();
    assert_eq!(v.steps[0].op["predicates"][0]["value"], json!({"int": "3"}));
    assert_eq!(v.steps[0].state, "stale");
    assert_eq!(v.redo.as_deref(), Some("Edited Big orders"));
    // Redo: the saved result for 5 is current again without running.
    let v = app.redo().unwrap();
    assert_eq!(v.steps[0].state, "fresh");
    assert_eq!(v.steps[0].rows, rows_at_5);

    // A new change clears redo.
    app.undo().unwrap();
    let v = app.rename("Renamed").unwrap();
    assert_eq!(v.redo, None);
    assert_eq!(v.undo.as_deref(), Some("Renamed the analysis to \"Renamed\""));

    // History keeps every saved version, newest first, with the current one marked.
    let h = app.history(50).unwrap();
    let whats: Vec<&str> = h.iter().map(|v| v.what.as_str()).collect();
    assert_eq!(
        whats,
        ["Renamed the analysis to \"Renamed\"", "Undid: Edited Big orders", "Redid: Edited Big orders", "Undid: Edited Big orders", "Edited Big orders", "Added Big orders", "As first opened"]
    );
    assert!(h[0].current && h.iter().filter(|v| v.current).count() == 1);

    // History outlives the app: reopen and restore the version before the filter.
    drop(app);
    let mut app = App::default().with_approvals_file(fresh("undo-approvals.json")).with_settings_file(fresh("undo-settings.json"));
    app.open_project(&dir).unwrap();
    assert_eq!(app.view().unwrap().undo, None, "undo is for this session");
    let first = app.history(50).unwrap().last().unwrap().version;
    let v = app.restore_version(first).unwrap();
    assert!(v.steps.is_empty() && v.name == "Slice 4");
    let what = v.undo.as_deref().unwrap();
    assert!(what.starts_with(&format!("Restored version {first} (")) && what.contains("Deleted Big orders"), "{what}");
    // And restoring can be undone.
    let v = app.undo().unwrap();
    assert_eq!(v.name, "Renamed");
    assert!(app.restore_version(9999).unwrap_err().message.contains("no saved version 9999"));
    assert_eq!(app.redo().unwrap().name, "Slice 4");
    assert_eq!(app.undo().unwrap().name, "Renamed");
}

#[test]
fn undoing_back_to_received_code_asks_for_review_again() {
    let (mut app, dir) = app("review");
    // A project received from someone else: its Python code is not approved.
    let mut a: serde_json::Value = serde_json::from_str(&fs::read_to_string(dir.join("analysis.json")).unwrap()).unwrap();
    a["steps"] = json!([{"id": "py", "inputs": ["source"], "label": null, "op": {"type": "python", "code": "result = df", "timeout_ms": 30000}}]);
    fs::write(dir.join("analysis.json"), a.to_string()).unwrap();
    app.open_project(&dir).unwrap();
    assert_eq!(app.view().unwrap().review.len(), 1);
    // Writing the code yourself approves it; undo brings back the received code.
    app.update_step("py", None, &json!({"type": "python", "code": "result = df.head(5)", "timeout_ms": 30000}), None).unwrap();
    assert!(app.view().unwrap().review.is_empty());
    assert_eq!(app.undo().unwrap().review.len(), 1);
    assert!(app.redo().unwrap().review.is_empty());
}

#[test]
fn comments_are_kept_with_the_project_and_follow_renamed_sources() {
    let (mut app, dir) = app("comments");
    app.set_author("karl");
    app.add_step(&["source".into()], None, &filter(3)).unwrap();
    let list = app.add_comment("step:filter_1", "Why 3?", None).unwrap();
    let id = list[0].id.clone();
    assert_eq!((list[0].author.as_str(), list[0].target.as_str()), ("karl", "step:filter_1"));
    let list = app.add_comment("ignored", "Agreed, 5 is better", Some(id.clone())).unwrap();
    assert_eq!(list[1].target, "step:filter_1", "a reply is on its comment's target");
    app.add_comment("source:source", "Exported from the shop on Monday", None).unwrap();
    assert!(app.add_comment("step:nope", "x", None).unwrap_err().message.contains("no such step"));
    assert!(app.add_comment("analysis", "   ", None).unwrap_err().message.contains("write a comment"));

    // Resolving a comment resolves its replies.
    let list = app.resolve_comment(&id, true).unwrap();
    assert!(list.iter().filter(|c| c.target == "step:filter_1").all(|c| c.resolved));

    // Undo never removes comments; a deleted step's comments come back with it.
    app.remove_step("filter_1").unwrap();
    assert_eq!(app.comments().unwrap().len(), 3);
    app.undo().unwrap();
    assert!(app.comments().unwrap().iter().any(|c| c.target == "step:filter_1"));

    // A renamed source's comments follow it, and back again on undo.
    app.update_source("source", Some("orders".into()), None).unwrap();
    assert!(app.comments().unwrap().iter().any(|c| c.target == "source:orders"));
    app.undo().unwrap();
    assert!(app.comments().unwrap().iter().any(|c| c.target == "source:source"));

    // Only the author deletes a comment.
    app.set_author("sam");
    assert!(app.delete_comment(&id).unwrap_err().message.contains("only karl can delete"));
    app.set_author("karl");
    let list = app.delete_comment(&id).unwrap();
    assert_eq!(list.len(), 1, "the reply went with it");
    assert!(dir.join("comments.json").exists());
}

#[test]
fn the_python_environment_is_described_and_can_be_chosen() {
    if std::env::var_os("VDA_PYTHON").is_some() {
        eprintln!("skipped: VDA_PYTHON decides the interpreter");
        return;
    }
    let (mut app, _dir) = app("python");
    let env = app.python_environment().unwrap();
    assert!(env.problem.is_none(), "{:?}", env.problem);
    assert_eq!(env.chosen_by, "default");
    assert!(env.packages.iter().any(|p| p.name == "pandas" && p.required && p.version.is_some()));

    let err = app.set_python(std::path::Path::new("/no/such/python")).unwrap_err();
    assert!(err.message.starts_with("cannot use /no/such/python"), "{}", err.message);
    let exe = PathBuf::from(env.executable.unwrap());
    let env = app.set_python(&exe).unwrap();
    assert_eq!(env.chosen_by, "chosen");
    // Remembered for this user.
    let settings = std::env::temp_dir().join(format!("vda-slice4-python-settings.json-{}", std::process::id()));
    let mut again = App::default().with_settings_file(settings);
    assert_eq!(again.python_environment().unwrap().interpreter, exe.to_string_lossy());
}

#[test]
fn a_database_becomes_a_source_read_by_table_or_query() {
    let (mut app, dir) = app("database");
    let db = fresh("shop.db");
    // No telling extension: recognized by its contents.
    let c = duckdb::Connection::open(&db).unwrap();
    c.execute_batch("CREATE TABLE targets AS SELECT * FROM (VALUES ('East', 2400), ('West', 2000)) v(region, target); CREATE TABLE notes (t VARCHAR);").unwrap();
    drop(c);
    let v = app.add_source(&db).unwrap();
    let src = v.sources.iter().find(|s| s.format == "duckdb").expect("added as a DuckDB source");
    assert_eq!((src.sheet.as_deref(), src.rows), (Some("notes"), 0), "the first table by name");
    let id = src.id.clone();
    assert_eq!(app.source_sheets(&id).unwrap(), ["notes", "targets"]);

    let v = app.set_source_read(&id, Some("targets".into()), None).unwrap();
    assert_eq!(v.sources.iter().find(|s| s.id == id).unwrap().rows, 2);
    let err = app.set_source_read(&id, None, Some("SELECT * FROM missing".into())).unwrap_err();
    assert!(err.message.contains("does not run"), "{}", err.message);
    let v = app.set_source_read(&id, None, Some("SELECT region FROM targets WHERE target > 2100".into())).unwrap();
    let src = v.sources.iter().find(|s| s.id == id).unwrap();
    assert_eq!((src.rows, src.query.is_some()), (1, true));
    assert!(v.undo.as_deref().is_some());
    // Saved with the project: the table and the query.
    let saved = fs::read_to_string(dir.join("analysis.json")).unwrap();
    assert!(saved.contains("\"query\"") && saved.contains("\"table\""), "{saved}");
}

fn vda_run(project: &std::path::Path, approvals: &std::path::Path, args: &[&str]) -> (i32, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_vda-run"))
        .arg(project)
        .args(args)
        .env("VDA_APPROVALS", approvals)
        .output()
        .unwrap();
    (out.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn vda_run_refreshes_a_project_on_a_schedule() {
    let (mut app, dir) = app("refresh");
    let approvals = fresh("refresh-approvals.json");
    app.add_step(&["source".into()], Some("Big orders".into()), &filter(3)).unwrap();
    app.add_step(&["filter_1".into()], None, &json!({"type": "aggregate", "group_by": ["region"], "measures": [{"func": "SUM", "column": "revenue", "alias": "revenue"}]})).unwrap();
    drop(app);
    let out = dir.join("out.csv");
    let export = format!("aggregate_1={}", out.display());

    let (code, text) = vda_run(&dir, &approvals, &["--export", &export]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("all results current (2 ran, 0 reused"), "{text}");
    assert!(fs::read_to_string(&out).unwrap().starts_with("region,revenue"));
    // Nothing changed: everything is reused.
    let (code, text) = vda_run(&dir, &approvals, &["--export", &export]);
    assert_eq!(code, 0);
    assert!(text.contains("(0 ran, 2 reused"), "{text}");
    // A new source file: what reads it runs again.
    let mut csv = fs::read_to_string(dir.join("sales.csv")).unwrap();
    // In the file's own line endings (the sample uses CRLF).
    let eol = if csv.contains("\r\n") { "\r\n" } else { "\n" };
    if !csv.ends_with('\n') {
        csv.push_str(eol);
    }
    csv.push_str(&format!("999,2026-11-01,East,Basic,9,12.0,108.0{eol}"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(dir.join("sales.csv"), csv).unwrap();
    let (code, text) = vda_run(&dir, &approvals, &["--quiet"]);
    assert_eq!((code, text.as_str()), (0, ""), "quiet prints nothing on success");

    // The app's run log shows the scheduled runs.
    let mut app = App::default().with_approvals_file(approvals.clone()).with_settings_file(fresh("refresh-settings.json"));
    app.open_project(&dir).unwrap();
    let log = app.run_log(10).unwrap();
    assert_eq!(log.iter().filter(|r| r.trigger.as_deref() == Some("scheduled")).count(), 3);
    assert!(app.view().unwrap().steps.iter().all(|s| s.state == "fresh"));

    // Code nobody approved waits for review: exit 3, nothing exported.
    let mut a: serde_json::Value = serde_json::from_str(&fs::read_to_string(dir.join("analysis.json")).unwrap()).unwrap();
    a["steps"].as_array_mut().unwrap().push(json!({"id": "py", "inputs": ["source"], "label": null, "op": {"type": "python", "code": "result = df", "timeout_ms": 30000}}));
    fs::write(dir.join("analysis.json"), a.to_string()).unwrap();
    fs::remove_file(&out).unwrap();
    let (code, text) = vda_run(&dir, &approvals, &["--export", &export]);
    assert_eq!(code, 3, "{text}");
    assert!(text.contains("Python code in py waits for your review"), "{text}");
    assert!(!out.exists(), "nothing exported");

    // A failing step: exit 1.
    a["steps"].as_array_mut().unwrap().pop();
    a["steps"].as_array_mut().unwrap().push(json!({"id": "bad", "inputs": ["source"], "label": null, "op": {"type": "sql", "sql": "SELECT nope FROM input"}}));
    fs::write(dir.join("analysis.json"), a.to_string()).unwrap();
    let (code, text) = vda_run(&dir, &approvals, &[]);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("step `bad` failed") && text.contains("nothing was exported"), "{text}");

    // One refresh at a time.
    fs::write(dir.join("runs").join("refresh.lock"), "1").unwrap();
    let (code, text) = vda_run(&dir, &approvals, &[]);
    assert_eq!(code, 2);
    assert!(text.contains("another refresh of this project is running"), "{text}");
    fs::remove_file(dir.join("runs").join("refresh.lock")).unwrap();
    let (code, text) = vda_run(&dir, &approvals, &["--export", "nope=x.csv"]);
    assert_eq!(code, 2);
    assert!(text.contains("no such step or source"), "{text}");
    let (code, _) = vda_run(&dir, &approvals, &["--bogus"]);
    assert_eq!(code, 2);
}

#[test]
fn schedules_are_written_for_the_computers_scheduler() {
    let (mut app, _dir) = app("schedule");
    let s = app.schedule("weekdays", 7, 30, &[("source".into(), PathBuf::from("/tmp/out file.csv"))]).unwrap();
    assert!(s.command.contains("vda-run") && s.command.contains("--export"), "{}", s.command);
    if cfg!(target_os = "linux") {
        assert_eq!(s.scheduler, "cron");
        assert!(s.entry.starts_with("30 7 * * 1-5 "), "{}", s.entry);
        assert!(s.entry.contains("'source=/tmp/out file.csv'"), "quoted: {}", s.entry);
        assert!(s.entry.contains("scheduled.log"));
    }
    assert!(app.schedule("hourly", 0, 5, &[]).unwrap().entry.starts_with("5 * * * *") || !cfg!(target_os = "linux"));
    assert!(app.schedule("monthly", 0, 0, &[]).unwrap_err().message.contains("hourly, daily, weekdays or weekly"));
    assert!(app.schedule("daily", 24, 0, &[]).is_err());
}
