//! Cross-mode equivalence: every visual operation must give the same result
//! when compiled to SQL and run by DuckDB as when evaluated by the reference
//! engine. Also checks that both engines import CSV identically.

use std::path::{Path, PathBuf};
use vda_core::duck::DuckDbEngine;
use vda_core::engine::{Engine, ReferenceEngine};
use vda_core::model::{AggFunc, CmpOp, JoinKey, JoinKind, Measure, Predicate, Step, StepKind, SOURCE_ID};
use vda_core::table::{Table, Value};

/// The engines exchange Parquet datasets; these tests compare small results,
/// so they adapt that interface to in-memory tables.
trait TableIo {
    fn import_table(&mut self, src: &Path) -> Result<Table, vda_core::Error>;
    fn run_tables(&mut self, step: &Step, inputs: &[&Table]) -> Result<Table, vda_core::Error>;
    fn run_table(&mut self, step: &Step, input: &Table) -> Result<Table, vda_core::Error> {
        self.run_tables(step, &[input])
    }
}

fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("vda-equiv-io-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{name}-{}.parquet", N.fetch_add(1, Ordering::SeqCst)))
}

impl<E: Engine> TableIo for E {
    fn import_table(&mut self, src: &Path) -> Result<Table, vda_core::Error> {
        let out = scratch("import");
        let ds = self.import_csv(src, &out)?;
        vda_core::duck::read_parquet(&ds.path)
    }
    fn run_tables(&mut self, step: &Step, inputs: &[&Table]) -> Result<Table, vda_core::Error> {
        let mut datasets = Vec::new();
        for t in inputs {
            let (raw, inp) = (scratch("raw"), scratch("in"));
            vda_core::duck::write_parquet(t, &raw)?;
            datasets.push(self.adopt_parquet(&raw, &inp)?);
        }
        let ds = self.run_step(step, &datasets, &scratch("out"))?;
        vda_core::duck::read_parquet(&ds.path)
    }
}

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
    let mut reference = ReferenceEngine::new();
    let mut duck = DuckDbEngine::new().unwrap();
    let r_src = reference.import_table(source).unwrap();
    let d_src = duck.import_table(source).unwrap();
    assert_same(&format!("{case} (import)"), &r_src, &d_src);
    let step = Step { id: "s".into(), inputs: vec![SOURCE_ID.into()], label: None, kind };
    let r = reference.run_table(&step, &r_src).unwrap_or_else(|e| panic!("{case}: reference: {e}"));
    let d = duck.run_table(&step, &d_src).unwrap_or_else(|e| panic!("{case}: duckdb: {e}"));
    assert_same(case, &r, &d);
}

#[test]
fn csv_import_agrees() {
    for src in [sample(), write("edge.csv", EDGE_CSV)] {
        let r = ReferenceEngine::new().import_table(&src).unwrap();
        let d = DuckDbEngine::new().unwrap().import_table(&src).unwrap();
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
        let mut reference = ReferenceEngine::new();
        let mut duck = DuckDbEngine::new().unwrap();
        let (mut r, mut d) = (reference.import_table(&src).unwrap(), duck.import_table(&src).unwrap());
        for step in &demo.steps {
            r = reference.run_table(step, &r).unwrap();
            d = duck.run_table(step, &d).unwrap();
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
        inputs: vec![SOURCE_ID.into()],
        label: None,
        kind: StepKind::Aggregate { group_by: vec!["k".into()], measures: vec![m(AggFunc::Sum, Some("n"), "total")] },
    };
    let r_src = ReferenceEngine::new().import_table(&src).unwrap();
    assert!(ReferenceEngine::new().run_table(&step, &r_src).is_err());
    let mut duck = DuckDbEngine::new().unwrap();
    let d_src = duck.import_table(&src).unwrap();
    let err = duck.run_table(&step, &d_src).unwrap_err().to_string();
    assert!(err.to_lowercase().contains("out of range") || err.to_lowercase().contains("overflow"), "{err}");
}

#[test]
fn validation_messages_match() {
    let src = write("edge.csv", EDGE_CSV);
    let step = Step {
        id: "s".into(),
        inputs: vec![SOURCE_ID.into()],
        label: None,
        kind: StepKind::Filter { predicates: vec![pred("missing", CmpOp::Eq, Value::Int(1))] },
    };
    let r = ReferenceEngine::new().run_table(&step, &ReferenceEngine::new().import_table(&src).unwrap()).unwrap_err().to_string();
    let mut duck = DuckDbEngine::new().unwrap();
    let d_src = duck.import_table(&src).unwrap();
    let d = duck.run_table(&step, &d_src).unwrap_err().to_string();
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
        inputs: vec!["by_region".into()],
        label: None,
        kind: StepKind::Sql {
            sql: "WITH r AS (SELECT region, revenue, rank() OVER (ORDER BY revenue DESC) AS pos FROM input)\nSELECT * FROM r ORDER BY pos DESC".into(),
        },
    });
    let mut duck = DuckDbEngine::new().unwrap();
    let mut t = duck.import_table(&src).unwrap();
    for step in &a.steps {
        t = duck.run_table(step, &t).unwrap();
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
    let r = ReferenceEngine::new().import_table(&src).unwrap();
    let d = DuckDbEngine::new().unwrap().import_table(&src).unwrap();
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
        inputs: vec![SOURCE_ID.into()],
        label: None,
        kind: StepKind::Aggregate { group_by: vec![], measures: vec![m(AggFunc::Sum, Some("d"), "x")] },
    };
    let mut duck = DuckDbEngine::new().unwrap();
    let t = duck.import_table(&src).unwrap();
    let err = duck.run_table(&step, &t).unwrap_err().to_string();
    assert!(err.contains("needs a numeric column") && err.contains("DATE"), "{err}");
}

