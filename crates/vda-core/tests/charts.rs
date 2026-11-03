//! Charts: summaries, histograms and box plots are computed by the engine
//! over every row, and the specification is the same from both engines.

use std::fs;
use std::path::{Path, PathBuf};
use vda_core::duck::DuckDbEngine;
use vda_core::engine::{Engine, ReferenceEngine};
use vda_core::json::Json;
use vda_core::model::*;
use vda_core::session::{Output, RunStatus, Session};

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-charts-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/regional-revenue");
    fs::copy(samples.join("sales.csv"), dir.join("sales.csv")).unwrap();
    dir
}

fn chart(mark: Mark, x: &str, y: &str, color: Option<&str>, summary: Option<AggFunc>) -> StepKind {
    StepKind::Chart { mark, x: x.into(), y: y.into(), title: None, color: color.map(str::to_string), summary }
}

fn cases() -> Vec<(&'static str, StepKind)> {
    vec![
        ("sum", chart(Mark::Bar, "region", "revenue", None, Some(AggFunc::Sum))),
        ("stacked", chart(Mark::Bar, "region", "revenue", Some("product"), Some(AggFunc::Sum))),
        ("count", chart(Mark::Bar, "product", "", None, Some(AggFunc::Count))),
        ("average over time", chart(Mark::Line, "order_date", "units", Some("region"), Some(AggFunc::Avg))),
        ("area", chart(Mark::Area, "order_date", "revenue", None, Some(AggFunc::Median))),
        ("points", chart(Mark::Point, "units", "revenue", Some("product"), None)),
        ("histogram", chart(Mark::Histogram, "revenue", "", None, None)),
        ("integer histogram", chart(Mark::Histogram, "units", "", None, None)),
        ("box per region", chart(Mark::Boxplot, "region", "revenue", None, None)),
        ("one box", chart(Mark::Boxplot, "", "unit_price", None, None)),
    ]
}

