//! The Clean step: DuckDB and the reference engine give the same result for
//! every way of finding and filling missing values, and a session fits
//! regressions and neighbour searches the same way on both.

use std::path::{Path, PathBuf};
use vda_core::duck::DuckDbEngine;
use vda_core::engine::{Engine, ReferenceEngine};
use vda_core::model::{Analysis, CleanColumn, Fill, SortKey, Source, Step, StepKind, SOURCE_ID};
use vda_core::session::Session;
use vda_core::table::{DataType, Table, Value};

const MESSY: &str = "\
id,region,day,price,units,size,joined,score
1,North,3,4.50,10,S,2026-01-02,1.5
2,North,1,N/A,,M,2026-01-05,2.5
3,South,2,5.25,-999,L,,3.0
4,South,5,,7,,2026-02-01,
5,North,4,abc,12,S,2026-01-09,4.5
6,,6,6.00,,L,2026-03-01,5.5
7,South,7,4.75,9,M,n/a,6.0
8,North,8,5.5,11,,2026-01-20,7.5
9,East,9,n/a,3,S,2026-04-01,8.0
10,East,10,7.00,,S,2026-04-02,
11,East,11,6.5,4,M,2026-04-09,9.0
12,South,12,NA,8,L,2026-05-01,9.5
";

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vda-clean-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("messy.csv"), MESSY).unwrap();
    d
}

fn scratch(d: &Path, name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    d.join(format!("{name}-{}.parquet", N.fetch_add(1, Ordering::SeqCst)))
}

fn run_on(engine: &mut dyn Engine, d: &Path, step: &Step) -> Result<Table, vda_core::Error> {
    let src = engine.import_csv(&d.join("messy.csv"), &scratch(d, "in"))?;
    let out = engine.run_step(step, &[src], &scratch(d, "out"))?;
    vda_core::duck::read_parquet(&out.path)
}

fn close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x == y || (x - y).abs() <= 1e-12 * x.abs().max(y.abs()),
        _ => a == b,
    }
}

fn same(case: &str, r: &Table, d: &Table) {
    assert_eq!(r.schema, d.schema, "{case}: schema differs");
    assert_eq!(r.rows.len(), d.rows.len(), "{case}: row count differs\nref:  {:?}\nduck: {:?}", r.rows, d.rows);
    for (i, (a, b)) in r.rows.iter().zip(&d.rows).enumerate() {
        assert!(a.iter().zip(b).all(|(x, y)| close(x, y)), "{case}: row {i} differs\nref:  {a:?}\nduck: {b:?}");
    }
}

fn clean(markers: &[&str], columns: Vec<CleanColumn>, flag: bool, internal: bool) -> Step {
    Step {
        id: "tidy".into(),
        inputs: vec![SOURCE_ID.into()],
        label: None,
        kind: StepKind::Clean { markers: markers.iter().map(|s| s.to_string()).collect(), columns, flag, seed: 7, internal },
    }
}

fn c(column: &str, fill: Fill) -> CleanColumn {
    CleanColumn::new(column, fill)
}
fn typed(column: &str, t: DataType, fill: Fill) -> CleanColumn {
    CleanColumn { as_type: Some(t), ..c(column, fill) }
}
fn grouped(mut col: CleanColumn, g: &[&str]) -> CleanColumn {
    col.group_by = g.iter().map(|s| s.to_string()).collect();
    col
}
fn by(column: &str, descending: bool) -> SortKey {
    SortKey { column: column.into(), descending }
}

