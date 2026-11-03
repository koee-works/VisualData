//! Window calculations, keeping one row per key, pivot and unpivot: the
//! reference engine and DuckDB must agree, including on missing values,
//! ties and errors.

use std::path::{Path, PathBuf};
use vda_core::duck::DuckDbEngine;
use vda_core::engine::{Engine, ReferenceEngine};
use vda_core::model::*;
use vda_core::table::{Table, Value};

const DATA: &str = "\
region,month,product,units,price,day,flag
East,2,A,5,1.5,2026-02-01,true
East,1,A,3,2.25,2026-01-01,false
West,1,B,,0.5,2026-01-01,true
East,1,B,7,,2026-01-01,
,2,A,1,4,2026-02-01,true
West,2,A,4,-1.25,,false
East,2,A,5,1.5,2026-02-01,true
West,3,B,0,3,2026-03-01,true
North,1,,2,1,2026-01-01,false
West,4,B,6,0.25,2026-04-01,true
";

fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("vda-reshape-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{name}-{}", N.fetch_add(1, Ordering::SeqCst)))
}

fn write(content: &str) -> PathBuf {
    let p = scratch("src").with_extension("csv");
    std::fs::write(&p, content).unwrap();
    p
}

fn import(e: &mut dyn Engine, src: &Path) -> vda_core::dataset::Dataset {
    e.import_csv(src, &scratch("import").with_extension("parquet")).unwrap()
}

fn run(e: &mut dyn Engine, src: &Path, kind: &StepKind) -> Result<Table, String> {
    let ds = import(e, src);
    let step = Step { id: "s".into(), inputs: vec![SOURCE_ID.into()], label: None, kind: kind.clone() };
    let out = e.run_step(&step, &[ds], &scratch("out").with_extension("parquet")).map_err(|e| e.to_string())?;
    Ok(vda_core::duck::read_parquet(&out.path).unwrap())
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
        assert!(a.iter().zip(b).all(|(x, y)| close(x, y)), "{case}: row {i} differs\nref:  {a:?}\nduck: {b:?}");
    }
}

/// Both engines give the same table, and return it.
fn both(src: &Path, case: &str, kind: StepKind) -> Table {
    let r = run(&mut ReferenceEngine::new(), src, &kind).unwrap_or_else(|e| panic!("{case}: reference: {e}"));
    let d = run(&mut DuckDbEngine::new().unwrap(), src, &kind).unwrap_or_else(|e| panic!("{case}: duckdb: {e}"));
    assert_same(case, &r, &d);
    r
}

fn key(c: &str, descending: bool) -> SortKey {
    SortKey { column: c.into(), descending }
}

