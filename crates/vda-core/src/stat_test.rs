//! The statistical test step.
//!
//! Like checks and chart summaries, the data work is done by running visual
//! steps through the engine (filter, aggregate, calculate), so DuckDB and the
//! reference engine agree by construction: counts, means and spreads per
//! group, cross-tab counts, centred sums of products. Only the last step,
//! turning those into a statistic and a p-value, is arithmetic here
//! ([`crate::stats`]), checked against SciPy in `tests/stat_tests.rs`.

use crate::dataset::Dataset;
use crate::engine::Engine;
use crate::model::{AggFunc, CmpOp, Derived, Measure, Predicate, Step, StepKind, TestKind};
use crate::stats;
use crate::table::{DataType, Field, Schema, Table, Value};
use crate::Error;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Most groups or categories a test reads.
pub const MAX_GROUPS: u64 = 1_000;
/// The level the reading sentence uses.
pub const ALPHA: f64 = 0.05;

fn err(id: &str, m: impl std::fmt::Display) -> Error {
    Error::Validation(format!("step `{id}`: {m}"))
}

fn fields(cols: &[(&str, DataType)]) -> Schema {
    Schema { fields: cols.iter().map(|(n, t)| Field { name: n.to_string(), dtype: *t }).collect() }
}

/// The result columns of each test.
pub fn schema(kind: TestKind) -> Schema {
    use DataType::*;
    match kind {
        TestKind::CompareMeans => fields(&[
            ("group_1", Text),
            ("n_1", Int),
            ("mean_1", Float),
            ("group_2", Text),
            ("n_2", Int),
            ("mean_2", Float),
            ("difference", Float),
            ("t", Float),
            ("df", Float),
            ("p_value", Float),
            ("reading", Text),
        ]),
        TestKind::CompareGroups => fields(&[
            ("groups", Int),
            ("n", Int),
            ("f", Float),
            ("df_between", Int),
            ("df_within", Int),
            ("p_value", Float),
            ("reading", Text),
        ]),
        TestKind::Independence => fields(&[
            ("n", Int),
            ("categories_1", Int),
            ("categories_2", Int),
            ("chi_square", Float),
            ("df", Int),
            ("p_value", Float),
            ("cramers_v", Float),
            ("smallest_expected", Float),
            ("reading", Text),
        ]),
        TestKind::Correlation => fields(&[("n", Int), ("r", Float), ("t", Float), ("df", Int), ("p_value", Float), ("reading", Text)]),
    }
}

/// Check a test against its input's columns.
pub fn validate(step: &Step, input: &Schema) -> Result<Schema, Error> {
    let StepKind::Test { test, a, b } = &step.kind else { unreachable!("not a test") };
    let id = &step.id;
    let col = |name: &str, what: &str| -> Result<DataType, Error> {
        if name.is_empty() {
            return Err(err(id, format!("choose {what}")));
        }
        Ok(crate::compile::require_column(input, name, id)?.dtype)
    };
    let number = |name: &str, what: &str| -> Result<(), Error> {
        let t = col(name, what)?;
        if t.is_numeric() {
            Ok(())
        } else {
            Err(err(id, format!("{what} needs numbers; `{name}` is {}", t.plain())))
        }
    };
    let category = |name: &str, what: &str| -> Result<(), Error> {
        match col(name, what)? {
            DataType::Float => Err(err(id, format!("{what} needs categories; `{name}` is decimal (group it first with bucket())"))),
            DataType::Timestamp => Err(err(id, format!("{what} needs categories; `{name}` is date and time (group it first with date_trunc())"))),
            _ => Ok(()),
        }
    };
    match test {
        TestKind::CompareMeans | TestKind::CompareGroups => {
            number(a, "the column to compare")?;
            category(b, "the column that forms the groups")?;
        }
        TestKind::Independence => {
            category(a, "the first column")?;
            category(b, "the second column")?;
        }
        TestKind::Correlation => {
            number(a, "the first column")?;
            number(b, "the second column")?;
        }
    }
    if a == b {
        return Err(err(id, "choose two different columns"));
    }
    Ok(schema(*test))
}

