//! The console beside a code step, against real Python and R.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vda_core::console::{Console, Language, Shown, Start};
use vda_core::params::Parameter;
use vda_core::table::Value;

fn work(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vda-console-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn inputs(dir: &std::path::Path) -> Vec<(String, PathBuf)> {
    let sales = vda_core::csv::read_table("region,units,revenue\nEast,3,30.5\nWest,1,10\nEast,2,20\n").unwrap();
    let targets = vda_core::csv::read_table("region,target\nEast,40\nWest,5\n").unwrap();
    let (a, b) = (dir.join("sales.parquet"), dir.join("targets.parquet"));
    vda_core::duck::write_parquet(&sales, &a).unwrap();
    vda_core::duck::write_parquet(&targets, &b).unwrap();
    vec![("sales".into(), a), ("targets".into(), b)]
}

fn start(language: Language, name: &str) -> Option<Console> {
    let dir = work(name);
    let ins = inputs(&dir);
    let params = [Parameter { name: "min_units".into(), value: Value::Int(2) }];
    let (program, libraries) = match language {
        Language::Python => (vda_core::python::default_interpreter(), vec![]),
        Language::R => {
            let mut r = vda_core::rlang::RRunner::new(vda_core::rlang::default_rscript(), &dir);
            if r.fingerprint().is_none() {
                eprintln!("skipped: R is not available");
                return None;
            }
            (vda_core::rlang::default_rscript(), r.libraries())
        }
    };
    let c = Console::start(
        Start { language, program: &program, inputs: &ins, params: &params, libraries: &libraries, work_root: &dir, memory_limit_mb: Some(4096) },
        Duration::from_secs(60),
    );
    Some(c.unwrap_or_else(|e| panic!("{e}")))
}

fn no() -> AtomicBool {
    AtomicBool::new(false)
}

const T: Duration = Duration::from_secs(60);

#[test]
fn python_keeps_state_and_shows_values_like_a_notebook() {
    let mut c = start(Language::Python, "py").unwrap();
    assert!(c.objects.iter().any(|o| o.name == "df" && o.summary == "3 rows × 3 columns"), "{:?}", c.objects);
    let r = c.run("big = df[df['units'] >= params['min_units']]\nprint('kept', len(big))", T, &no()).unwrap();
    assert!(r.ok && r.stdout.contains("kept 2"), "{r:?}");
    assert!(r.objects.iter().any(|o| o.name == "big"));
    // A later run sees what an earlier one made; the last value shows as a table.
    let r = c.run("big.groupby('region', as_index=False)['revenue'].sum()", T, &no()).unwrap();
    let Some(Shown::Table(t)) = &r.value else { panic!("{r:?}") };
    assert_eq!((t.columns[0].0.as_str(), t.total), ("region", 1));
    let r = c.run("len(dfs['targets'])", T, &no()).unwrap();
    assert_eq!(r.value, Some(Shown::Text("2".into())));
    // Errors say where, relative to the code that was run.
    let r = c.run("x = 1\ny = x + nope", T, &no()).unwrap();
    assert!(!r.ok);
    assert_eq!(r.error, Some(("NameError: name 'nope' is not defined".into(), Some(2))));
    let r = c.run("def f(:\n  pass", T, &no()).unwrap();
    assert!(r.error.as_ref().unwrap().0.starts_with("syntax error"), "{r:?}");
    assert!(c.run("x", T, &no()).unwrap().ok, "state survives an error");
}

#[test]
fn python_plots_and_interrupt_and_result() {
    let mut c = start(Language::Python, "py2").unwrap();
    let has_mpl = c.run("import matplotlib", T, &no()).unwrap().ok;
    if has_mpl {
        let r = c.run("import matplotlib.pyplot as plt\nplt.bar(df['region'], df['revenue'])", T, &no()).unwrap();
        assert_eq!(r.images.len(), 1, "{r:?}");
        assert!(std::fs::read(&r.images[0]).unwrap().starts_with(b"\x89PNG"));
    }
    // Interrupt long-running code; the console keeps its state.
    c.run("keep = 41", T, &no()).unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        f2.store(true, Ordering::SeqCst);
    });
    let started = Instant::now();
    let r = c.run("import time\nwhile True: time.sleep(0.05)", T, &flag).unwrap();
    assert_eq!(r.error.as_ref().map(|e| e.0.as_str()), Some("interrupted"));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(c.run("keep + 1", T, &no()).unwrap().value, Some(Shown::Text("42".into())));

    let out = work("py2").join("console-result.parquet");
    assert!(c.save_result(&out, T).unwrap_err().to_string().contains("no data frame named `result`"));
    c.run("result = df.merge(dfs['targets'], on='region')", T, &no()).unwrap();
    c.save_result(&out, T).unwrap();
    assert_eq!(vda_core::duck::read_parquet(&out).unwrap().rows.len(), 3);
}

