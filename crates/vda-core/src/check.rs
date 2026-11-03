//! Data quality checks (the Check step).
//!
//! Each rule is evaluated by composing visual steps the engines already
//! implement and the cross-engine tests already hold to the same results:
//! an `expect` rule is a calculated column `coalesce((formula), false)`
//! followed by a filter on it, a `unique` rule is a count per key followed by
//! a filter on the count. So a rule means the same in DuckDB and the
//! reference engine without a second implementation.

use crate::dataset::Dataset;
use crate::engine::Engine;
use crate::json::Json;
use crate::model::{AggFunc, CmpOp, Derived, Measure, Predicate, Rule, Step, StepKind};
use crate::table::{Table, Value};
use crate::Error;
use std::path::PathBuf;

/// Rows shown as examples of a broken rule.
pub const EXAMPLES: u64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct RuleResult {
    /// The rule's position in the step.
    pub rule: usize,
    pub passed: bool,
    /// Rows (or repeated keys) that break the rule.
    pub failing: u64,
    pub message: String,
    pub columns: Vec<String>,
    /// A few rows that break it, as displayed.
    pub examples: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CheckReport {
    pub rows: u64,
    pub results: Vec<RuleResult>,
}

impl CheckReport {
    pub fn failed(&self) -> impl Iterator<Item = &RuleResult> {
        self.results.iter().filter(|r| !r.passed)
    }

    /// For the console's Output tab and for failure messages.
    pub fn text(&self) -> String {
        let mut out = Vec::new();
        for r in &self.results {
            out.push(format!("{} {}", if r.passed { "passed:" } else { "FAILED:" }, r.message));
            if !r.examples.is_empty() {
                out.push(format!("    {}", r.columns.join(" | ")));
                for e in &r.examples {
                    out.push(format!("    {}", e.join(" | ")));
                }
            }
        }
        out.join("\n")
    }

    /// One line per broken rule, for a step that stops.
    pub fn failure_summary(&self) -> String {
        let broken: Vec<&str> = self.failed().map(|r| r.message.as_str()).collect();
        format!("{} of {} checks failed: {}", broken.len(), self.results.len(), broken.join("; "))
    }

    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("rows", Json::Num(self.rows as f64)),
            (
                "results",
                Json::Arr(
                    self.results
                        .iter()
                        .map(|r| {
                            let strs = |v: &[String]| Json::Arr(v.iter().map(|s| Json::str(s.clone())).collect());
                            Json::obj(vec![
                                ("rule", Json::Num(r.rule as f64)),
                                ("passed", Json::Bool(r.passed)),
                                ("failing", Json::Num(r.failing as f64)),
                                ("message", Json::str(r.message.clone())),
                                ("columns", strs(&r.columns)),
                                ("examples", Json::Arr(r.examples.iter().map(|e| strs(e)).collect())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }

    pub fn from_json(j: &Json) -> Result<CheckReport, Error> {
        let strs = |j: Option<&Json>| -> Vec<String> {
            j.and_then(Json::as_arr).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        Ok(CheckReport {
            rows: j.get("rows").and_then(Json::as_f64).unwrap_or(0.0) as u64,
            results: j
                .req_arr("results")?
                .iter()
                .map(|r| {
                    Ok(RuleResult {
                        rule: r.get("rule").and_then(Json::as_f64).unwrap_or(0.0) as usize,
                        passed: r.get("passed") == Some(&Json::Bool(true)),
                        failing: r.get("failing").and_then(Json::as_f64).unwrap_or(0.0) as u64,
                        message: r.req_str("message")?.to_string(),
                        columns: strs(r.get("columns")),
                        examples: r
                            .get("examples")
                            .and_then(Json::as_arr)
                            .map(|a| a.iter().map(|e| strs(Some(e))).collect())
                            .unwrap_or_default(),
                    })
                })
                .collect::<Result<_, Error>>()?,
        })
    }
}

fn shown(v: &Value) -> String {
    if v.is_null() { "missing".into() } else { v.to_string() }
}

fn rows_of(t: &Table, drop_last: bool) -> (Vec<String>, Vec<Vec<String>>) {
    let n = t.schema.fields.len() - usize::from(drop_last);
    let cols = t.schema.fields[..n].iter().map(|f| f.name.clone()).collect();
    let rows = t.rows.iter().map(|r| r[..n].iter().map(shown).collect()).collect();
    (cols, rows)
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A short description of a rule, as the editor and messages name it.
pub fn describe(rule: &Rule) -> String {
    match rule {
        Rule::Expect { formula } => format!("`{formula}`"),
        Rule::Unique { columns } => format!("one row per {}", columns.join(", ")),
        Rule::RowCount { min: Some(a), max: Some(b) } => format!("{a} to {b} rows"),
        Rule::RowCount { min: Some(a), max: None } => format!("at least {a} rows"),
        Rule::RowCount { min: None, max: Some(b) } => format!("at most {b} rows"),
        Rule::RowCount { min: None, max: None } => "any number of rows".into(),
    }
}

/// Evaluate a check step's rules on its input. `scratch` gives a fresh path
/// for an intermediate result; those files are removed before returning.
/// `original` is the rule as the user wrote it (before parameters were
/// filled in), for messages.
pub fn evaluate(
    engine: &mut dyn Engine,
    step: &Step,
    original: &Step,
    input: &Dataset,
    scratch: &mut dyn FnMut() -> PathBuf,
) -> Result<CheckReport, Error> {
    let StepKind::Check { rules, .. } = &step.kind else { unreachable!("evaluate is only called for checks") };
    let StepKind::Check { rules: written, .. } = &original.kind else { unreachable!() };
    let mut made: Vec<PathBuf> = Vec::new();
    let mut run = |engine: &mut dyn Engine, kind: StepKind, from: &Dataset, made: &mut Vec<PathBuf>| -> Result<Dataset, Error> {
        let s = Step { id: step.id.clone(), inputs: vec![step.input().to_string()], label: None, kind };
        let out = scratch();
        made.push(out.clone());
        engine.run_step(&s, std::slice::from_ref(from), &out)
    };
    let result = (|| -> Result<CheckReport, Error> {
        let mut results = Vec::new();
        for (i, (rule, as_written)) in rules.iter().zip(written).enumerate() {
            let what = describe(as_written);
            let r = match rule {
                Rule::Expect { formula } => {
                    let ok = run(
                        engine,
                        StepKind::Derive {
                            columns: vec![Derived { name: "__vda_ok".into(), formula: format!("coalesce(({formula}), false)") }],
                        },
                        input,
                        &mut made,
                    )?;
                    let bad = run(
                        engine,
                        StepKind::Filter {
                            predicates: vec![Predicate { column: "__vda_ok".into(), op: CmpOp::Eq, value: Value::Bool(false), param: None }],
                        },
                        &ok,
                        &mut made,
                    )?;
                    let (columns, examples) = rows_of(&engine.preview(&bad, 0, EXAMPLES)?, true);
                    RuleResult {
                        rule: i,
                        passed: bad.rows == 0,
                        failing: bad.rows,
                        message: if bad.rows == 0 {
                            format!("{what} holds for every row")
                        } else {
                            format!("{what} is not true for {} of {}", bad.rows, plural(input.rows, "row", "rows"))
                        },
                        columns,
                        examples,
                    }
                }
                Rule::Unique { columns: keys } => {
                    let counts = run(
                        engine,
                        StepKind::Aggregate {
                            group_by: keys.clone(),
                            measures: vec![Measure { func: AggFunc::Count, column: None, alias: "__vda_n".into() }],
                        },
                        input,
                        &mut made,
                    )?;
                    let repeated = run(
                        engine,
                        StepKind::Filter {
                            predicates: vec![Predicate { column: "__vda_n".into(), op: CmpOp::Gt, value: Value::Int(1), param: None }],
                        },
                        &counts,
                        &mut made,
                    )?;
                    let t = engine.preview(&repeated, 0, EXAMPLES)?;
                    let (mut columns, examples) = rows_of(&t, false);
                    *columns.last_mut().unwrap() = "rows".into();
                    RuleResult {
                        rule: i,
                        passed: repeated.rows == 0,
                        failing: repeated.rows,
                        message: if repeated.rows == 0 {
                            format!("{what}: no repeats")
                        } else {
                            format!(
                                "{what}: {} more than once",
                                plural(repeated.rows, &format!("value of {} appears", keys.join(", ")), &format!("values of {} appear", keys.join(", ")))
                            )
                        },
                        columns,
                        examples,
                    }
                }
                Rule::RowCount { min, max } => {
                    let n = input.rows;
                    let passed = min.is_none_or(|m| n >= m) && max.is_none_or(|m| n <= m);
                    RuleResult {
                        rule: i,
                        passed,
                        failing: u64::from(!passed),
                        message: format!("{} ({what} expected)", plural(n, "row", "rows")),
                        columns: vec![],
                        examples: vec![],
                    }
                }
            };
            results.push(r);
        }
        Ok(CheckReport { rows: input.rows, results })
    })();
    for p in made {
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("parquet.meta.json"));
    }
    result
}