fn p_text(p: f64) -> String {
    if p < 0.001 {
        "p < 0.001".into()
    } else {
        format!("p = {:.3}", p)
    }
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        other => other.as_f64().unwrap_or(0.0) as i64,
    }
}

/// A column reference in a formula.
fn r(name: &str) -> String {
    format!("[{}]", name.replace(']', ""))
}

/// A number written so the formula reads it back exactly.
fn lit(x: f64) -> String {
    if x < 0.0 { format!("({x:?})") } else { format!("{x:?}") }
}

/// Run the test over `input`, returning its one-row result.
pub fn evaluate(engine: &mut dyn Engine, step: &Step, input: &Dataset, scratch: &mut dyn FnMut() -> PathBuf) -> Result<Table, Error> {
    let StepKind::Test { test, a, b } = &step.kind else { unreachable!("not a test") };
    let schema = validate(step, &input.schema)?;
    let id = step.id.as_str();
    let mut made: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<Vec<Value>, Error> {
        let mut run = |engine: &mut dyn Engine, kind: StepKind, from: &Dataset| -> Result<Dataset, Error> {
            let s = Step { id: step.id.clone(), inputs: vec![step.input().to_string()], label: None, kind };
            let out = scratch();
            made.push(out.clone());
            engine.run_step(&s, std::slice::from_ref(from), &out)
        };
        let present = |c: &str| Predicate { column: c.into(), op: CmpOp::IsNotNull, value: Value::Null, param: None };
        let m = |func: AggFunc, col: Option<&str>, alias: &str| Measure { func, column: col.map(str::to_string), alias: alias.into() };
        // Rows with either value missing take no part.
        let both = run(engine, StepKind::Filter { predicates: vec![present(a), present(b)] }, input)?;
        let read = |engine: &mut dyn Engine, ds: &Dataset| -> Result<Table, Error> {
            if ds.rows > MAX_GROUPS * MAX_GROUPS.min(100) {
                return Err(err(id, format!("there are {} combinations; a test reads at most {MAX_GROUPS} groups", ds.rows)));
            }
            engine.preview(ds, 0, ds.rows)
        };
        match test {
            TestKind::CompareMeans | TestKind::CompareGroups => {
                let groups = run(
                    engine,
                    StepKind::Aggregate {
                        group_by: vec![b.clone()],
                        measures: vec![m(AggFunc::Count, Some(a), "n"), m(AggFunc::Avg, Some(a), "mean"), m(AggFunc::Stddev, Some(a), "sd")],
                    },
                    &both,
                )?;
                if groups.rows > MAX_GROUPS {
                    return Err(err(id, format!("`{b}` has {} values; a test compares at most {MAX_GROUPS} groups", groups.rows)));
                }
                let t = read(engine, &groups)?;
                let g: Vec<(String, f64, f64, f64)> = t
                    .rows
                    .iter()
                    .map(|row| (row[0].to_string(), int(&row[1]) as f64, num(&row[2]), row[3].as_f64().unwrap_or(0.0)))
                    .collect();
                if *test == TestKind::CompareMeans {
                    if g.len() != 2 {
                        let names: Vec<&str> = g.iter().take(6).map(|x| x.0.as_str()).collect();
                        return Err(err(
                            id,
                            format!(
                                "comparing two averages needs exactly two groups, and `{b}` has {} ({}{}); filter to two, or compare several groups instead",
                                g.len(),
                                names.join(", "),
                                if g.len() > 6 { ", …" } else { "" }
                            ),
                        ));
                    }
                    let ((g1, n1, m1, s1), (g2, n2, m2, s2)) = (&g[0], &g[1]);
                    if *n1 < 2.0 || *n2 < 2.0 {
                        return Err(err(id, format!("each group needs at least two values; `{g1}` has {n1}, `{g2}` has {n2}")));
                    }
                    // Welch's t-test: the groups may have different spreads.
                    let (v1, v2) = (s1 * s1 / n1, s2 * s2 / n2);
                    let se = (v1 + v2).sqrt();
                    if se == 0.0 {
                        return Err(err(id, "every value in each group is the same, so the spread is zero and there is nothing to test"));
                    }
                    let tstat = (m1 - m2) / se;
                    let df = (v1 + v2).powi(2) / (v1 * v1 / (n1 - 1.0) + v2 * v2 / (n2 - 1.0));
                    let p = stats::t_two_sided(tstat, df);
                    let reading = if p < ALPHA {
                        format!("The average of {a} differs between {g1} and {g2} ({}).", p_text(p))
                    } else {
                        format!("No clear difference in the average of {a} between {g1} and {g2} ({}).", p_text(p))
                    };
                    Ok(vec![
                        Value::Text(g1.clone()),
                        Value::Int(*n1 as i64),
                        Value::Float(*m1),
                        Value::Text(g2.clone()),
                        Value::Int(*n2 as i64),
                        Value::Float(*m2),
                        Value::Float(m1 - m2),
                        Value::Float(tstat),
                        Value::Float(df),
                        Value::Float(p),
                        Value::Text(reading),
                    ])
                } else {
                    let k = g.len() as f64;
                    let n: f64 = g.iter().map(|x| x.1).sum();
                    if k < 2.0 || n <= k {
                        return Err(err(id, format!("comparing groups needs at least two groups and more values than groups; `{b}` has {k} groups and {n} values")));
                    }
                    let grand = g.iter().map(|x| x.1 * x.2).sum::<f64>() / n;
                    let between: f64 = g.iter().map(|x| x.1 * (x.2 - grand).powi(2)).sum();
                    let within: f64 = g.iter().map(|x| (x.1 - 1.0) * x.3 * x.3).sum();
                    if within == 0.0 {
                        return Err(err(id, "every value within each group is the same, so there is nothing to test"));
                    }
                    let (d1, d2) = (k - 1.0, n - k);
                    let f = (between / d1) / (within / d2);
                    let p = stats::f_upper(f, d1, d2);
                    let reading = if p < ALPHA {
                        format!("The average of {a} differs among the {k} groups of {b} ({}).", p_text(p))
                    } else {
                        format!("No clear difference in the average of {a} among the {k} groups of {b} ({}).", p_text(p))
                    };
                    Ok(vec![
                        Value::Int(k as i64),
                        Value::Int(n as i64),
                        Value::Float(f),
                        Value::Int(d1 as i64),
                        Value::Int(d2 as i64),
                        Value::Float(p),
                        Value::Text(reading),
                    ])
                }
            }
            TestKind::Independence => {
                let cells = run(engine, StepKind::Aggregate { group_by: vec![a.clone(), b.clone()], measures: vec![m(AggFunc::Count, None, "n")] }, &both)?;
                let t = read(engine, &cells)?;
                let mut rows: BTreeMap<String, f64> = BTreeMap::new();
                let mut cols: BTreeMap<String, f64> = BTreeMap::new();
                let mut obs: BTreeMap<(String, String), f64> = BTreeMap::new();
                for row in &t.rows {
                    let (x, y, c) = (row[0].key(), row[1].key(), int(&row[2]) as f64);
                    *rows.entry(x.clone()).or_default() += c;
                    *cols.entry(y.clone()).or_default() += c;
                    obs.insert((x, y), c);
                }
                let (r_n, c_n) = (rows.len(), cols.len());
                if r_n as u64 > MAX_GROUPS || c_n as u64 > MAX_GROUPS {
                    return Err(err(id, format!("a test reads at most {MAX_GROUPS} categories per column")));
                }
                if r_n < 2 || c_n < 2 {
                    return Err(err(id, format!("each column needs at least two categories; `{a}` has {r_n}, `{b}` has {c_n}")));
                }
                let n: f64 = rows.values().sum();
                let (mut chi2, mut smallest) = (0.0, f64::INFINITY);
                for (x, rt) in &rows {
                    for (y, ct) in &cols {
                        let e = rt * ct / n;
                        smallest = smallest.min(e);
                        let o = obs.get(&(x.clone(), y.clone())).copied().unwrap_or(0.0);
                        chi2 += (o - e).powi(2) / e;
                    }
                }
                let df = ((r_n - 1) * (c_n - 1)) as f64;
                let p = stats::chi2_upper(chi2, df);
                let v = (chi2 / (n * ((r_n.min(c_n) - 1) as f64))).sqrt();
                let mut reading = if p < ALPHA {
                    format!("{a} and {b} are related ({}; strength {:.2} on a 0–1 scale).", p_text(p), v)
                } else {
                    format!("No clear relation between {a} and {b} ({}).", p_text(p))
                };
                if smallest < 5.0 {
                    reading.push_str(" Some expected counts are below 5, so the p-value is rough.");
                }
                Ok(vec![
                    Value::Int(n as i64),
                    Value::Int(r_n as i64),
                    Value::Int(c_n as i64),
                    Value::Float(chi2),
                    Value::Int(df as i64),
                    Value::Float(p),
                    Value::Float(v),
                    Value::Float(smallest),
                    Value::Text(reading),
                ])
            }
            TestKind::Correlation => {
                let means = run(
                    engine,
                    StepKind::Aggregate { group_by: vec![], measures: vec![m(AggFunc::Count, None, "n"), m(AggFunc::Avg, Some(a), "ma"), m(AggFunc::Avg, Some(b), "mb")] },
                    &both,
                )?;
                let t = engine.preview(&means, 0, 1)?;
                let n = int(&t.rows[0][0]) as f64;
                if n < 3.0 {
                    return Err(err(id, format!("a correlation needs at least three rows with both values; there are {n}")));
                }
                let (ma, mb) = (num(&t.rows[0][1]), num(&t.rows[0][2]));
                // Centred on the means (a second pass), which keeps precision.
                let (da, db) = (format!("({} - {})", r(a), lit(ma)), format!("({} - {})", r(b), lit(mb)));
                let products = run(
                    engine,
                    StepKind::Derive {
                        columns: vec![
                            Derived { name: "__vda_ab".into(), formula: format!("{da} * {db}") },
                            Derived { name: "__vda_aa".into(), formula: format!("{da} * {da}") },
                            Derived { name: "__vda_bb".into(), formula: format!("{db} * {db}") },
                        ],
                    },
                    &both,
                )?;
                let sums = run(
                    engine,
                    StepKind::Aggregate {
                        group_by: vec![],
                        measures: vec![m(AggFunc::Sum, Some("__vda_ab"), "ab"), m(AggFunc::Sum, Some("__vda_aa"), "aa"), m(AggFunc::Sum, Some("__vda_bb"), "bb")],
                    },
                    &products,
                )?;
                let s = engine.preview(&sums, 0, 1)?;
                let (sab, saa, sbb) = (num(&s.rows[0][0]), num(&s.rows[0][1]), num(&s.rows[0][2]));
                if saa == 0.0 || sbb == 0.0 {
                    let flat = if saa == 0.0 { a } else { b };
                    return Err(err(id, format!("every value of `{flat}` is the same, so there is no correlation to measure")));
                }
                let rr = (sab / (saa * sbb).sqrt()).clamp(-1.0, 1.0);
                let df = n - 2.0;
                let (tstat, p) = if rr.abs() == 1.0 { (f64::INFINITY.copysign(rr), 0.0) } else {
                    let t = rr * (df / (1.0 - rr * rr)).sqrt();
                    (t, stats::t_two_sided(t, df))
                };
                let strength = match rr.abs() {
                    x if x >= 0.7 => "strong",
                    x if x >= 0.4 => "moderate",
                    x if x >= 0.1 => "weak",
                    _ => "very weak",
                };
                let direction = if rr >= 0.0 { "rises" } else { "falls" };
                let reading = if p < ALPHA {
                    format!("A {strength} correlation: {b} {direction} as {a} rises (r = {rr:.2}, {}).", p_text(p))
                } else {
                    format!("No clear correlation between {a} and {b} (r = {rr:.2}, {}).", p_text(p))
                };
                Ok(vec![
                    Value::Int(n as i64),
                    Value::Float(rr),
                    if tstat.is_finite() { Value::Float(tstat) } else { Value::Null },
                    Value::Int(df as i64),
                    Value::Float(p),
                    Value::Text(reading),
                ])
            }
        }
    })();
    for p in made {
        let _ = std::fs::remove_file(&p);
    }
    Ok(Table { schema, rows: vec![result?] })
}