// ---------------------------------------------------------------- joins

const ORDERS_CSV: &str = "\
order_id,cust,region,amount
1,10,East,5.5
2,11,West,7
3,10,East,
4,,North,2
5,12,East,1
6,99,South,3
";

/// `id` is decimal on purpose (joined with an integer column); `region`
/// clashes with the left side; customer 10 appears twice.
const CUSTOMERS_CSV: &str = "\
id,name,region,tier
10.0,Ada,East,gold
10.0,Ada (old),East,
11.0,Bo,West,silver
13.0,Cy,North,gold
,Nobody,,
";

fn join(how: JoinKind, on: &[(&str, &str)]) -> StepKind {
    StepKind::Join { how, on: on.iter().map(|(l, r)| JoinKey::equal(*l, *r)).collect() }
}

fn join_both(case: &str, left_csv: &str, right_csv: &str, kind: StepKind) -> Table {
    let (l, r) = (write(&format!("{case}-l.csv"), left_csv), write(&format!("{case}-r.csv"), right_csv));
    let mut reference = ReferenceEngine::new();
    let mut duck = DuckDbEngine::new().unwrap();
    let (rl, rr) = (reference.import_table(&l).unwrap(), reference.import_table(&r).unwrap());
    let (dl, dr) = (duck.import_table(&l).unwrap(), duck.import_table(&r).unwrap());
    let step = Step { id: "j".into(), inputs: vec!["orders".into(), "customers".into()], label: None, kind };
    let a = reference.run_tables(&step, &[&rl, &rr]).unwrap_or_else(|e| panic!("{case}: reference: {e}"));
    let b = duck.run_tables(&step, &[&dl, &dr]).unwrap_or_else(|e| panic!("{case}: duckdb: {e}"));
    assert_same(case, &a, &b);
    a
}

#[test]
fn joins_agree() {
    use JoinKind::*;
    let inner = join_both("inner", ORDERS_CSV, CUSTOMERS_CSV, join(Inner, &[("cust", "id")]));
    // Customer 10 matches twice; NULL keys never match; 12 and 99 have no customer.
    let ids: Vec<String> = inner.rows.iter().map(|r| r[0].to_string()).collect();
    assert_eq!(ids, ["1", "1", "2", "3", "3"]);
    let names: Vec<&str> = inner.schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["order_id", "cust", "region", "amount", "name", "region_customers", "tier"]);

    let left = join_both("left", ORDERS_CSV, CUSTOMERS_CSV, join(Left, &[("cust", "id")]));
    assert_eq!(left.rows.len(), 8, "every order, twice for customer 10");
    assert_eq!(left.rows[5][4], Value::Null, "order 4 has no customer");

    let full = join_both("full", ORDERS_CSV, CUSTOMERS_CSV, join(Full, &[("cust", "id")]));
    assert_eq!(full.rows.len(), 10, "8 left rows, then Cy and Nobody");
    // The key is filled from the right side, as a decimal (integer joined with decimal).
    assert_eq!(full.rows[8][1], Value::Float(13.0));
    assert_eq!(full.rows[9][1], Value::Null);

    join_both("two keys", ORDERS_CSV, CUSTOMERS_CSV, join(Inner, &[("cust", "id"), ("region", "region")]));
    let none = join_both("no matches", ORDERS_CSV, "id,name\n500,X\n", join(Inner, &[("cust", "id")]));
    assert!(none.rows.is_empty());
    let none = join_both("no matches, full", ORDERS_CSV, "id,name\n500,X\n", join(Full, &[("cust", "id")]));
    assert_eq!(none.rows.len(), 7);
}