fn calc(name: &str, func: WindowFunc, column: Option<&str>, n: Option<u32>) -> WindowColumn {
    WindowColumn { name: name.into(), func, column: column.map(str::to_string), n }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn ints(t: &Table, col: &str) -> Vec<Option<i64>> {
    let i = t.schema.index_of(col).unwrap();
    t.rows.iter().map(|r| if let Value::Int(n) = r[i] { Some(n) } else { None }).collect()
}

#[test]
fn window_calculations_agree() {
    use WindowFunc::*;
    let src = write(DATA);
    let t = both(&src, "by region and month", StepKind::Window {
        partition_by: strs(&["region"]),
        order_by: vec![key("month", false)],
        columns: vec![
            calc("run_units", RunningSum, Some("units"), None),
            calc("run_price", RunningSum, Some("price"), None),
            calc("row", RowNumber, None, None),
            calc("rank", Rank, None, None),
            calc("prev_units", Previous, Some("units"), None),
            calc("prev2_price", Previous, Some("price"), Some(2)),
            calc("next_units", Next, Some("units"), None),
            calc("ma2", MovingAverage, Some("units"), Some(2)),
            calc("ma3_price", MovingAverage, Some("price"), Some(3)),
            calc("chg", Change, Some("units"), None),
            calc("chg_price", Change, Some("price"), None),
            calc("pct", PercentChange, Some("units"), None),
            calc("share", ShareOfTotal, Some("units"), None),
            calc("share_price", ShareOfTotal, Some("price"), None),
        ],
    });
    // Rows keep their input order; East's two month-2 rows tie in rank.
    assert_eq!(t.rows.len(), 10);
    assert_eq!(ints(&t, "rank")[..2], [Some(3), Some(1)]);
    assert_eq!(ints(&t, "rank")[6], Some(3));
    assert_eq!(ints(&t, "row")[..2], [Some(3), Some(1)]);
    // West: units missing, 4, 0, 6. The running sum skips the missing value;
    // the change after 0 has no percentage.
    assert_eq!(ints(&t, "run_units")[2], None);
    assert_eq!(ints(&t, "run_units")[9], Some(10));
    let pct = t.schema.index_of("pct").unwrap();
    assert_eq!(t.rows[9][pct], Value::Null);

    both(&src, "whole table in input order", StepKind::Window {
        partition_by: vec![],
        order_by: vec![],
        columns: vec![calc("run", RunningSum, Some("units"), None), calc("row", RowNumber, None, None), calc("rank", Rank, None, None)],
    });
    both(&src, "descending with ties and missing last", StepKind::Window {
        partition_by: vec![],
        order_by: vec![key("price", true), key("units", false)],
        columns: vec![calc("rank", Rank, None, None), calc("row", RowNumber, None, None), calc("prev", Previous, Some("product"), Some(3))],
    });
    both(&src, "previous and next of text, dates and flags", StepKind::Window {
        partition_by: strs(&["flag"]),
        order_by: vec![key("day", false)],
        columns: vec![
            calc("prev_product", Previous, Some("product"), None),
            calc("next_day", Next, Some("day"), Some(2)),
            calc("prev_flag", Previous, Some("flag"), None),
        ],
    });
}

#[test]
fn window_errors_agree() {
    let src = write("k,n\na,9223372036854775000\na,1000\n");
    let kind = StepKind::Window {
        partition_by: vec![],
        order_by: vec![],
        columns: vec![calc("run", WindowFunc::RunningSum, Some("n"), None)],
    };
    assert!(run(&mut ReferenceEngine::new(), &src, &kind).is_err());
    let e = run(&mut DuckDbEngine::new().unwrap(), &src, &kind).unwrap_err().to_lowercase();
    assert!(e.contains("out of range") || e.contains("overflow"), "{e}");

    let src = write(DATA);
    for (kind, want) in [
        (
            StepKind::Window { partition_by: vec![], order_by: vec![], columns: vec![calc("x", WindowFunc::RunningSum, Some("product"), None)] },
            "running sum needs a numeric column, `product` is text",
        ),
        (
            StepKind::Window { partition_by: vec![], order_by: vec![], columns: vec![calc("units", WindowFunc::RowNumber, None, None)] },
            "there is already a column `units`",
        ),
        (
            StepKind::Window { partition_by: vec![], order_by: vec![], columns: vec![calc("m", WindowFunc::MovingAverage, Some("units"), None)] },
            "choose how many rows",
        ),
        (
            StepKind::Pivot { rows: vec![], column: "price".into(), value: None, func: AggFunc::Count },
            "cannot make columns from decimal column `price`",
        ),
        (
            StepKind::Unpivot { columns: strs(&["units", "product"]), name: "k".into(), value: "v".into() },
            "`units` is whole number but `product` is text",
        ),
    ] {
        let r = run(&mut ReferenceEngine::new(), &src, &kind).unwrap_err();
        let d = run(&mut DuckDbEngine::new().unwrap(), &src, &kind).unwrap_err();
        assert_eq!(r, d);
        assert!(r.contains(want), "{r}");
    }
}

#[test]
fn keeping_one_row_per_key_agrees() {
    let src = write(DATA);
    let t = both(&src, "identical rows", StepKind::Dedupe { keys: vec![], order_by: vec![] });
    assert_eq!(t.rows.len(), 9, "the repeated East row goes");
    let t = both(&src, "largest units per region", StepKind::Dedupe { keys: strs(&["region"]), order_by: vec![key("units", true)] });
    // The kept rows stay in input order: East's 7, the missing region's 1,
    // North's 2, West's 6 (its missing units sort last).
    assert_eq!(ints(&t, "units"), [Some(7), Some(1), Some(2), Some(6)]);
    both(&src, "first row per region and product", StepKind::Dedupe { keys: strs(&["region", "product"]), order_by: vec![] });
    both(&src, "missing keys are equal, missing order last", StepKind::Dedupe { keys: strs(&["flag"]), order_by: vec![key("price", false)] });
}

#[test]
fn unpivot_agrees() {
    let src = write(DATA);
    let t = both(&src, "numbers", StepKind::Unpivot { columns: strs(&["units", "price"]), name: "measure".into(), value: "amount".into() });
    assert_eq!(t.rows.len(), 20);
    assert_eq!(t.schema.fields.last().unwrap().dtype, vda_core::table::DataType::Float);
    assert_eq!(t.rows[0][t.schema.index_of("measure").unwrap()], Value::Text("units".into()));
    both(&src, "text", StepKind::Unpivot { columns: strs(&["product", "region"]), name: "field".into(), value: "text".into() });
    both(&src, "one column", StepKind::Unpivot { columns: strs(&["day"]), name: "which".into(), value: "when".into() });
}

#[test]
fn pivot_agrees() {
    let src = write(DATA);
    let t = both(&src, "units by region and month", StepKind::Pivot {
        rows: strs(&["region"]),
        column: "month".into(),
        value: Some("units".into()),
        func: AggFunc::Sum,
    });
    let names: Vec<&str> = t.schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["region", "1", "2", "3", "4"]);
    assert_eq!(t.rows.last().unwrap()[0], Value::Null, "missing region last");
    both(&src, "count rows by product", StepKind::Pivot { rows: strs(&["region"]), column: "product".into(), value: None, func: AggFunc::Count });
    both(&src, "average price by flag, one row", StepKind::Pivot { rows: vec![], column: "flag".into(), value: Some("price".into()), func: AggFunc::Avg });
    both(&src, "latest price by day", StepKind::Pivot {
        rows: strs(&["region", "product"]),
        column: "day".into(),
        value: Some("price".into()),
        func: AggFunc::Max,
    });
    for func in [AggFunc::Median, AggFunc::CountDistinct, AggFunc::Min, AggFunc::Count] {
        both(&src, &format!("{} of units", func.token()), StepKind::Pivot {
            rows: strs(&["product"]),
            column: "region".into(),
            value: Some("units".into()),
            func,
        });
    }
    // No rows: no values, so no new columns.
    let empty = write("region,k\n");
    let t = both(&empty, "empty", StepKind::Pivot { rows: strs(&["region"]), column: "k".into(), value: None, func: AggFunc::Count });
    assert_eq!((t.schema.fields.len(), t.rows.len()), (1, 0));
    // A value named like a row column gets a suffix.
    let clash = write("A,k,n\nx,A,1\ny,B,2\nx,B,3\n");
    let t = both(&clash, "name clash", StepKind::Pivot { rows: strs(&["A"]), column: "k".into(), value: Some("n".into()), func: AggFunc::Sum });
    let names: Vec<&str> = t.schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["A", "A_1", "B"]);
}

