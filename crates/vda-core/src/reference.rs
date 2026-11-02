//! Reference evaluator for visual operations.
//!
//! This is the correctness oracle: it implements the intended semantics of
//! each visual operation directly, so DuckDB's execution of the compiled SQL
//! can be compared against it (cross-mode equivalence tests).
//!
//! Semantics follow SQL/DuckDB:
//! - A comparison involving NULL is not true, so the row is filtered out.
//! - COUNT(*) counts rows; COUNT(col) skips NULLs.
//! - SUM/AVG/MIN/MAX skip NULLs and return NULL when nothing is left.
//! - NULL group keys form their own group, sorted last.
//! - SUM over integers stays integer (overflow is an error, not a wrap).

use crate::compile::output_schema;
use crate::model::{AggFunc, CmpOp, Measure, Predicate, Step, StepKind};
use crate::table::{Table, Value};
use crate::Error;
use std::cmp::Ordering;
use std::collections::HashMap;

pub fn eval_step(step: &Step, input: &Table) -> Result<Table, Error> {
    let schema = output_schema(step, &input.schema)?
        .ok_or_else(|| Error::Unsupported(format!("step `{}` is not a visual table step", step.id)))?;
    let rows = match &step.kind {
        StepKind::Filter { predicates } => filter(input, predicates),
        StepKind::Aggregate { group_by, measures } => aggregate(input, group_by, measures)?,
        _ => unreachable!("output_schema returned Some for a non-table step"),
    };
    Ok(Table { schema, rows })
}

fn holds(p: &Predicate, v: &Value) -> bool {
    match p.op {
        CmpOp::IsNull => v.is_null(),
        CmpOp::IsNotNull => !v.is_null(),
        op => match v.sql_cmp(&p.value) {
            None => false,
            Some(ord) => match op {
                CmpOp::Eq => ord == Ordering::Equal,
                CmpOp::Ne => ord != Ordering::Equal,
                CmpOp::Lt => ord == Ordering::Less,
                CmpOp::Le => ord != Ordering::Greater,
                CmpOp::Gt => ord == Ordering::Greater,
                CmpOp::Ge => ord != Ordering::Less,
                CmpOp::IsNull | CmpOp::IsNotNull => unreachable!(),
            },
        },
    }
}

fn filter(input: &Table, predicates: &[Predicate]) -> Vec<Vec<Value>> {
    let idx: Vec<usize> = predicates.iter().map(|p| input.schema.index_of(&p.column).unwrap()).collect();
    input
        .rows
        .iter()
        .filter(|row| predicates.iter().zip(&idx).all(|(p, &i)| holds(p, &row[i])))
        .cloned()
        .collect()
}

fn aggregate(input: &Table, group_by: &[String], measures: &[Measure]) -> Result<Vec<Vec<Value>>, Error> {
    let gidx: Vec<usize> = group_by.iter().map(|g| input.schema.index_of(g).unwrap()).collect();
    let mut order: Vec<Vec<Value>> = Vec::new();
    let mut groups: HashMap<String, Vec<&Vec<Value>>> = HashMap::new();
    for row in &input.rows {
        let key_vals: Vec<Value> = gidx.iter().map(|&i| row[i].clone()).collect();
        let key: String = key_vals.iter().map(Value::key).collect::<Vec<_>>().join("\u{1f}");
        groups.entry(key).or_insert_with(|| {
            order.push(key_vals);
            Vec::new()
        }).push(row);
    }
    // A global aggregate over zero rows still yields one row.
    if group_by.is_empty() && order.is_empty() {
        order.push(Vec::new());
        groups.insert(String::new(), Vec::new());
    }
    order.sort_by(|a, b| {
        a.iter().zip(b).map(|(x, y)| x.sort_cmp(y)).find(|o| *o != Ordering::Equal).unwrap_or(Ordering::Equal)
    });
    let mut out = Vec::new();
    for key_vals in order {
        let key: String = key_vals.iter().map(Value::key).collect::<Vec<_>>().join("\u{1f}");
        let rows = &groups[&key];
        let mut row = key_vals;
        for m in measures {
            row.push(measure(m, rows, input)?);
        }
        out.push(row);
    }
    Ok(out)
}