#[test]
fn join_validation_messages_match() {
    let (l, r) = (write("v-l.csv", ORDERS_CSV), write("v-r.csv", CUSTOMERS_CSV));
    for (kind, expect) in [
        (join(JoinKind::Inner, &[("region", "id")]), "cannot join VARCHAR column `region` with DOUBLE column `id`"),
        (join(JoinKind::Inner, &[("cust", "nope")]), "column `nope` not found"),
        (join(JoinKind::Inner, &[]), "at least one pair"),
    ] {
        let step = Step { id: "j".into(), inputs: vec!["orders".into(), "customers".into()], label: None, kind };
        let mut reference = ReferenceEngine::new();
        let (rl, rr) = (reference.import_table(&l).unwrap(), reference.import_table(&r).unwrap());
        let a = reference.run_tables(&step, &[&rl, &rr]).unwrap_err().to_string();
        let mut duck = DuckDbEngine::new().unwrap();
        let (dl, dr) = (duck.import_table(&l).unwrap(), duck.import_table(&r).unwrap());
        let b = duck.run_tables(&step, &[&dl, &dr]).unwrap_err().to_string();
        assert_eq!(a, b);
        assert!(a.contains(expect), "{a}");
    }
}

/// The inspectable SQL of a join, and of steps after it, executes to the
/// same result as running the steps one by one.
#[test]
fn inspectable_sql_with_a_join_executes_to_the_same_result() {
    use vda_core::model::{Analysis, Source};
    let (l, r) = (write("p-orders.csv", ORDERS_CSV), write("p-customers.csv", CUSTOMERS_CSV));
    let a = Analysis {
        name: "join".into(),
        sources: vec![
            Source::new("orders", l.to_string_lossy()),
            Source::new("customers", r.to_string_lossy()),
        ],
        steps: vec![
            Step {
                id: "gold".into(),
                inputs: vec!["customers".into()],
                label: None,
                kind: StepKind::Filter { predicates: vec![pred("tier", CmpOp::Eq, Value::Text("gold".into()))] },
            },
            Step { id: "j".into(), inputs: vec!["orders".into(), "gold".into()], label: None, kind: join(JoinKind::Left, &[("cust", "id")]) },
            Step {
                id: "per_name".into(),
                inputs: vec!["j".into(), "customers".into()],
                label: None,
                kind: StepKind::Sql {
                    sql: "SELECT j.name, COUNT(*) AS n, (SELECT COUNT(*) FROM customers) AS all_customers \
                          FROM input AS j GROUP BY j.name ORDER BY j.name NULLS LAST"
                        .into(),
                },
            },
        ],
        cells: vec![],
    };
    let mut duck = DuckDbEngine::new().unwrap();
    let mut tables = std::collections::HashMap::new();
    tables.insert("orders".to_string(), duck.import_table(&l).unwrap());
    tables.insert("customers".to_string(), duck.import_table(&r).unwrap());
    for step in &a.steps {
        let inputs: Vec<&Table> = step.inputs.iter().map(|i| &tables[i]).collect();
        let t = duck.run_tables(step, &inputs).unwrap_or_else(|e| panic!("{}: {e}", step.id));
        tables.insert(step.id.clone(), t);
        let combined = vda_core::compile::pipeline_sql_with(&a, &step.id, &|id| tables.get(id).map(|t| t.schema.clone()))
            .unwrap();
        let direct = DuckDbEngine::new().unwrap().query(&combined).unwrap_or_else(|e| panic!("{e}\n{combined}"));
        assert_same(&format!("pipeline for {}", step.id), &tables[&step.id], &direct);
    }
    assert_eq!(tables["per_name"].rows[0][2], Value::Int(5), "SQL reads a second input by its id");
    // Without the inputs' columns a join has no SQL form yet.
    assert!(vda_core::compile::pipeline_sql(&a, "j").is_err());
}

