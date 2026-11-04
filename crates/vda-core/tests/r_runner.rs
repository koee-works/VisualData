//! R steps against a real R installation (`Rscript`, or `$VDA_R`). Skipped
//! when R cannot start.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vda_core::params::Parameter;
use vda_core::rlang::{params_r, RRunner};
use vda_core::table::{DataType, Schema, Table, Value};

fn work(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vda-r-tests-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn runner(name: &str) -> Option<RRunner> {
    let mut r = RRunner::new(vda_core::rlang::default_rscript(), work(name));
    if r.fingerprint().is_none() {
        eprintln!("skipped: R is not available");
        return None;
    }
    Some(r)
}

/// Run `code` over tables named by id; the result read back.
fn run(r: &mut RRunner, name: &str, code: &str, inputs: &[(&str, &Table)], params: &[Parameter], timeout: Duration, cancel: &AtomicBool) -> Result<(Table, String), vda_core::Error> {
    let dir = work(name);
    let named: Vec<(String, PathBuf)> = inputs
        .iter()
        .enumerate()
        .map(|(i, (id, t))| {
            let p = dir.join(format!("in{i}.parquet"));
            vda_core::duck::write_parquet(t, &p).unwrap();
            (id.to_string(), p)
        })
        .collect();
    let out = dir.join("out.parquet");
    let run = r.run(code, &named, &params_r(params), &out, timeout, cancel);
    run.result?;
    Ok((vda_core::duck::read_parquet(&out)?, run.output))
}

fn ok(r: &mut RRunner, name: &str, code: &str, inputs: &[(&str, &Table)]) -> (Table, String) {
    run(r, name, code, inputs, &[], Duration::from_secs(60), &AtomicBool::new(false)).unwrap_or_else(|e| panic!("{e}"))
}

fn err(r: &mut RRunner, name: &str, code: &str, inputs: &[(&str, &Table)]) -> String {
    match run(r, name, code, inputs, &[], Duration::from_secs(60), &AtomicBool::new(false)) {
        Ok(_) => panic!("expected an error from {code:?}"),
        Err(e) => e.to_string(),
    }
}

fn every_type() -> Table {
    Table {
        schema: Schema::new(vec![
            ("id", DataType::Int),
            ("big", DataType::Int),
            ("x", DataType::Float),
            ("label", DataType::Text),
            ("ok", DataType::Bool),
            ("day", DataType::Date),
            ("at", DataType::Timestamp),
            ("Unit price (€)", DataType::Float),
        ]),
        rows: vec![
            vec![
                Value::Int(1),
                Value::Int(9_007_199_254_740_993),
                Value::Float(0.1 + 0.2),
                Value::Text("say \"hi\", then\nleave".into()),
                Value::Bool(true),
                Value::Date(20_000),
                Value::Timestamp(1_751_358_600_123_456),
                Value::Float(19.99),
            ],
            vec![Value::Int(2), Value::Null, Value::Float(-1e-300), Value::Text(String::new()), Value::Bool(false), Value::Null, Value::Null, Value::Null],
            vec![Value::Int(3), Value::Int(-5), Value::Null, Value::Null, Value::Null, Value::Date(-1), Value::Timestamp(-500_000), Value::Float(1e21)],
        ],
    }
}

#[test]
fn every_type_survives_a_round_trip() {
    let Some(mut r) = runner("types") else { return };
    let t = every_type();
    let (back, _) = ok(&mut r, "types", "result <- df", &[("input", &t)]);
    let names: Vec<&str> = back.schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["id", "big", "x", "label", "ok", "day", "at", "Unit price (€)"]);
    let types: Vec<DataType> = back.schema.fields.iter().map(|f| f.dtype).collect();
    // `big` holds a value past R's integers, so it comes back as decimals.
    assert_eq!(types, [DataType::Int, DataType::Float, DataType::Float, DataType::Text, DataType::Bool, DataType::Date, DataType::Timestamp, DataType::Float]);
    assert_eq!(back.rows[0][2], Value::Float(0.1 + 0.2), "doubles are exact");
    assert_eq!(back.rows[1][2], Value::Float(-1e-300));
    assert_eq!(back.rows[0][3], Value::Text("say \"hi\", then\nleave".into()));
    assert_eq!(back.rows[1][3], Value::Text(String::new()), "empty text is not missing");
    assert_eq!(back.rows[2][3], Value::Null);
    assert_eq!(back.rows[0][5], Value::Date(20_000));
    assert_eq!(back.rows[2][5], Value::Date(-1));
    assert_eq!(back.rows[0][6], Value::Timestamp(1_751_358_600_123_456), "microseconds kept");
    assert_eq!(back.rows[2][6], Value::Timestamp(-500_000));
    assert_eq!(back.rows[2][7], Value::Float(1e21));
    assert_eq!(back.rows[1][4], Value::Bool(false));
    assert_eq!(back.rows[2][4], Value::Null);
}