#[test]
fn r_keeps_state_and_shows_values_like_the_r_console() {
    let Some(mut c) = start(Language::R, "r") else { return };
    assert!(c.version.starts_with('4'), "{}", c.version);
    assert!(c.objects.iter().any(|o| o.name == "df" && o.summary == "3 obs. of 3 variables"), "{:?}", c.objects);
    let r = c.run("big <- df[df$units >= params$min_units, ]\ncat('kept', nrow(big), '\\n')\nnrow(big)", T, &no()).unwrap();
    assert!(r.ok, "{r:?}");
    assert!(r.stdout.contains("kept 2") && r.stdout.contains("[1] 2"), "{}", r.stdout);
    let r = c.run("aggregate(revenue ~ region, data = big, FUN = sum)", T, &no()).unwrap();
    let Some(Shown::Table(t)) = &r.value else { panic!("{r:?}") };
    assert_eq!(t.columns, [("region".to_string(), "character".to_string()), ("revenue".into(), "numeric".into())]);
    assert_eq!(t.rows[0][1], vda_core::json::Json::Num(50.5));
    let r = c.run("x <- 1\ny <- x + nope", T, &no()).unwrap();
    assert_eq!(r.error, Some(("object 'nope' not found".into(), Some(2))));
    let r = c.run("message('hello'); warning('careful'); 1", T, &no()).unwrap();
    assert!(r.stderr.contains("hello") && r.stderr.contains("careful"), "{r:?}");
    let r = c.run("y <- (1 +", T, &no()).unwrap();
    assert!(r.error.as_ref().unwrap().0.starts_with("syntax error"), "{r:?}");
    // Plots, base and ggplot2.
    let r = c.run("plot(df$units, df$revenue)", T, &no()).unwrap();
    assert_eq!(r.images.len(), 1, "{r:?}");
    let r = c.run("x <- 2", T, &no()).unwrap();
    assert!(r.images.is_empty(), "no plot, no image: {r:?}");
    if c.run("library(ggplot2)", T, &no()).unwrap().ok {
        let r = c.run("ggplot(df, aes(region, revenue)) + geom_col()", T, &no()).unwrap();
        assert_eq!(r.images.len(), 1, "{r:?}");
    }
    let out = work("r").join("console-result.parquet");
    c.run("result <- merge(df, dfs$targets, by = 'region')", T, &no()).unwrap();
    c.save_result(&out, T).unwrap();
    assert_eq!(vda_core::duck::read_parquet(&out).unwrap().rows.len(), 3);
}

#[test]
fn r_interrupt_keeps_the_console() {
    let Some(mut c) = start(Language::R, "r2") else { return };
    c.run("keep <- 41", T, &no()).unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        f2.store(true, Ordering::SeqCst);
    });
    let r = c.run("repeat Sys.sleep(0.05)", T, &flag);
    match r {
        Ok(r) => {
            assert_eq!(r.error.as_ref().map(|e| e.0.as_str()), Some("interrupted"), "{r:?}");
            let r = c.run("keep + 1", T, &no()).unwrap();
            assert!(r.stdout.contains("[1] 42"), "{r:?}");
        }
        Err(e) => panic!("interrupting R closed the console: {e}"),
    }
}

#[test]
fn a_line_answers_quickly_once_the_console_is_up() {
    // The point of a console: no reload per try. Generous for slow CI.
    for lang in [Language::Python, Language::R] {
        let Some(mut c) = start(lang, if lang == Language::R { "fast-r" } else { "fast-py" }) else { continue };
        c.run("1 + 1", T, &no()).unwrap();
        let mut worst = Duration::ZERO;
        for _ in 0..5 {
            worst = worst.max(c.run(if lang == Language::R { "nrow(df)" } else { "len(df)" }, T, &no()).unwrap().elapsed);
        }
        eprintln!("{lang:?}: slowest of 5 runs {worst:?}");
        assert!(worst < Duration::from_millis(500), "{lang:?} took {worst:?}");
    }
}