// ------------------------------------------------------------ range joins

const SALES_CSV: &str = "\
id,product,day,amount
1,A,2026-01-05,10
2,A,2026-02-10,12
3,B,2026-01-20,7
4,A,,9
5,C,2026-03-01,4
6,B,2026-03-31,
";

/// Price periods: a product's price applies from `start` to `end` inclusive.
/// A gap (A in mid-February) and an overlap (B in March) on purpose.
const PRICES_CSV: &str = "\
product,start,end,price
A,2026-01-01,2026-01-31,2.5
A,2026-02-15,2026-12-31,3
B,2026-01-01,2026-03-31,1.25
B,2026-03-01,2026-03-31,1.5
C,,,9
";

fn cond(l: &str, op: CmpOp, r: &str) -> JoinKey {
    JoinKey { left: l.into(), op, right: r.into() }
}

#[test]
fn range_joins_agree() {
    use CmpOp::*;
    let period = |how| StepKind::Join {
        how,
        on: vec![cond("product", Eq, "product"), cond("day", Ge, "start"), cond("day", Le, "end")],
    };
    // Inner: the price in force on the sale day; B on 2026-03-31 matches both
    // overlapping periods; a missing day or period bound never matches.
    let inner = join_both("periods inner", SALES_CSV, PRICES_CSV, period(JoinKind::Inner));
    let ids: Vec<String> = inner.rows.iter().map(|r| r[0].to_string()).collect();
    assert_eq!(ids, ["1", "3", "6", "6"]);
    let names: Vec<&str> = inner.schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["id", "product", "day", "amount", "start", "end", "price"], "range bounds are kept");

    let left = join_both("periods left", SALES_CSV, PRICES_CSV, period(JoinKind::Left));
    assert_eq!(left.rows.len(), 7, "every sale; sale 6 twice");
    join_both("periods full", SALES_CSV, PRICES_CSV, period(JoinKind::Full));

    // Ranges only, no equal pair: every sale against every larger amount band.
    join_both(
        "bands",
        SALES_CSV,
        "low,high,band\n0,5,small\n5,10,medium\n10,100,large\n",
        StepKind::Join { how: JoinKind::Full, on: vec![cond("amount", Ge, "low"), cond("amount", Lt, "high")] },
    );
    // Not equal.
    join_both(
        "other products",
        SALES_CSV,
        PRICES_CSV,
        StepKind::Join { how: JoinKind::Left, on: vec![cond("product", Ne, "product")] },
    );
    // A date key equal to a timestamp key, in a full join: the key becomes a timestamp.
    let mixed = join_both(
        "date = timestamp",
        SALES_CSV,
        "at,note\n2026-01-05 00:00:00,new year\n2026-01-05 10:00:00,later\n2027-01-01 00:00:00,future\n",
        StepKind::Join { how: JoinKind::Full, on: vec![cond("day", Eq, "at")] },
    );
    assert_eq!(mixed.schema.fields[2].dtype, vda_core::table::DataType::Timestamp);
}

#[test]
fn join_conditions_must_compare() {
    let (l, r) = (write("c-l.csv", SALES_CSV), write("c-r.csv", PRICES_CSV));
    let step = Step {
        id: "j".into(),
        inputs: vec!["orders".into(), "customers".into()],
        label: None,
        kind: StepKind::Join { how: JoinKind::Inner, on: vec![cond("day", CmpOp::IsNull, "start")] },
    };
    let mut duck = DuckDbEngine::new().unwrap();
    let (dl, dr) = (duck.import_table(&l).unwrap(), duck.import_table(&r).unwrap());
    let err = duck.run_tables(&step, &[&dl, &dr]).unwrap_err().to_string();
    assert!(err.contains("not a comparison"), "{err}");
}

// ----------------------------------------------------------------- append