#[test]
fn base_r_and_dplyr_and_parameters() {
    let Some(mut r) = runner("dplyr") else { return };
    let sales = vda_core::csv::read_table("region,units,revenue\nEast,3,30.5\nWest,1,10\nEast,2,20\n").unwrap();
    let targets = vda_core::csv::read_table("region,target\nEast,40\nWest,5\n").unwrap();
    let params = [Parameter { name: "goal".into(), value: Value::Float(0.5) }, Parameter { name: "min_units".into(), value: Value::Int(2) }];

    let code = "result <- aggregate(revenue ~ region, data = df[df$units >= params$min_units, ], FUN = sum)\nresult$share <- result$revenue / sum(result$revenue)\nresult$met <- result$share >= params$goal";
    let (t, _) = run(&mut r, "base", code, &[("sales", &sales)], &params, Duration::from_secs(60), &AtomicBool::new(false)).unwrap();
    assert_eq!(t.rows, vec![vec![Value::Text("East".into()), Value::Float(50.5), Value::Float(1.0), Value::Bool(true)]]);

    let has_dplyr = std::process::Command::new("Rscript").args(["-e", "quit(status = !requireNamespace('dplyr', quietly = TRUE))"]).status().map(|s| s.success()).unwrap_or(false);
    if has_dplyr {
        let code = "library(dplyr)\nresult <- df |>\n  group_by(region) |>\n  summarise(units = sum(units), .groups = \"drop\") |>\n  left_join(dfs$targets, by = \"region\") |>\n  mutate(ratio = units / target) |>\n  arrange(desc(ratio))";
        let (t, printed) = ok(&mut r, "dplyr", code, &[("sales", &sales), ("targets", &targets)]);
        assert_eq!(t.schema.fields.iter().map(|f| (f.name.as_str(), f.dtype)).collect::<Vec<_>>(), [("region", DataType::Text), ("units", DataType::Int), ("target", DataType::Int), ("ratio", DataType::Float)]);
        assert_eq!(t.rows[0][0], Value::Text("West".into()));
        assert!(!printed.contains("Attaching package"), "library() startup messages are R's, not the step's: {printed}");
    } else {
        eprintln!("dplyr not installed: skipped that part");
    }
}

#[test]
fn several_inputs_by_id_and_printed_output() {
    let Some(mut r) = runner("multi") else { return };
    let a = vda_core::csv::read_table("k,v\n1,10\n2,20\n").unwrap();
    let b = vda_core::csv::read_table("k,w\n2,200\n3,300\n").unwrap();
    let (t, printed) = ok(&mut r, "multi", "print(nrow(dfs[[\"other set\"]]))\nmessage(\"merging\")\nresult <- merge(df, dfs[[\"other set\"]], by = \"k\")", &[("main", &a), ("other set", &b)]);
    assert_eq!(t.rows, vec![vec![Value::Int(2), Value::Int(20), Value::Int(200)]]);
    assert!(printed.contains("[1] 2") && printed.contains("merging"), "{printed}");
}

#[test]
fn errors_say_what_and_where() {
    let Some(mut r) = runner("errors") else { return };
    let t = vda_core::csv::read_table("a\n1\n").unwrap();
    let e = err(&mut r, "e1", "x <- 1\nresult <- df[, \"nope\"]", &[("input", &t)]);
    assert!(e.starts_with("R step: ") && e.contains("undefined columns") && e.contains("(line 2)"), "{e}");
    let e = err(&mut r, "e2", "x <- 1\ny <- (2 +\nresult <- df )) ", &[("input", &t)]);
    assert!(e.contains("syntax error") && e.contains("(line 3)"), "{e}");
    let e = err(&mut r, "e3", "res <- df", &[("input", &t)]);
    assert!(e.contains("must assign a data frame to `result` (got nothing)"), "{e}");
    let e = err(&mut r, "e4", "result <- 42", &[("input", &t)]);
    assert!(e.contains("(got numeric)"), "{e}");
    let e = err(&mut r, "e5", "result <- df\nresult$l <- list(1, 2)[1]", &[("input", &t)]);
    assert!(e.contains("holds lists"), "{e}");
    let e = err(&mut r, "e6", "stop(\"bad input: \", nrow(df), \" rows\")", &[("input", &t)]);
    assert!(e.contains("bad input: 1 rows (line 1)"), "{e}");
    // Warnings are kept with the output, with their line.
    let (_, printed) = ok(&mut r, "warn", "x <- as.integer(\"seven\")\nresult <- df", &[("input", &t)]);
    assert!(printed.contains("Warning (line 1): NAs introduced by coercion"), "{printed}");
}

#[test]
fn timeout_and_cancel_stop_r() {
    let Some(mut r) = runner("stop") else { return };
    let t = vda_core::csv::read_table("a\n1\n").unwrap();
    let started = Instant::now();
    let e = run(&mut r, "slow", "Sys.sleep(30)\nresult <- df", &[("input", &t)], &[], Duration::from_millis(1500), &AtomicBool::new(false)).unwrap_err();
    assert!(e.to_string().contains("timed out"), "{e}");
    assert!(started.elapsed() < Duration::from_secs(10));

    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        c2.store(true, Ordering::SeqCst);
    });
    let started = Instant::now();
    let e = run(&mut r, "cancel", "Sys.sleep(30)\nresult <- df", &[("input", &t)], &[], Duration::from_secs(60), &cancel).unwrap_err();
    assert!(matches!(e, vda_core::Error::Cancelled), "{e}");
    assert!(started.elapsed() < Duration::from_secs(5));
}
