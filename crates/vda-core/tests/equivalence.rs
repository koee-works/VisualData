//! Cross-mode equivalence: every visual operation must give the same result
//! when compiled to SQL and run by DuckDB as when evaluated by the reference
//! engine. Also checks that both engines import CSV identically.

use std::path::{Path, PathBuf};
use vda_core::duck::DuckDbEngine;
use vda_core::engine::{Engine, ReferenceEngine};
use vda_core::model::{AggFunc, CmpOp, Measure, Predicate, Step, StepKind, SOURCE_ID};
use vda_core::table::{Table, Value};

const EDGE_CSV: &str = "\
k,g,flag,n,x,s
a,1,true,5,1.5,O'Hare
a,1,false,-3,2.25,\"quote \"\"q\"\"\"
b,,true,,-0.5,
,2,,7,,zeta
b,2,false,0,1e10,Alpha
a,,true,12,3,beta
c,3,true,4,0.1,
";

fn write(name: &str, content: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-equiv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, content).unwrap();
    p
}

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue/sales.csv")
}

fn close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x == y || (x - y).abs() <= 1e-12 * x.abs().max(y.abs()),
        _ => a == b,
    }
}

fn assert_same(case: &str, r: &Table, d: &Table) {
    assert_eq!(r.schema, d.schema, "{case}: schema differs");
    assert_eq!(r.rows.len(), d.rows.len(), "{case}: row count differs\nref: {:?}\nduck: {:?}", r.rows, d.rows);
    for (i, (a, b)) in r.rows.iter().zip(&d.rows).enumerate() {
        assert!(
            a.iter().zip(b).all(|(x, y)| close(x, y)),
            "{case}: row {i} differs\nref:  {a:?}\nduck: {b:?}"
        );
    }
}

fn pred(column: &str, op: CmpOp, value: Value) -> Predicate {
    Predicate { column: column.into(), op, value }
}

fn m(func: AggFunc, column: Option<&str>, alias: &str) -> Measure {
    Measure { func, column: column.map(Into::into), alias: alias.into() }
}

fn every_measure(col: &str) -> Vec<Measure> {
    vec![
        m(AggFunc::Count, None, "rows"),
        m(AggFunc::Count, Some(col), "present"),
        m(AggFunc::Sum, Some(col), "sum"),
        m(AggFunc::Avg, Some(col), "avg"),
        m(AggFunc::Min, Some(col), "min"),
        m(AggFunc::Max, Some(col), "max"),
    ]
}

fn edge_cases() -> Vec<(&'static str, StepKind)> {
    use CmpOp::*;
    let f = |p: Vec<Predicate>| StepKind::Filter { predicates: p };
    let a = |g: Vec<&str>, ms: Vec<Measure>| StepKind::Aggregate {
        group_by: g.into_iter().map(Into::into).collect(),
        measures: ms,
    };
    vec![
        ("no predicates", f(vec![])),
        ("int eq", f(vec![pred("n", Eq, Value::Int(5))])),
        ("int ne skips nulls", f(vec![pred("n", Ne, Value::Int(5))])),
        ("int vs float literal", f(vec![pred("n", Gt, Value::Float(1.5))])),
        ("float lt", f(vec![pred("x", Lt, Value::Int(2))])),
        ("float le", f(vec![pred("x", Le, Value::Float(2.25))])),
        ("float ge negative", f(vec![pred("x", Ge, Value::Float(-0.5))])),
        ("text ordering is binary", f(vec![pred("s", Ge, Value::Text("a".into()))])),
        ("text with quote", f(vec![pred("s", Eq, Value::Text("O'Hare".into()))])),
        ("bool eq", f(vec![pred("flag", Eq, Value::Bool(true))])),
        ("is null", f(vec![pred("k", IsNull, Value::Null)])),
        ("is not null and", f(vec![pred("g", IsNotNull, Value::Null), pred("x", Gt, Value::Int(0))])),
        ("filter to empty", f(vec![pred("n", Gt, Value::Int(1000))])),
        ("group text, all measures int", a(vec!["k"], every_measure("n"))),
        ("group text, all measures float", a(vec!["k"], every_measure("x"))),
        ("group with null keys", a(vec!["g"], every_measure("x"))),
        ("group by two keys", a(vec!["k", "flag"], vec![m(AggFunc::Count, None, "rows")])),
        ("group bool", a(vec!["flag"], vec![m(AggFunc::Sum, Some("n"), "sum_n")])),
        ("min/max text", a(vec!["flag"], vec![m(AggFunc::Min, Some("s"), "lo"), m(AggFunc::Max, Some("s"), "hi")])),
        ("global aggregate", a(vec![], every_measure("x"))),
        ("group only", a(vec!["k"], vec![])),
    ]
}