fn specs(dir: &Path, engine: Box<dyn Engine>) -> Vec<Json> {
    let steps: Vec<Step> = cases()
        .into_iter()
        .enumerate()
        .map(|(i, (_, kind))| Step { id: format!("c{i}"), inputs: vec![SOURCE_ID.into()], label: None, kind })
        .collect();
    let a = Analysis { name: "charts".into(), sources: vec![Source::primary("sales.csv")], steps, cells: vec![], parameters: vec![] };
    let mut s = Session::new(a, dir, engine, None).unwrap();
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:#?}");
    (0..cases().len())
        .map(|i| match s.output(&format!("c{i}")) {
            Some(Output::Chart(spec)) => spec.clone(),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn values(spec: &Json) -> &Vec<Json> {
    spec.get("data").and_then(|d| d.get("values")).and_then(Json::as_arr).unwrap()
}

fn num(v: &Json, k: &str) -> f64 {
    v.get(k).and_then(Json::as_f64).unwrap_or_else(|| panic!("no {k} in {}", v.to_string_compact()))
}

#[test]
fn both_engines_draw_the_same_charts() {
    let duck = specs(&project("duck"), Box::new(DuckDbEngine::new().unwrap()));
    let reference = specs(&project("ref"), Box::new(ReferenceEngine::new()));
    for ((case, _), (d, r)) in cases().iter().zip(duck.iter().zip(&reference)) {
        let (d, r) = (d.to_string_compact(), r.to_string_compact());
        // Medians and averages are decimals; compare them at full precision
        // only where both engines compute the same way.
        assert_eq!(d, r, "{case}");
    }

    let sum = &duck[0];
    assert_eq!(sum.get("title").and_then(Json::as_str), Some("Sum of revenue by region"));
    let total: f64 = values(sum).iter().map(|v| v.get("sum of revenue").and_then(Json::as_f64).unwrap_or(0.0)).sum();
    assert!(total > 0.0);
    let stacked = &duck[1];
    assert_eq!(stacked.get("encoding").and_then(|e| e.get("color")).and_then(|c| c.get("field")).and_then(Json::as_str), Some("product"));
    let count = &duck[2];
    let rows: f64 = values(count).iter().map(|v| num(v, "rows")).sum();
    assert_eq!(rows, 60.0);

    // Histogram bars are round and together hold every row with a value.
    let hist = values(&duck[6]);
    let w = num(&hist[0], "end") - num(&hist[0], "start");
    let mag = 10f64.powf(w.log10().floor());
    assert!([1.0, 2.0, 5.0].iter().any(|m| (w - m * mag).abs() < 1e-9), "a round width: {w}");
    assert!(hist.iter().all(|b| num(b, "start") % w == 0.0));
    assert_eq!(hist.iter().map(|b| num(b, "rows")).sum::<f64>(), 58.0, "two rows have no revenue");
    assert_eq!(values(&duck[7]).iter().map(|b| num(b, "rows")).sum::<f64>(), 58.0);

    // Box plots: quartiles in order, one box per region.
    let boxes = values(&duck[8]);
    assert_eq!(boxes.len(), 5, "four regions and the missing one");
    for b in boxes {
        let q: Vec<f64> = ["min", "q1", "median", "q3", "max"].iter().map(|k| num(b, k)).collect();
        assert!(q.windows(2).all(|p| p[0] <= p[1]), "{q:?}");
    }
    assert_eq!(values(&duck[9]).len(), 1);
}

#[test]
fn summaries_use_every_row_not_the_first_five_thousand() {
    let dir = project("large");
    let mut csv = String::from("k,n\n");
    for i in 0..12_000 {
        csv.push_str(&format!("{},{}\n", ["a", "b", "c"][i % 3], i));
    }
    fs::write(dir.join("big.csv"), csv).unwrap();
    let a = Analysis {
        name: "big".into(),
        sources: vec![Source::primary("big.csv")],
        steps: vec![
            Step { id: "sum".into(), inputs: vec![SOURCE_ID.into()], label: None, kind: chart(Mark::Bar, "k", "n", None, Some(AggFunc::Sum)) },
            Step { id: "raw".into(), inputs: vec![SOURCE_ID.into()], label: None, kind: chart(Mark::Point, "k", "n", None, None) },
        ],
        cells: vec![],
        parameters: vec![],
    };
    let mut s = Session::new(a, &dir, Box::new(DuckDbEngine::new().unwrap()), None).unwrap();
    assert!(s.run_all().unwrap().succeeded());
    let Some(Output::Chart(spec)) = s.output("sum") else { panic!() };
    let total: f64 = values(spec).iter().map(|v| num(v, "sum of n")).sum();
    assert_eq!(total, (0..12_000u64).sum::<u64>() as f64);
    assert_eq!(spec.get("title").and_then(Json::as_str), Some("Sum of n by k"));
    let Some(Output::Chart(raw)) = s.output("raw") else { panic!() };
    assert_eq!(values(raw).len(), 5_000);
    assert_eq!(raw.get("title").and_then(Json::as_str), Some("n by k (first 5000 of 12000 rows)"));
}

#[test]
fn charts_explain_what_does_not_fit() {
    let dir = project("bad");
    let bad = [
        (chart(Mark::Histogram, "region", "", None, None), "a histogram needs a numeric column; `region` is text"),
        (chart(Mark::Bar, "region", "region", None, Some(AggFunc::Sum)), "sum needs a numeric column"),
        (chart(Mark::Bar, "region", "revenue", Some("region"), None), "`region` is already along the bottom"),
        (chart(Mark::Boxplot, "region", "product", None, None), "a box plot needs a numeric column; `product` is text"),
        (chart(Mark::Line, "", "revenue", None, None), "choose the column along the bottom"),
    ];
    let steps: Vec<Step> = bad
        .iter()
        .enumerate()
        .map(|(i, (kind, _))| Step { id: format!("b{i}"), inputs: vec![SOURCE_ID.into()], label: None, kind: kind.clone() })
        .collect();
    let a = Analysis { name: "bad".into(), sources: vec![Source::primary("sales.csv")], steps, cells: vec![], parameters: vec![] };
    let mut s = Session::new(a, &dir, Box::new(DuckDbEngine::new().unwrap()), None).unwrap();
    let run = s.run_all().unwrap();
    for (i, (_, want)) in bad.iter().enumerate() {
        match &run.step(&format!("b{i}")).unwrap().status {
            RunStatus::Failed { error } => assert!(error.to_lowercase().contains(&want.to_lowercase()), "{error}"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn column_details_agree_across_engines() {
    let mut out = Vec::new();
    for (name, engine) in [("duck", Box::new(DuckDbEngine::new().unwrap()) as Box<dyn Engine>), ("ref", Box::new(ReferenceEngine::new()))] {
        let dir = project(&format!("details-{name}"));
        fs::write(dir.join("odd.csv"), "n,x,t,d\n1,0.5,a,2026-01-01\n,-2,  ,2026-01-03\n3,0,a,\n-4,0,bb,2026-01-03\n5,,a,2026-01-02\n").unwrap();
        let a = Analysis { name: "d".into(), sources: vec![Source::primary("sales.csv"), Source::new("odd", "odd.csv")], steps: vec![], cells: vec![], parameters: vec![] };
        let mut s = Session::new(a, &dir, engine, None).unwrap();
        let t0 = std::time::Instant::now();
        out.push((s.column_details(SOURCE_ID).unwrap(), s.column_details("odd").unwrap()));
        eprintln!("{name}: details in {:?}", t0.elapsed());
    }
    // Floating-point sums can differ in the last digit between engines.
    let approx = |ds: &[vda_core::profile::ColumnDetail]| -> Vec<vda_core::profile::ColumnDetail> {
        let r = |x: Option<f64>| x.map(|v| format!("{v:.9e}").parse::<f64>().unwrap());
        ds.iter()
            .map(|d| vda_core::profile::ColumnDetail { mean: r(d.mean), stddev: r(d.stddev), q1: r(d.q1), median: r(d.median), q3: r(d.q3), ..d.clone() })
            .collect()
    };
    assert_eq!(approx(&out[0].0 .1), approx(&out[1].0 .1), "sales details agree");
    assert_eq!(approx(&out[0].1 .1), approx(&out[1].1 .1), "edge-case details agree");
    let odd = &out[0].1 .1;
    let n = &odd[0];
    assert_eq!((n.zeros, n.negatives, n.median), (Some(0), Some(1), Some(2.0)));
    assert_eq!(n.histogram.iter().sum::<u64>(), 4);
    assert_eq!((n.histogram[0], n.histogram[9]), (1, 1));
    let x = &odd[1];
    assert_eq!((x.zeros, x.negatives), (Some(2), Some(1)));
    let t = &odd[2];
    assert_eq!((t.blank, t.min_length, t.max_length), (Some(1), Some(1), Some(2)));
    assert_eq!(t.top[0], (vda_core::table::Value::Text("a".into()), 3));
    let sales = &out[0].0 .1;
    let region = sales.iter().find(|d| d.name == "region").unwrap();
    assert_eq!(region.top.len(), 4);
    assert!(region.top.windows(2).all(|w| w[0].1 >= w[1].1));
}