fn append_both(case: &str, csvs: &[&str], kind: StepKind) -> Result<Table, String> {
    let paths: Vec<PathBuf> = csvs.iter().enumerate().map(|(i, c)| write(&format!("{case}-{i}.csv"), c)).collect();
    let ids: Vec<String> = (0..csvs.len()).map(|i| format!("part_{}", i + 1)).collect();
    let step = Step { id: "a".into(), inputs: ids, label: None, kind };
    let mut reference = ReferenceEngine::new();
    let mut duck = DuckDbEngine::new().unwrap();
    let rt: Vec<Table> = paths.iter().map(|p| reference.import_table(p).unwrap()).collect();
    let dt: Vec<Table> = paths.iter().map(|p| duck.import_table(p).unwrap()).collect();
    let a = reference.run_tables(&step, &rt.iter().collect::<Vec<_>>()).map_err(|e| e.to_string());
    let b = duck.run_tables(&step, &dt.iter().collect::<Vec<_>>()).map_err(|e| e.to_string());
    match (a, b) {
        (Ok(a), Ok(b)) => {
            assert_same(case, &a, &b);
            Ok(a)
        }
        (Err(a), Err(b)) => {
            assert_eq!(a, b, "{case}: messages differ");
            Err(a)
        }
        (a, b) => panic!("{case}: reference {a:?} vs duckdb {b:?}"),
    }
}

const JAN: &str = "region,units,day\nEast,3,2026-01-02\nWest,,2026-01-03\nEast,3,2026-01-02\n";
const FEB: &str = "units,region,note\n2.5,North,late\n3,East,\n";
const MAR: &str = "region,day\nEast,2026-03-01 10:30:00\n,\n";

#[test]
fn appends_agree() {
    let plain = || StepKind::Append { distinct: false, origin: None };
    let t = append_both("three", &[JAN, FEB, MAR], plain()).unwrap();
    let names: Vec<&str> = t.schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["region", "units", "day", "note"], "first input's columns, then new ones");
    use vda_core::table::DataType::*;
    let types: Vec<_> = t.schema.fields.iter().map(|f| f.dtype).collect();
    assert_eq!(types, [Text, Float, Timestamp, Text], "integer+decimal and date+timestamp widen");
    assert_eq!(t.rows.len(), 7);
    assert_eq!(t.rows[3][3], Value::Text("late".into()));
    assert_eq!(t.rows[0][3], Value::Null, "January has no note column");

    let distinct = append_both("distinct", &[JAN, JAN, FEB], StepKind::Append { distinct: true, origin: None }).unwrap();
    assert_eq!(distinct.rows.len(), 4, "repeated rows, including ones with missing values, kept once");

    let tagged =
        append_both("origin", &[JAN, FEB], StepKind::Append { distinct: true, origin: Some("month".into()) }).unwrap();
    assert_eq!(tagged.schema.fields.last().unwrap().name, "month");
    assert_eq!(tagged.rows.len(), 4, "identical rows within one input still collapse");
    assert_eq!(tagged.rows[3][4], Value::Text("part_2".into()));

    let err = append_both("clash", &[JAN, "region,units\nEast,lots\n"], plain()).unwrap_err();
    assert!(err.contains("column `units` is BIGINT in `part_1` but VARCHAR in `part_2`"), "{err}");
    let err = append_both("origin clash", &[JAN, FEB], StepKind::Append { distinct: false, origin: Some("note".into()) })
        .unwrap_err();
    assert!(err.contains("already have a column `note`"), "{err}");
}

#[test]
fn inspectable_sql_of_an_append_executes_to_the_same_result() {
    use vda_core::model::{Analysis, Source};
    let paths = [write("pa-jan.csv", JAN), write("pa-feb.csv", FEB)];
    let a = Analysis {
        name: "append".into(),
        sources: vec![
            Source::new("jan", paths[0].to_string_lossy()),
            Source::new("feb", paths[1].to_string_lossy()),
        ],
        steps: vec![Step {
            id: "both".into(),
            inputs: vec!["jan".into(), "feb".into()],
            label: None,
            kind: StepKind::Append { distinct: true, origin: Some("month".into()) },
        }],
        cells: vec![],
    };
    let mut duck = DuckDbEngine::new().unwrap();
    let (j, f) = (duck.import_table(&paths[0]).unwrap(), duck.import_table(&paths[1]).unwrap());
    let direct = duck.run_tables(&a.steps[0], &[&j, &f]).unwrap();
    let schemas = [("jan", j.schema.clone()), ("feb", f.schema.clone())];
    let sql = vda_core::compile::pipeline_sql_with(&a, "both", &|id| {
        schemas.iter().find(|(k, _)| *k == id).map(|(_, s)| s.clone())
    })
    .unwrap();
    let combined = DuckDbEngine::new().unwrap().query(&sql).unwrap_or_else(|e| panic!("{e}\n{sql}"));
    assert_same("append pipeline", &direct, &combined);
}
