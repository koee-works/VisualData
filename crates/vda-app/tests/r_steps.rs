//! R steps through the app's commands: written in the app they run at once;
//! arriving in a project from elsewhere they wait for review.

use serde_json::json;
use std::fs;
use std::path::PathBuf;
use vda_app::App;

fn samples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue")
}

fn fresh(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vda-rsteps-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    let _ = fs::remove_file(&p);
    p
}

fn r_available() -> bool {
    std::process::Command::new(vda_core::rlang::default_rscript()).args(["-e", "1"]).output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn an_r_step_written_in_the_app_runs_and_one_from_elsewhere_waits() {
    if !r_available() {
        return eprintln!("skipped: R is not available");
    }
    let dir = fresh("proj");
    let approvals = fresh("approvals.json");
    let mut app = App::default().with_approvals_file(approvals.clone()).with_settings_file(fresh("settings.json"));
    app.create_project(&dir, &samples().join("sales.csv"), "R").unwrap();
    app.set_parameters(&json!([{"name": "min_units", "value": {"int": "2"}}])).unwrap();
    let code = "big <- df[df$units >= params$min_units, ]\nresult <- aggregate(revenue ~ region, data = big, FUN = sum)\nprint(nrow(result))";
    let v = app.add_step(&["source".into()], Some("Revenue by region in R".into()), &json!({"type": "r", "code": code, "timeout_ms": 60000})).unwrap();
    assert_eq!(v.steps[0].mode, "r");
    assert!(v.review.is_empty(), "the author's own code needs no review");
    app.run(&mut |_| {}).unwrap();
    let v = app.view().unwrap();
    assert_eq!(v.steps[0].state, "fresh", "{:?}", v.steps[0].detail);
    let cols: Vec<String> = v.steps[0].columns.as_ref().unwrap().iter().map(|c| c.name.clone()).collect();
    assert_eq!(cols, ["region", "revenue"]);

    // A failing edit points at its line.
    app.update_step("r_1", None, &json!({"type": "r", "code": "x <- 1\nresult <- df[, \"nope\"]", "timeout_ms": 60000}), None).unwrap();
    app.run(&mut |_| {}).unwrap();
    let p = app.problems().unwrap();
    let failed = p.items.iter().find(|p| p.code == "failed").expect("a failure");
    assert_eq!((failed.field.as_deref(), failed.line), (Some("code"), Some(2)), "{failed:?}");
    assert!(failed.message.contains("R step: ") && failed.message.contains("undefined columns"), "{failed:?}");

    // Code changed outside the app (a colleague's edit) waits for review.
    let path = dir.join("analysis.json");
    let text = fs::read_to_string(&path).unwrap().replace("x <- 1", "x <- 2");
    fs::write(&path, text).unwrap();
    let mut other = App::default().with_approvals_file(approvals).with_settings_file(fresh("settings2.json"));
    let v = other.open_project(&dir).unwrap();
    assert_eq!(v.review.len(), 1);
    assert_eq!(v.review[0].language, "r");
    assert!(v.review[0].code.contains("x <- 2"));
}

#[test]
fn the_r_installation_is_described_and_can_be_chosen() {
    if !r_available() {
        return eprintln!("skipped: R is not available");
    }
    let settings = fresh("settings-r.json");
    let mut app = App::default().with_approvals_file(fresh("approvals-r.json")).with_settings_file(settings.clone());
    let v = app.r_environment().unwrap();
    if std::env::var_os("VDA_R").is_none() {
        assert_eq!(v.chosen_by, "default");
    }
    assert!(v.version.is_some() && v.problem.is_none(), "{v:?}");
    assert!(!v.libraries.is_empty());
    assert!(v.packages.iter().any(|p| p.name == "dplyr"));
    if std::env::var_os("VDA_R").is_some() {
        return;
    }
    let e = app.set_r(std::path::Path::new("/nowhere/Rscript")).unwrap_err().message;
    assert!(e.contains("cannot use /nowhere/Rscript"), "{e}");
    let rscript = vda_core::python::resolve(std::path::Path::new("Rscript"));
    let v = app.set_r(&rscript).unwrap();
    assert_eq!(v.chosen_by, "chosen");
    let saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(saved["r"], rscript.to_string_lossy().as_ref());
}

#[test]
fn a_console_beside_a_step_and_the_check_after_saving() {
    let dir = fresh("console");
    let mut app = App::default().with_approvals_file(fresh("approvals-c.json")).with_settings_file(fresh("settings-c.json"));
    app.create_project(&dir, &samples().join("sales.csv"), "Console").unwrap();
    let v = app.add_step(&["source".into()], None, &json!({"type": "python", "code": "result = df", "timeout_ms": 60000})).unwrap();
    let id = v.steps[0].id.clone();
    // Inputs are sources here, so the console starts without a run.
    let c = app.console_start(&id).unwrap();
    assert_eq!(c.language, "python");
    assert!(c.objects.iter().any(|o| o.name == "df"));
    let r = app.console_run("result = df[df['units'] > 2]\nresult.head(3)").unwrap();
    assert!(r.ok, "{r:?}");
    let t = r.table.unwrap();
    assert_eq!(t.rows.len(), 3);

    // The step saved with the code that made the console's result: they match.
    app.update_step(&id, None, &json!({"type": "python", "code": "result = df[df['units'] > 2]", "timeout_ms": 60000}), None).unwrap();
    app.run(&mut |_| {}).unwrap();
    assert_eq!(app.console_compare(&id).unwrap(), None);
    // The console changes `result` with a line the step does not have.
    app.console_run("result = result.head(1)").unwrap();
    let warning = app.console_compare(&id).unwrap().expect("a difference");
    assert!(warning.contains("only in the step"), "{warning}");

    app.console_stop();
    assert!(app.console_run("1").unwrap_err().message.contains("start the console first"));

    if r_available() {
        let v = app.add_step(&["source".into()], None, &json!({"type": "r", "code": "result <- df", "timeout_ms": 60000})).unwrap();
        let rid = v.steps.iter().find(|s| s.mode == "r").unwrap().id.clone();
        let c = app.console_start(&rid).unwrap();
        assert_eq!(c.language, "r");
        let r = app.console_run("plot(df$units, df$revenue)\nsummary(df$units)").unwrap();
        assert!(r.ok && r.images.len() == 1 && r.images[0].starts_with("data:image/png;base64,iVBOR"), "{r:?}");
        assert!(r.stdout.contains("Min."), "{}", r.stdout);
    }
}
