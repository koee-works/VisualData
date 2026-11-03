//! Column details beyond the basic profile: averages, spread, quartiles,
//! zeros and negatives, text lengths and blanks, the most common values,
//! and a ten-bar distribution.
//!
//! Like checks and chart summaries, they are computed by running visual
//! steps (calculate, aggregate, filter, sort) through the engine, so DuckDB
//! and the reference engine give the same figures by construction.

use crate::dataset::Dataset;
use crate::engine::Engine;
use crate::model::{AggFunc, CmpOp, Derived, Measure, Predicate, SortKey, Step, StepKind, SOURCE_ID};
use crate::table::{ColumnProfile, DataType, Value};
use crate::Error;
use std::path::PathBuf;

/// Most common values listed per column.
pub const TOP_VALUES: u64 = 5;
/// Bars in a column's distribution.
pub const BINS: i64 = 10;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ColumnDetail {
    pub name: String,
    pub mean: Option<f64>,
    pub stddev: Option<f64>,
    pub q1: Option<f64>,
    pub median: Option<f64>,
    pub q3: Option<f64>,
    pub zeros: Option<u64>,
    pub negatives: Option<u64>,
    /// Text that is empty or only spaces.
    pub blank: Option<u64>,
    pub min_length: Option<i64>,
    pub max_length: Option<i64>,
    /// Most common values and how often they occur (missing values excluded).
    pub top: Vec<(Value, u64)>,
    /// Counts in [`BINS`] equal-width bars from the column's min to max.
    pub histogram: Vec<u64>,
}

fn quotable(name: &str) -> bool {
    !name.contains(']') && !name.contains('[')
}

/// Details for every column of `ds`, given its basic profile.
pub fn details(
    engine: &mut dyn Engine,
    ds: &Dataset,
    profile: &[ColumnProfile],
    scratch: &mut dyn FnMut() -> PathBuf,
) -> Result<Vec<ColumnDetail>, Error> {
    let mut made: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<Vec<ColumnDetail>, Error> {
        let mut run = |engine: &mut dyn Engine, kind: StepKind, from: &Dataset| -> Result<Dataset, Error> {
            let s = Step { id: "profile".into(), inputs: vec![SOURCE_ID.into()], label: None, kind };
            let out = scratch();
            made.push(out.clone());
            engine.run_step(&s, std::slice::from_ref(from), &out)
        };
        let m = |func: AggFunc, col: &str, alias: String| Measure { func, column: Some(col.to_string()), alias };
        let mut out: Vec<ColumnDetail> = profile.iter().map(|p| ColumnDetail { name: p.name.clone(), ..Default::default() }).collect();
        if ds.rows == 0 {
            return Ok(out);
        }

        // One pass for the per-column figures: helper columns, then one summary.
        let mut derived = Vec::new();
        let mut measures = Vec::new();
        for (i, p) in profile.iter().enumerate() {
            if !quotable(&p.name) {
                continue;
            }
            let c = format!("[{}]", p.name);
            let h = |what: &str| format!("__vda_{what}_{i}");
            if p.dtype.is_numeric() {
                derived.push(Derived { name: h("zero"), formula: format!("if({c} = 0, 1, 0)") });
                derived.push(Derived { name: h("neg"), formula: format!("if({c} < 0, 1, 0)") });
                measures.extend([
                    m(AggFunc::Avg, &p.name, h("mean")),
                    m(AggFunc::Stddev, &p.name, h("sd")),
                    m(AggFunc::Percentile(25), &p.name, h("q1")),
                    m(AggFunc::Median, &p.name, h("median")),
                    m(AggFunc::Percentile(75), &p.name, h("q3")),
                    m(AggFunc::Sum, &h("zero"), h("zeros")),
                    m(AggFunc::Sum, &h("neg"), h("negs")),
                ]);
            } else if p.dtype == DataType::Text {
                derived.push(Derived { name: h("len"), formula: format!("length({c})") });
                derived.push(Derived { name: h("blank"), formula: format!("if(trim({c}) = \"\", 1, 0)") });
                measures.extend([
                    m(AggFunc::Min, &h("len"), h("minlen")),
                    m(AggFunc::Max, &h("len"), h("maxlen")),
                    m(AggFunc::Sum, &h("blank"), h("blanks")),
                ]);
            }
        }
        if !measures.is_empty() {
            let helped = run(engine, StepKind::Derive { columns: derived }, ds)?;
            let summary = run(engine, StepKind::Aggregate { group_by: vec![], measures: measures.clone() }, &helped)?;
            let t = engine.preview(&summary, 0, 1)?;
            let get = |alias: String| -> Value {
                t.schema.index_of(&alias).map(|j| t.rows[0][j].clone()).unwrap_or(Value::Null)
            };
            for (i, p) in profile.iter().enumerate() {
                let d = &mut out[i];
                let h = |what: &str| format!("__vda_{what}_{i}");
                let count = |v: Value| -> Option<u64> { Some(if let Value::Int(n) = v { n as u64 } else { 0 }) };
                let int = |v: Value| if let Value::Int(n) = v { Some(n) } else { None };
                if p.dtype.is_numeric() && quotable(&p.name) {
                    d.mean = get(h("mean")).as_f64();
                    d.stddev = get(h("sd")).as_f64();
                    d.q1 = get(h("q1")).as_f64();
                    d.median = get(h("median")).as_f64();
                    d.q3 = get(h("q3")).as_f64();
                    d.zeros = count(get(h("zeros")));
                    d.negatives = count(get(h("negs")));
                } else if p.dtype == DataType::Text && quotable(&p.name) {
                    d.min_length = int(get(h("minlen")));
                    d.max_length = int(get(h("maxlen")));
                    d.blank = count(get(h("blanks")));
                }
            }
        }

        for (i, p) in profile.iter().enumerate() {
            if !quotable(&p.name) || p.nulls == p.rows {
                continue;
            }
            // Most common values: count per value, most frequent first, ties by value.
            let present = run(
                engine,
                StepKind::Filter { predicates: vec![Predicate { column: p.name.clone(), op: CmpOp::IsNotNull, value: Value::Null, param: None }] },
                ds,
            )?;
            let counts = run(
                engine,
                StepKind::Aggregate {
                    group_by: vec![p.name.clone()],
                    measures: vec![Measure { func: AggFunc::Count, column: None, alias: "__vda_n".into() }],
                },
                &present,
            )?;
            let top = run(
                engine,
                StepKind::Sort {
                    by: vec![SortKey { column: "__vda_n".into(), descending: true }, SortKey { column: p.name.clone(), descending: false }],
                    limit: Some(TOP_VALUES),
                },
                &counts,
            )?;
            let t = engine.preview(&top, 0, TOP_VALUES)?;
            out[i].top = t.rows.iter().map(|r| (r[0].clone(), if let Value::Int(n) = r[1] { n as u64 } else { 0 })).collect();

            // Distribution: ten equal bars between the smallest and largest value.
            if p.dtype.is_numeric() {
                let (Some(lo), Some(hi)) = (p.min.as_ref().and_then(Value::as_f64), p.max.as_ref().and_then(Value::as_f64)) else { continue };
                let c = format!("[{}]", p.name);
                let formula = if hi > lo {
                    let w = (hi - lo) / BINS as f64;
                    format!("if({c} >= {hi:?}, {}, integer(floor(({c} - {lo:?}) / {w:?})))", BINS - 1)
                } else {
                    "0".into()
                };
                let binned = run(engine, StepKind::Derive { columns: vec![Derived { name: "__vda_bar".into(), formula }] }, &present)?;
                let per = run(
                    engine,
                    StepKind::Aggregate {
                        group_by: vec!["__vda_bar".into()],
                        measures: vec![Measure { func: AggFunc::Count, column: None, alias: "__vda_n".into() }],
                    },
                    &binned,
                )?;
                let t = engine.preview(&per, 0, BINS as u64 + 1)?;
                let mut bars = vec![0u64; BINS as usize];
                for r in &t.rows {
                    if let (Value::Int(b), Value::Int(n)) = (&r[0], &r[1]) {
                        bars[(*b).clamp(0, BINS - 1) as usize] += *n as u64;
                    }
                }
                out[i].histogram = bars;
            }
        }
        Ok(out)
    })();
    for p in made {
        let _ = std::fs::remove_file(&p);
    }
    result
}

