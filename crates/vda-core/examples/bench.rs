//! Large-data benchmark.
//!
//!     cargo run --release -p vda-core --example bench -- [rows] [dir] [memory_limit_mb]
//!
//! Generates an orders CSV with `rows` rows (default 10,000,000), runs a
//! filter → aggregate → Python → chart analysis plus a custom SQL step, and
//! reports timings and this process's peak memory. DuckDB's own memory is
//! capped at `memory_limit_mb` (default 1024) and spills to disk beyond it.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;
use vda_core::duck::{DuckDbEngine, EngineOptions};
use vda_core::model::*;
use vda_core::python::{default_interpreter, PythonRunner};
use vda_core::session::Session;
use vda_core::table::Value;

/// (current RSS, peak RSS) of this process in MB, from /proc (Linux only).
fn memory_mb() -> (f64, f64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<f64>().ok())
            .map(|kb| kb / 1024.0)
            .unwrap_or(f64::NAN)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

fn generate(path: &PathBuf, rows: u64) -> std::io::Result<()> {
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path)?);
    writeln!(w, "order_id,order_date,customer,region,product,units,unit_price,revenue")?;
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..rows {
        // xorshift: fast, deterministic pseudo-random data
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let units = x % 9 + 1;
        let price = [12.0, 24.5, 49.0, 99.0][(x >> 8) as usize % 4];
        let region = if x.is_multiple_of(997) { String::new() } else { format!("r{}", (x >> 12) % 7) };
        let revenue = if x.is_multiple_of(1009) { String::new() } else { format!("{:.2}", units as f64 * price) };
        let day = 1 + (x >> 20) % 28;
        let month = 1 + (x >> 28) % 12;
        writeln!(
            w,
            "{i},2026-{month:02}-{day:02},c{},{region},p{},{units},{price},{revenue}",
            (x >> 32) % 100_000,
            (x >> 40) % 50
        )?;
    }
    w.flush()
}

fn analysis() -> Analysis {
    let st = |id: &str, input: &str, kind| Step { id: id.into(), inputs: vec![input.into()], label: None, kind };
    Analysis {
        parameters: vec![],
        name: "Orders benchmark".into(),
        sources: vec![Source::primary("orders.csv")],
        steps: vec![
            st(
                "valid",
                SOURCE_ID,
                StepKind::Filter {
                    predicates: vec![
                        Predicate { param: None, column: "region".into(), op: CmpOp::IsNotNull, value: Value::Null },
                        Predicate { param: None, column: "revenue".into(), op: CmpOp::Ge, value: Value::Int(20) },
                    ],
                },
            ),
            st(
                "by_region_product",
                "valid",
                StepKind::Aggregate {
                    group_by: vec!["region".into(), "product".into()],
                    measures: vec![
                        Measure { func: AggFunc::Count, column: None, alias: "orders".into() },
                        Measure { func: AggFunc::Sum, column: Some("revenue".into()), alias: "revenue".into() },
                        Measure { func: AggFunc::Avg, column: Some("unit_price".into()), alias: "avg_price".into() },
                    ],
                },
            ),
            st(
                "share",
                "by_region_product",
                StepKind::Python {
                    code: "result = df.assign(share=df['revenue'] / df['revenue'].sum())".into(),
                    timeout_ms: 120_000,
                },
            ),
            st(
                "chart",
                "share",
                StepKind::Chart { mark: Mark::Bar, x: "product".into(), y: "share".into(), title: None, color: None, summary: None },
            ),
            st(
                "top_customers",
                "valid",
                StepKind::Sql {
                    sql: "SELECT customer, count(*) AS orders, sum(revenue) AS revenue FROM input \
                          GROUP BY customer ORDER BY revenue DESC, customer LIMIT 100"
                        .into(),
                },
            ),
        ],
        cells: vec![],
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let rows: u64 = args.next().map(|a| a.parse()).transpose()?.unwrap_or(10_000_000);
    let dir: PathBuf = args.next().unwrap_or_else(|| "bench-project".into()).into();
    let mem: u64 = args.next().map(|a| a.parse()).transpose()?.unwrap_or(1024);
    std::fs::create_dir_all(&dir)?;
    let csv = dir.join("orders.csv");
    let engine = || {
        DuckDbEngine::with_options(EngineOptions { memory_limit_mb: Some(mem), temp_dir: Some(dir.join("spill")), allowed_paths: None })
    };
    let python = || Some(PythonRunner::new(default_interpreter(), dir.join(".work")));
    let note = |what: &str, value: String| {
        let (rss, _) = memory_mb();
        println!("{what:<48} {value:>18}   (RSS {rss:.0} MB)");
    };

    let t = Instant::now();
    if !csv.exists() {
        generate(&csv, rows)?;
    }
    note("generate CSV (skipped if present)", format!("{:.1} s", t.elapsed().as_secs_f64()));
    note("CSV size", format!("{:.0} MB", std::fs::metadata(&csv)?.len() as f64 / 1e6));

    let mut s = Session::new(analysis(), &dir, Box::new(engine()?), python())?;
    let t = Instant::now();
    let src = s.source_dataset(SOURCE_ID)?;
    note("import CSV → Parquet (first time)", format!("{:.1} s", t.elapsed().as_secs_f64()));
    note("source rows", src.rows.to_string());
    note("source Parquet size", format!("{:.0} MB", src.bytes as f64 / 1e6));

    let t = Instant::now();
    let run = s.run_all()?;
    let total = t.elapsed().as_secs_f64();
    for r in &run.steps {
        note(
            &format!("step `{}` ({})", r.step_id, r.status.name()),
            format!("{:.2} s, {} rows", r.duration_ms as f64 / 1000.0, r.rows.map(|n| n.to_string()).unwrap_or("-".into())),
        );
    }
    note("full run", format!("{total:.1} s"));
    if !run.succeeded() {
        return Err(format!("run failed: {run:#?}").into());
    }

    let t = Instant::now();
    let page = s.preview("valid", src.rows / 2, 50)?;
    note("preview 50 rows from the middle of `valid`", format!("{:.0} ms", t.elapsed().as_secs_f64() * 1000.0));
    assert_eq!(page.rows.len(), 50);

    let t = Instant::now();
    let profile = s.source_profile(SOURCE_ID)?;
    note("profile source (all columns)", format!("{:.1} s", t.elapsed().as_secs_f64()));
    assert_eq!(profile.len(), 8);

    let t = Instant::now();
    let again = s.run_all()?;
    note("rerun, nothing changed", format!("{:.0} ms", t.elapsed().as_secs_f64() * 1000.0));
    assert!(again.steps.iter().all(|r| matches!(r.status, vda_core::session::RunStatus::Succeeded { cached: true })));

    vda_core::persist::save_analysis(&s.analysis, &dir)?;
    let t = Instant::now();
    let reopened = Session::open(&dir, Box::new(engine()?), python())?;
    let fresh = s.analysis.steps.iter().filter(|st| reopened.state(&st.id).map(|x| x.label() == "fresh").unwrap_or(false)).count();
    note("reopen project, all steps fresh", format!("{:.0} ms ({fresh}/5)", t.elapsed().as_secs_f64() * 1000.0));

    let (_, peak) = memory_mb();
    note("peak RSS of this process (incl. DuckDB)", format!("{peak:.0} MB"));
    let artifacts: u64 = std::fs::read_dir(dir.join("artifacts"))?.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum();
    note("artifacts on disk", format!("{:.0} MB", artifacts as f64 / 1e6));
    Ok(())
}