fn run_both(source: &Path, case: &str, kind: StepKind) {
    let mut reference = ReferenceEngine;
    let mut duck = DuckDbEngine::new().unwrap();
    let r_src = reference.import_csv(source).unwrap();
    let d_src = duck.import_csv(source).unwrap();
    assert_same(&format!("{case} (import)"), &r_src, &d_src);
    let step = Step { id: "s".into(), input: SOURCE_ID.into(), label: None, kind };
    let r = reference.run_step(&step, &r_src).unwrap_or_else(|e| panic!("{case}: reference: {e}"));
    let d = duck.run_step(&step, &d_src).unwrap_or_else(|e| panic!("{case}: duckdb: {e}"));
    assert_same(case, &r, &d);
}

#[test]
fn csv_import_agrees() {
    for src in [sample(), write("edge.csv", EDGE_CSV)] {
        let r = ReferenceEngine.import_csv(&src).unwrap();
        let d = DuckDbEngine::new().unwrap().import_csv(&src).unwrap();
        assert_same(&src.display().to_string(), &r, &d);
    }
}

#[test]
fn visual_ops_agree_on_edge_cases() {
    let src = write("edge.csv", EDGE_CSV);
    for (case, kind) in edge_cases() {
        run_both(&src, case, kind);
    }
}

#[test]
fn visual_ops_agree_on_sample_data() {
    let demo = vda_core::demo::regional_revenue("sales.csv");
    let src = sample();
    for id in ["valid_orders", "by_region"] {
        // Chain: by_region reads valid_orders, so evaluate both engines step by step.
        let mut reference = ReferenceEngine;
        let mut duck = DuckDbEngine::new().unwrap();
        let (mut r, mut d) = (reference.import_csv(&src).unwrap(), duck.import_csv(&src).unwrap());
        for step in &demo.steps {
            r = reference.run_step(step, &r).unwrap();
            d = duck.run_step(step, &d).unwrap();
            assert_same(&step.id, &r, &d);
            if step.id == id {
                break;
            }
        }
    }
}

#[test]
fn integer_sum_overflow_is_an_error_in_both() {
    let src = write("big.csv", "k,n\na,9223372036854775000\na,1000\n");
    let step = Step {
        id: "s".into(),
        input: SOURCE_ID.into(),
        label: None,
        kind: StepKind::Aggregate { group_by: vec!["k".into()], measures: vec![m(AggFunc::Sum, Some("n"), "total")] },
    };
    let r_src = ReferenceEngine.import_csv(&src).unwrap();
    assert!(ReferenceEngine.run_step(&step, &r_src).is_err());
    let mut duck = DuckDbEngine::new().unwrap();
    let d_src = duck.import_csv(&src).unwrap();
    let err = duck.run_step(&step, &d_src).unwrap_err().to_string();
    assert!(err.to_lowercase().contains("out of range") || err.to_lowercase().contains("overflow"), "{err}");
}

#[test]
fn validation_messages_match() {
    let src = write("edge.csv", EDGE_CSV);
    let step = Step {
        id: "s".into(),
        input: SOURCE_ID.into(),
        label: None,
        kind: StepKind::Filter { predicates: vec![pred("missing", CmpOp::Eq, Value::Int(1))] },
    };
    let r = ReferenceEngine.run_step(&step, &ReferenceEngine.import_csv(&src).unwrap()).unwrap_err().to_string();
    let mut duck = DuckDbEngine::new().unwrap();
    let d_src = duck.import_csv(&src).unwrap();
    let d = duck.run_step(&step, &d_src).unwrap_err().to_string();
    assert_eq!(r, d);
}