#[test]
fn bucket_agrees() {
    let src = write("n,x\n37,3.7\n-1,-0.25\n0,0\n,1e3\n9223372036854775807,2.5\n");
    let t = both(&src, "buckets", StepKind::Derive {
        columns: vec![
            Derived { name: "b10".into(), formula: "bucket(n, 10)".into() },
            Derived { name: "b_neg".into(), formula: "bucket(n, -5)".into() },
            Derived { name: "bx".into(), formula: "bucket(x, 0.5)".into() },
            Derived { name: "bnx".into(), formula: "bucket(n, 2.5)".into() },
        ],
    });
    assert_eq!(ints(&t, "b10")[..4], [Some(30), Some(-10), Some(0), None]);
    assert_eq!(ints(&t, "b_neg")[0], None);
}

/// The SQL shown for reshaping steps runs to the same result as the steps.
#[test]
fn inspectable_sql_of_reshaping_steps_executes_to_the_same_result() {
    let src = write(DATA);
    let step = |id: &str, input: &str, kind: StepKind| Step { id: id.into(), inputs: vec![input.into()], label: None, kind };
    let a = Analysis {
        name: "reshape".into(),
        sources: vec![Source::primary(src.to_string_lossy())],
        steps: vec![
            step("once", SOURCE_ID, StepKind::Dedupe { keys: vec![], order_by: vec![] }),
            step("calc", "once", StepKind::Window {
                partition_by: strs(&["region"]),
                order_by: vec![key("month", false)],
                columns: vec![
                    calc("run", WindowFunc::RunningSum, Some("units"), None),
                    calc("pct", WindowFunc::PercentChange, Some("price"), None),
                ],
            }),
            step("long", "calc", StepKind::Unpivot { columns: strs(&["units", "run"]), name: "what".into(), value: "n".into() }),
            step("wide", "long", StepKind::Pivot { rows: strs(&["region"]), column: "what".into(), value: Some("n".into()), func: AggFunc::Sum }),
        ],
        cells: vec![],
    };
    let mut duck = DuckDbEngine::new().unwrap();
    let mut tables: std::collections::HashMap<String, (vda_core::dataset::Dataset, Table)> = Default::default();
    let ds = import(&mut duck, &src);
    tables.insert(SOURCE_ID.into(), (ds.clone(), vda_core::duck::read_parquet(&ds.path).unwrap()));
    for s in &a.steps {
        let input = tables[&s.inputs[0]].0.clone();
        let out = duck.run_step(s, &[input], &scratch("out").with_extension("parquet")).unwrap_or_else(|e| panic!("{}: {e}", s.id));
        let t = vda_core::duck::read_parquet(&out.path).unwrap();
        tables.insert(s.id.clone(), (out, t));
        let sql = vda_core::compile::pipeline_sql_with(&a, &s.id, &|id| tables.get(id).map(|t| t.1.schema.clone())).unwrap();
        let direct = DuckDbEngine::new().unwrap().query(&sql).unwrap_or_else(|e| panic!("{e}\n{sql}"));
        assert_same(&format!("pipeline for {}", s.id), &tables[&s.id].1, &direct);
    }
    let names: Vec<String> = tables["wide"].1.schema.fields.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["region", "run", "units"]);
}