fn cases() -> Vec<(&'static str, Step)> {
    let markers = ["N/A", "NA", "", "-999"];
    let when = |mut col: CleanColumn, f: &str| {
        col.missing_when = Some(f.into());
        col
    };
    vec![
        ("text read as numbers, median", clean(&markers, vec![typed("price", DataType::Float, Fill::Median)], true, false)),
        ("whole numbers, mean per region", clean(&markers, vec![grouped(c("units", Fill::Mean), &["region"])], true, true)),
        ("median of whole numbers", clean(&markers, vec![c("units", Fill::Median)], false, false)),
        ("most common, per region", clean(&markers, vec![grouped(c("size", Fill::Mode), &["region"])], false, false)),
        ("most common overall", clean(&[], vec![c("size", Fill::Mode)], false, true)),
        ("a constant", clean(&[], vec![c("size", Fill::Constant { value: "unknown".into() })], true, false)),
        ("random draw", clean(&markers, vec![c("units", Fill::Random)], false, true)),
        ("random draw per region", clean(&markers, vec![grouped(c("units", Fill::Random), &["region"])], false, false)),
        ("previous by day", clean(&markers, vec![c("units", Fill::Previous { order_by: vec![by("day", false)] })], false, false)),
        ("next, per region, newest first", clean(&markers, vec![grouped(c("units", Fill::Next { order_by: vec![by("day", true)] }), &["region"])], false, false)),
        ("previous in row order", clean(&markers, vec![c("size", Fill::Previous { order_by: vec![] })], false, false)),
        ("dates from text, previous", clean(&["n/a"], vec![typed("joined", DataType::Date, Fill::Previous { order_by: vec![by("day", false)] })], true, false)),
        ("a condition, kept missing", clean(&[], vec![when(c("score", Fill::Keep), "[score] > 7")], true, true)),
        ("drop rows", clean(&markers, vec![c("units", Fill::DropRows)], false, false)),
        ("drop rows, internal", clean(&markers, vec![c("units", Fill::DropRows)], true, true)),
        (
            "a fitted line",
            clean(&markers, vec![c("units", Fill::Linear { intercept: 2.0, terms: vec![("score".into(), 1.5), ("day".into(), -0.25)] })], false, false),
        ),
        (
            "neighbour values",
            clean(&[], vec![c("size", Fill::Rows { values: vec![(4, Value::Text("XL".into())), (8, Value::Text("M".into()))] })], false, false),
        ),
        (
            "several columns at once",
            clean(
                &markers,
                vec![
                    typed("price", DataType::Float, Fill::Mean),
                    grouped(c("units", Fill::Median), &["region"]),
                    c("size", Fill::Random),
                    when(c("score", Fill::Constant { value: "0".into() }), "[score] > 9"),
                ],
                true,
                true,
            ),
        ),
    ]
}

#[test]
fn both_engines_find_and_fill_the_same_way() {
    let d = dir("engines");
    for (case, step) in cases() {
        let r = run_on(&mut ReferenceEngine::new(), &d, &step).unwrap_or_else(|e| panic!("{case}: reference: {e}"));
        let k = run_on(&mut DuckDbEngine::new().unwrap(), &d, &step).unwrap_or_else(|e| panic!("{case}: duckdb: {e}"));
        same(case, &r, &k);
    }
}

#[test]
fn the_results_are_what_an_analyst_expects() {
    let d = dir("expected");
    let mut duck = DuckDbEngine::new().unwrap();
    // Prices: "N/A", "", "abc", "n/a" and "NA" are missing; the rest become numbers.
    let t = run_on(&mut duck, &d, &clean(&["N/A", "NA", ""], vec![typed("price", DataType::Float, Fill::Median)], true, false)).unwrap();
    let pi = t.schema.index_of("price").unwrap();
    let known = [4.5, 5.25, 6.0, 4.75, 5.5, 7.0, 6.5];
    let mut sorted = known.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = sorted[3];
    let prices: Vec<f64> = t.rows.iter().map(|r| r[pi].as_f64().unwrap()).collect();
    assert_eq!(prices, vec![4.5, med, 5.25, med, med, 6.0, 4.75, 5.5, med, 7.0, 6.5, med]);
    let flags: Vec<bool> = t.rows.iter().map(|r| r[t.schema.index_of("price_missing").unwrap()] == Value::Bool(true)).collect();
    assert_eq!(flags.iter().filter(|f| **f).count(), 5);
    // Dropping rows with no units (-999 counts as none) keeps the others in order.
    let t = run_on(&mut duck, &d, &clean(&["-999"], vec![c("units", Fill::DropRows)], false, false)).unwrap();
    let ids: Vec<i64> = t.rows.iter().map(|r| if let Value::Int(i) = r[0] { i } else { 0 }).collect();
    assert_eq!(ids, vec![1, 4, 5, 7, 8, 9, 11, 12]);
}

fn analysis(step: Step) -> Analysis {
    Analysis { name: "Messy".into(), sources: vec![Source::primary("messy.csv")], steps: vec![step], cells: vec![], parameters: vec![] }
}