fn measure(m: &Measure, rows: &[&Vec<Value>], input: &Table) -> Result<Value, Error> {
    let col = match &m.column {
        None => return Ok(Value::Int(rows.len() as i64)),
        Some(c) => input.schema.index_of(c).unwrap(),
    };
    let vals: Vec<&Value> = rows.iter().map(|r| &r[col]).filter(|v| !v.is_null()).collect();
    if m.func == AggFunc::Count {
        return Ok(Value::Int(vals.len() as i64));
    }
    if vals.is_empty() {
        return Ok(Value::Null);
    }
    Ok(match m.func {
        AggFunc::Count => unreachable!(),
        AggFunc::Sum => {
            if vals.iter().all(|v| matches!(v, Value::Int(_))) {
                let mut acc: i64 = 0;
                for v in &vals {
                    if let Value::Int(i) = v {
                        acc = acc.checked_add(*i).ok_or_else(|| {
                            Error::Data(format!("SUM overflow in `{}`", m.alias))
                        })?;
                    }
                }
                Value::Int(acc)
            } else {
                Value::Float(vals.iter().filter_map(|v| v.as_f64()).sum())
            }
        }
        AggFunc::Avg => {
            Value::Float(vals.iter().filter_map(|v| v.as_f64()).sum::<f64>() / vals.len() as f64)
        }
        AggFunc::Min => (*vals.iter().min_by(|a, b| a.sort_cmp(b)).unwrap()).clone(),
        AggFunc::Max => (*vals.iter().max_by(|a, b| a.sort_cmp(b)).unwrap()).clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv::read_table;
    use crate::model::SOURCE_ID;

    fn data() -> Table {
        read_table("region,amount,units\nEast,10.5,1\nWest,4,2\nEast,,3\n,7,\nWest,1.5,5\n").unwrap()
    }

    fn step(kind: StepKind) -> Step {
        Step { id: "s".into(), input: SOURCE_ID.into(), label: None, kind }
    }

    #[test]
    fn filter_excludes_nulls_from_comparisons() {
        let s = step(StepKind::Filter {
            predicates: vec![Predicate { column: "amount".into(), op: CmpOp::Gt, value: Value::Int(5) }],
        });
        let out = eval_step(&s, &data()).unwrap();
        assert_eq!(out.rows.len(), 2); // 10.5 and 7; NULL amount excluded
        let ne = step(StepKind::Filter {
            predicates: vec![Predicate { column: "region".into(), op: CmpOp::Ne, value: Value::Text("East".into()) }],
        });
        assert_eq!(eval_step(&ne, &data()).unwrap().rows.len(), 2); // NULL region excluded
    }

    #[test]
    fn aggregate_semantics() {
        let s = step(StepKind::Aggregate {
            group_by: vec!["region".into()],
            measures: vec![
                Measure { func: AggFunc::Count, column: None, alias: "n".into() },
                Measure { func: AggFunc::Count, column: Some("amount".into()), alias: "n_amount".into() },
                Measure { func: AggFunc::Sum, column: Some("units".into()), alias: "units".into() },
                Measure { func: AggFunc::Avg, column: Some("amount".into()), alias: "avg_amount".into() },
            ],
        });
        let out = eval_step(&s, &data()).unwrap();
        assert_eq!(
            out.rows,
            vec![
                vec![Value::Text("East".into()), Value::Int(2), Value::Int(1), Value::Int(4), Value::Float(10.5)],
                vec![Value::Text("West".into()), Value::Int(2), Value::Int(2), Value::Int(7), Value::Float(2.75)],
                vec![Value::Null, Value::Int(1), Value::Int(1), Value::Null, Value::Float(7.0)],
            ]
        );
    }

    #[test]
    fn global_aggregate_over_empty_input() {
        let empty = Table { schema: data().schema, rows: vec![] };
        let s = step(StepKind::Aggregate {
            group_by: vec![],
            measures: vec![
                Measure { func: AggFunc::Count, column: None, alias: "n".into() },
                Measure { func: AggFunc::Sum, column: Some("amount".into()), alias: "total".into() },
            ],
        });
        assert_eq!(eval_step(&s, &empty).unwrap().rows, vec![vec![Value::Int(0), Value::Null]]);
    }
}