#[test]
fn reshaping_steps_round_trip_through_the_project_file() {
    let a = Analysis {
        name: "rt".into(),
        sources: vec![Source::primary("data.csv")],
        steps: vec![
            Step {
                id: "w".into(),
                inputs: vec![SOURCE_ID.into()],
                label: Some("Running".into()),
                kind: StepKind::Window {
                    partition_by: strs(&["region"]),
                    order_by: vec![key("month", true)],
                    columns: vec![calc("prev", WindowFunc::Previous, Some("units"), Some(2)), calc("r", WindowFunc::Rank, None, None)],
                },
            },
            Step { id: "d".into(), inputs: vec!["w".into()], label: None, kind: StepKind::Dedupe { keys: strs(&["region"]), order_by: vec![key("units", false)] } },
            Step {
                id: "p".into(),
                inputs: vec!["d".into()],
                label: None,
                kind: StepKind::Pivot { rows: vec![], column: "month".into(), value: Some("units".into()), func: AggFunc::Percentile(90) },
            },
            Step {
                id: "u".into(),
                inputs: vec!["d".into()],
                label: None,
                kind: StepKind::Unpivot { columns: strs(&["units", "price"]), name: "k".into(), value: "v".into() },
            },
        ],
        cells: vec![],
    };
    let text = vda_core::persist::analysis_to_json(&a).to_string_pretty();
    let back = vda_core::persist::analysis_from_json(&vda_core::json::Json::parse(&text).unwrap()).unwrap();
    assert_eq!(back, a);
}