#[test]
fn a_session_fits_regressions_and_neighbours_the_same_on_both_engines() {
    let step = clean(
        &["-999", "N/A", "NA", "", "n/a"],
        vec![
            c("units", Fill::Regression { predictors: vec!["score".into(), "day".into()] }),
            c("size", Fill::Neighbours { predictors: vec!["score".into(), "day".into()], k: 3 }),
            typed("price", DataType::Float, Fill::Median),
        ],
        true,
        false,
    );
    let mut results = vec![];
    for (name, engine) in [("duck", Box::new(DuckDbEngine::new().unwrap()) as Box<dyn Engine>), ("reference", Box::new(ReferenceEngine::new()))] {
        let d = dir(&format!("session-{name}"));
        let mut s = Session::new(analysis(step.clone()), &d, engine, None).unwrap();
        let run = s.run_all().unwrap();
        let t = s.table("tidy").unwrap_or_else(|e| panic!("{name}: {e}\n{run:?}"));
        let report = s.clean_report("tidy").expect("a report");
        results.push((t, report));
    }
    let ((t, report), (r, _)) = (&results[0], &results[1]);
    same("session", r, t);
    let units = &report.columns[0];
    // Empty in rows 2, 6 and 10; -999 in row 3. Rows 4 and 10 have no score, so their units can't be predicted.
    assert_eq!((units.empty, units.markers), (3, 1), "{report:?}");
    assert_eq!(units.left, 1, "{report:?}");
    assert_eq!(units.filled, 3);
    assert!(units.detail.starts_with("a line fitted on") && units.detail.contains("units = "), "{}", units.detail);
    let size = &report.columns[1];
    assert_eq!(size.empty, 2);
    assert!(size.detail.contains("3 most similar rows by score, day"), "{}", size.detail);
    let price = &report.columns[2];
    // "N/A", "n/a" and "NA" are markers, "abc" doesn't convert; the empty cell is read as missing already.
    assert_eq!((price.markers, price.conversion, price.empty), (3, 1, 1), "{report:?}");
    assert!(price.detail.starts_with("the median, "), "{}", price.detail);
    assert!(report.text().contains("units: 4 missing (3 empty, 1 a marker); filled 3 with a line fitted"), "{}", report.text());
}

#[test]
fn too_few_rows_to_fit_is_explained() {
    let step = clean(&[], vec![c("units", Fill::Regression { predictors: vec!["score".into(), "day".into(), "id".into()] })], false, false);
    let d = dir("few");
    std::fs::write(d.join("messy.csv"), "id,day,score,units\n1,1,1.0,3\n2,2,2.0,\n3,3,3.5,5\n").unwrap();
    let mut s = Session::new(analysis(step), &d, Box::new(DuckDbEngine::new().unwrap()), None).unwrap();
    let run = s.run_all().unwrap();
    let msg = format!("{run:?}");
    assert!(msg.contains("only 2 rows have every value known"), "{msg}");
}

#[test]
fn comparing_methods_on_hidden_values_ranks_them_sensibly() {
    // y follows x closely; kind follows x's band.
    let mut csv = String::from("x,y,kind,day\n");
    for i in 0..400 {
        let x = (i * 37 % 400) as f64 / 10.0;
        let noise = ((i * 7919) % 13) as f64 / 13.0 - 0.5;
        let y = if i % 9 == 0 { String::new() } else { format!("{:.3}", 3.0 + 2.0 * x + noise) };
        let kind = if x < 13.0 { "low" } else if x < 26.0 { "mid" } else { "high" };
        csv.push_str(&format!("{x},{y},{kind},{i}\n"));
    }
    let d = dir("compare");
    std::fs::write(d.join("messy.csv"), csv).unwrap();
    let step = clean(&[], vec![c("y", Fill::Median), c("kind", Fill::Mode)], false, false);
    let mut s = Session::new(analysis(step.clone()), &d, Box::new(DuckDbEngine::new().unwrap()), None).unwrap();
    let cmp = s.compare_fills(&step, "y").unwrap();
    assert_eq!(cmp.rows, 400);
    assert_eq!(cmp.hidden, (cmp.known as f64 * 0.1).ceil() as usize);
    assert_eq!(cmp.predictors, vec!["x", "day"]);
    let by = |label: &str| cmp.methods.iter().find(|m| m.label.starts_with(label)).unwrap_or_else(|| panic!("no {label}: {cmp:?}"));
    let (reg, mean, random) = (by("Regression"), by("Mean"), by("Random"));
    assert!(reg.mae.unwrap() < 0.5, "{reg:?}");
    assert!(reg.mae.unwrap() < by("Median").mae.unwrap() && reg.mae.unwrap() < random.mae.unwrap());
    assert!(mean.spread.unwrap() < 1e-9, "one value for every gap keeps none of the spread: {mean:?}");
    assert!(random.spread.unwrap() > 0.5, "{random:?}");
    assert_eq!(reg.filled, cmp.hidden);
    // Using a method: its fill as saved.
    assert_eq!(reg.fill.to_string_compact(), r#"{"method":"regression","predictors":["x","day"]}"#);

    let cmp = s.compare_fills(&step, "kind").unwrap();
    let nn = cmp.methods.iter().find(|m| m.label.contains("nearest")).unwrap();
    assert!(nn.accuracy.unwrap() > 0.9, "{nn:?}");
    assert!(cmp.methods.iter().all(|m| m.mae.is_none()), "text has no distance");
    assert!(cmp.methods.iter().all(|m| !m.label.starts_with("Mean")));
}
