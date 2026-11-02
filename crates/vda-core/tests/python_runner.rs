use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vda_core::csv::read_table;
use vda_core::python::PythonRunner;
use vda_core::table::{DataType, Value};
use vda_core::Error;

fn runner(name: &str) -> PythonRunner {
    let dir = std::env::temp_dir().join(format!("vda-py-tests-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    PythonRunner::new("python3", dir)
}

fn input() -> vda_core::table::Table {
    read_table("region,amount\nEast,10\nWest,4\nEast,6\n").unwrap()
}

#[test]
fn runs_code_and_returns_table() {
    let code = "result = df.assign(share=df['amount'] / df['amount'].sum())";
    let out = runner("ok").run(code, &input(), Duration::from_secs(30), &AtomicBool::new(false)).unwrap();
    assert_eq!(out.schema.field("share").unwrap().dtype, DataType::Float);
    assert_eq!(out.rows[0][2], Value::Float(0.5));
}

#[test]
fn reports_contract_violation() {
    let err = runner("contract").run("x = 1", &input(), Duration::from_secs(30), &AtomicBool::new(false)).unwrap_err();
    assert!(err.to_string().contains("must assign a pandas DataFrame to `result`"), "{err}");
}

#[test]
fn reports_user_exception_with_line() {
    let code = "a = 1\nresult = df['missing_column']";
    let err = runner("exc").run(code, &input(), Duration::from_secs(30), &AtomicBool::new(false)).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("KeyError") && msg.contains("line 2"), "{msg}");
}

#[test]
fn enforces_timeout() {
    let t0 = Instant::now();
    let err = runner("timeout")
        .run("import time\ntime.sleep(30)\nresult = df", &input(), Duration::from_millis(1500), &AtomicBool::new(false))
        .unwrap_err();
    assert!(err.to_string().contains("timed out"), "{err}");
    assert!(t0.elapsed() < Duration::from_secs(10));
}

#[test]
fn honours_cancellation() {
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(800));
        c2.store(true, Ordering::SeqCst);
    });
    let t0 = Instant::now();
    let err = runner("cancel").run("import time\ntime.sleep(30)\nresult = df", &input(), Duration::from_secs(60), &cancel).unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err}");
    assert!(t0.elapsed() < Duration::from_secs(10));
}

#[test]
fn enforces_memory_limit() {
    let mut r = runner("mem");
    r.memory_limit_mb = Some(1024);
    let code = "blob = bytearray(4 * 1024 * 1024 * 1024)\nresult = df";
    let err = r.run(code, &input(), Duration::from_secs(30), &AtomicBool::new(false)).unwrap_err();
    assert!(err.to_string().contains("out of memory"), "{err}");
}

#[test]
fn child_does_not_inherit_environment() {
    std::env::set_var("VDA_TEST_SECRET", "hunter2");
    let code = "import os\nresult = pd.DataFrame({'secret': [os.environ.get('VDA_TEST_SECRET', 'absent')]})";
    let out = runner("env").run(code, &input(), Duration::from_secs(30), &AtomicBool::new(false)).unwrap();
    assert_eq!(out.rows[0][0], Value::Text("absent".into()));
}

#[test]
fn identity_step_preserves_float_values_exactly() {
    let t = read_table("x,y\n1,28.153846153846153\n2,0.1\n3,1e-300\n").unwrap();
    let out = runner("exact").run("result = df", &t, Duration::from_secs(30), &AtomicBool::new(false)).unwrap();
    assert_eq!(out, t);
}

#[test]
fn works_with_a_relative_work_directory() {
    let rel = format!("target/vda-py-rel-{}", std::process::id());
    let r = PythonRunner::new("python3", &rel);
    let out = r.run("result = df", &input(), Duration::from_secs(30), &AtomicBool::new(false)).unwrap();
    assert_eq!(out, input());
    let _ = std::fs::remove_dir_all(rel);
}
