//! Custom SQL runs in a sandbox: one read-only query, and file access limited
//! to the project (plus an explicitly allowed source file and the spill
//! directory). The restriction cannot be lifted from inside a query.

use std::path::{Path, PathBuf};
use vda_core::duck::{DuckDbEngine, EngineOptions};
use vda_core::model::*;
use vda_core::session::{RunStatus, Session};

fn setup(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("vda-sandbox-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("data.csv"), "region,amount\nEast,10\nWest,4\n").unwrap();
    std::fs::write(project.join("lookup.csv"), "region,manager\nEast,Ana\nWest,Bo\n").unwrap();
    std::fs::write(root.join("private.csv"), "secret\nhunter2\n").unwrap();
    (root, project)
}

fn session(project: &Path, extra: Vec<PathBuf>, sql: &str) -> Session {
    let mut allowed = vec![project.to_path_buf()];
    allowed.extend(extra);
    let opts = EngineOptions { allowed_paths: Some(allowed), ..Default::default() };
    let analysis = Analysis {
        parameters: vec![],
        name: "sandbox".into(),
        sources: vec![Source::primary("data.csv")],
        steps: vec![Step { id: "q".into(), inputs: vec![SOURCE_ID.into()], label: None, kind: StepKind::Sql { sql: sql.into() } }],
        cells: vec![],
    };
    Session::new(analysis, project, Box::new(DuckDbEngine::with_options(opts).unwrap()), None).unwrap()
}

fn run(project: &Path, sql: &str) -> RunStatus {
    let mut s = session(project, vec![], sql);
    s.run_all().unwrap().step("q").unwrap().status.clone()
}

fn error(status: RunStatus) -> String {
    match status {
        RunStatus::Failed { error } => error,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn ordinary_queries_and_project_files_work() {
    let (_root, project) = setup("ok");
    assert_eq!(run(&project, "SELECT region, amount * 2 AS twice FROM input ORDER BY region"), RunStatus::Succeeded { cached: false });
    assert_eq!(run(&project, "WITH t AS (SELECT * FROM input) FROM t SELECT count(*) AS n"), RunStatus::Succeeded { cached: false });
    // Joining another file inside the project is allowed.
    let lookup = project.join("lookup.csv");
    let sql = format!("SELECT i.region, l.manager FROM input i JOIN read_csv('{}') l USING (region)", lookup.display());
    assert_eq!(run(&project, &sql), RunStatus::Succeeded { cached: false });
}

#[test]
fn files_outside_the_project_cannot_be_read() {
    let (root, project) = setup("read");
    for path in [root.join("private.csv"), project.join("../private.csv"), PathBuf::from("/etc/passwd")] {
        let e = error(run(&project, &format!("SELECT * FROM read_csv('{}')", path.display())));
        assert!(e.contains("Permission Error") || e.contains("disabled by configuration"), "{path:?}: {e}");
    }
    let e = error(run(&project, "SELECT * FROM glob('/etc/*')"));
    assert!(e.contains("Permission Error") || e.contains("disabled"), "{e}");
}

#[test]
fn statements_other_than_one_query_are_refused_before_running() {
    let (_root, project) = setup("stmts");
    let target = project.join("pwned.csv");
    for sql in [
        format!("COPY (SELECT 1) TO '{}'", target.display()),
        format!("SELECT 1) TO '{}' (FORMAT csv); COPY (SELECT 2", target.display()),
        "SET enable_external_access = true".to_string(),
        "SELECT 1; SET lock_configuration = false".to_string(),
        "SELECT * FROM input; SELECT 2".to_string(),
        format!("ATTACH '{}' AS other", project.join("x.db").display()),
        "INSTALL httpfs".to_string(),
        "PRAGMA version".to_string(),
    ] {
        let e = error(run(&project, &sql));
        assert!(e.contains("one query") || e.contains("single query") || e.contains("does not parse"), "{sql}: {e}");
    }
    assert!(!target.exists(), "nothing was written");
    let e = error(run(&project, "SELEC region FROM input"));
    assert!(e.contains("does not parse") && e.contains("SELEC"), "{e}");
}

#[test]
fn a_source_outside_the_project_can_be_allowed_explicitly() {
    let (root, project) = setup("external");
    let outside = root.join("shared");
    std::fs::create_dir_all(&outside).unwrap();
    let csv = outside.join("orders.csv");
    std::fs::write(&csv, "region,amount\nEast,1\n").unwrap();
    let mut s = session(&project, vec![csv.clone()], "SELECT * FROM input");
    s.analysis.sources[0].path = csv.to_string_lossy().into_owned();
    assert!(s.run_all().unwrap().succeeded());
    // Allowing one file does not open its folder.
    std::fs::write(outside.join("other.csv"), "x\n1\n").unwrap();
    let sql = format!("SELECT * FROM read_csv('{}')", outside.join("other.csv").display());
    let mut s2 = session(&project, vec![csv], &sql);
    let e = error(s2.run_all().unwrap().step("q").unwrap().status.clone());
    assert!(e.contains("Permission Error") || e.contains("disabled"), "{e}");
}