/// Profile rows with their details, for the app: numbers as numbers, values
/// as displayed text (missing as null).
pub fn to_json(profile: &[ColumnProfile], details: &[ColumnDetail]) -> crate::json::Json {
    use crate::json::Json;
    let num = |x: Option<f64>| x.filter(|v| v.is_finite()).map(Json::Num).unwrap_or(Json::Null);
    let count = |x: Option<u64>| x.map(|v| Json::Num(v as f64)).unwrap_or(Json::Null);
    let shown = |v: &Option<Value>| v.as_ref().map(|v| Json::str(v.to_string())).unwrap_or(Json::Null);
    Json::Arr(
        profile
            .iter()
            .map(|p| {
                let d = details.iter().find(|d| d.name == p.name).cloned().unwrap_or_default();
                Json::obj(vec![
                    ("name", Json::str(p.name.clone())),
                    ("type", Json::str(p.dtype.name())),
                    ("rows", Json::Num(p.rows as f64)),
                    ("missing", Json::Num(p.nulls as f64)),
                    ("distinct", Json::Num(p.distinct as f64)),
                    ("min", shown(&p.min)),
                    ("max", shown(&p.max)),
                    ("mean", num(d.mean)),
                    ("stddev", num(d.stddev)),
                    ("q1", num(d.q1)),
                    ("median", num(d.median)),
                    ("q3", num(d.q3)),
                    ("zeros", count(d.zeros)),
                    ("negatives", count(d.negatives)),
                    ("blank", count(d.blank)),
                    ("min_length", d.min_length.map(|v| Json::Num(v as f64)).unwrap_or(Json::Null)),
                    ("max_length", d.max_length.map(|v| Json::Num(v as f64)).unwrap_or(Json::Null)),
                    (
                        "top",
                        Json::Arr(
                            d.top
                                .iter()
                                .map(|(v, n)| Json::obj(vec![("value", Json::str(v.to_string())), ("count", Json::Num(*n as f64))]))
                                .collect(),
                        ),
                    ),
                    ("histogram", Json::Arr(d.histogram.iter().map(|n| Json::Num(*n as f64)).collect())),
                ])
            })
            .collect(),
    )
}