/// The SQL shown to users must be what the steps mean: executing the combined
/// inspectable query gives the same result as running the steps one by one.
#[test]
fn inspectable_sql_executes_to_the_same_result() {
    let src = sample();
    let mut a = vda_core::demo::regional_revenue(&src.to_string_lossy());
    a.steps.truncate(2); // valid_orders, by_region (no Python in the chain)
    a.cells.clear();
    a.steps.push(Step {
        id: "ranked".into(),
        input: "by_region".into(),
        label: None,
        kind: StepKind::Sql {
            sql: "WITH r AS (SELECT region, revenue, rank() OVER (ORDER BY revenue DESC) AS pos FROM input)\nSELECT * FROM r ORDER BY pos DESC".into(),
        },
    });
    let mut duck = DuckDbEngine::new().unwrap();
    let mut t = duck.import_csv(&src).unwrap();
    for step in &a.steps {
        t = duck.run_step(step, &t).unwrap();
        let combined = vda_core::compile::pipeline_sql(&a, &step.id).unwrap();
        let direct = DuckDbEngine::new().unwrap().query(&combined).unwrap_or_else(|e| panic!("{e}\n{combined}"));
        assert_same(&format!("pipeline for {}", step.id), &t, &direct);
        if step.id == "ranked" {
            break;
        }
    }
    // The final ORDER BY (DESC) is honoured, so the last row is rank 1.
    assert_eq!(t.rows.last().unwrap()[2], Value::Int(1));
}

const DATES_CSV: &str = "\
id,d,ts,ts_t,mixed,us_style,invalid
1,2026-07-03,2026-07-03 10:00:00,2026-07-03T10:00:00,2026-07-03,07/03/2026,2026-13-01
2,2026-12-31,2026-12-31 23:59:59.5,2026-12-31T23:59:59,2026-07-04 01:02:03,12/31/2026,2026-02-30
3,,,,,,
4,1969-12-31,1969-12-31 23:59:59.123456,1969-12-31T00:00:00,2026-01-01,01/01/1969,2026-01-01
5,2026-07-03,2026-07-03 00:00:00,2026-07-03T00:00:01,2026-01-02,07/03/2026,2026-01-02
";

#[test]
fn date_and_timestamp_import_agrees() {
    use vda_core::table::DataType::*;
    let src = write("dates.csv", DATES_CSV);
    let r = ReferenceEngine.import_csv(&src).unwrap();
    let d = DuckDbEngine::new().unwrap().import_csv(&src).unwrap();
    assert_same("dates import", &r, &d);
    let types: Vec<_> = d.schema.fields.iter().map(|f| f.dtype).collect();
    // Bare dates and timestamps mixed in one column, US-style dates and
    // impossible dates all stay text, in both engines.
    assert_eq!(types, vec![Int, Date, Timestamp, Timestamp, Text, Text, Text]);
}

#[test]
fn visual_ops_agree_on_dates() {
    use vda_core::table::{parse_date, parse_timestamp};
    use CmpOp::*;
    let src = write("dates.csv", DATES_CSV);
    let date = |s| Value::Date(parse_date(s).unwrap());
    let ts = |s| Value::Timestamp(parse_timestamp(s).unwrap());
    let f = |p: Vec<Predicate>| StepKind::Filter { predicates: p };
    let cases = vec![
        ("date ge", f(vec![pred("d", Ge, date("2026-07-03"))])),
        ("date before epoch", f(vec![pred("d", Lt, date("1970-01-01"))])),
        ("timestamp lt fractional", f(vec![pred("ts", Lt, ts("2026-12-31 23:59:59.5"))])),
        ("date column vs timestamp literal", f(vec![pred("d", Lt, ts("2026-07-03 00:00:01"))])),
        ("timestamp column vs date literal", f(vec![pred("ts", Eq, date("2026-07-03"))])),
        ("date is null", f(vec![pred("d", IsNull, Value::Null)])),
        (
            "group by date, min/max timestamp",
            StepKind::Aggregate {
                group_by: vec!["d".into()],
                measures: vec![
                    m(AggFunc::Count, None, "rows"),
                    m(AggFunc::Min, Some("ts"), "first"),
                    m(AggFunc::Max, Some("ts_t"), "last"),
                ],
            },
        ),
        (
            "global min/max date",
            StepKind::Aggregate {
                group_by: vec![],
                measures: vec![m(AggFunc::Min, Some("d"), "lo"), m(AggFunc::Max, Some("d"), "hi")],
            },
        ),
    ];
    for (case, kind) in cases {
        run_both(&src, case, kind);
    }
}

#[test]
fn sum_of_dates_is_rejected_before_execution() {
    let src = write("dates.csv", DATES_CSV);
    let step = Step {
        id: "s".into(),
        input: SOURCE_ID.into(),
        label: None,
        kind: StepKind::Aggregate { group_by: vec![], measures: vec![m(AggFunc::Sum, Some("d"), "x")] },
    };
    let mut duck = DuckDbEngine::new().unwrap();
    let t = duck.import_csv(&src).unwrap();
    let err = duck.run_step(&step, &t).unwrap_err().to_string();
    assert!(err.contains("needs a numeric column") && err.contains("DATE"), "{err}");
}
