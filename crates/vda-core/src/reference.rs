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
//! - Join keys match when every pair compares equal; NULL never matches.
//!   Output keeps left rows in order, each followed by its matches in right
//!   order, then (full join) unmatched right rows in order.

use crate::compile::output_schema;
use crate::model::{AggFunc, CmpOp, JoinKey, JoinKind, Measure, Predicate, Step, StepKind};
use crate::table::{DataType, Schema, Table, Value};
use crate::Error;
use std::cmp::Ordering;
use std::collections::HashMap;

/// Evaluate a visual table step on its inputs (in input order).
pub fn eval_step(step: &Step, inputs: &[Table]) -> Result<Table, Error> {
    let schemas: Vec<Schema> = inputs.iter().map(|t| t.schema.clone()).collect();
    let schema = output_schema(step, &schemas)?
        .ok_or_else(|| Error::Unsupported(format!("step `{}` is not a visual table step", step.id)))?;
    let input = &inputs[0];
    let rows = match &step.kind {
        StepKind::Filter { predicates } => filter(input, predicates),
        StepKind::Aggregate { group_by, measures } => aggregate(input, group_by, measures)?,
        StepKind::Join { how, on } => join(*how, on, &inputs[0], &inputs[1], &schema),
        StepKind::Append { distinct, origin } => append(&step.inputs, inputs, &schema, *distinct, origin.as_deref()),
        _ => unreachable!("output_schema returned Some for a non-table step"),
    };
    Ok(Table { schema, rows })
}

fn join(how: JoinKind, on: &[JoinKey], left: &Table, right: &Table, out: &Schema) -> Vec<Vec<Value>> {
    let lk: Vec<usize> = on.iter().map(|k| left.schema.index_of(&k.left).unwrap()).collect();
    let rk: Vec<usize> = on.iter().map(|k| right.schema.index_of(&k.right).unwrap()).collect();
    let eq: Vec<bool> = on.iter().map(|k| k.op == CmpOp::Eq).collect();
    let dropped: Vec<usize> = rk.iter().zip(&eq).filter(|(_, e)| **e).map(|(i, _)| *i).collect();
    let right_keep: Vec<usize> = (0..right.schema.fields.len()).filter(|i| !dropped.contains(i)).collect();
    let matches = |l: &[Value], r: &[Value]| {
        on.iter().zip(lk.iter().zip(&rk)).all(|(k, (&a, &b))| l[a].sql_cmp(&r[b]).is_some_and(|o| holds_cmp(k.op, o)))
    };
    // Typed as the output column: a full join widens mixed key types.
    let fit = |v: Value, col: usize| match (out.fields[col].dtype, v) {
        (DataType::Float, Value::Int(i)) => Value::Float(i as f64),
        (DataType::Timestamp, Value::Date(d)) => Value::Timestamp(d as i64 * crate::table::MICROS_PER_DAY),
        (_, v) => v,
    };
    let row_of = |l: Option<&Vec<Value>>, r: Option<&Vec<Value>>| -> Vec<Value> {
        let mut row = Vec::with_capacity(out.fields.len());
        for i in 0..left.schema.fields.len() {
            let mut v = l.map(|l| l[i].clone()).unwrap_or(Value::Null);
            if how == JoinKind::Full && v.is_null() {
                if let (Some(k), Some(r)) = (lk.iter().zip(&eq).position(|(&x, &e)| x == i && e), r) {
                    v = r[rk[k]].clone();
                }
            }
            row.push(fit(v, i));
        }
        for &j in &right_keep {
            row.push(r.map(|r| r[j].clone()).unwrap_or(Value::Null));
        }
        row
    };
    let mut rows = Vec::new();
    let mut right_matched = vec![false; right.rows.len()];
    for l in &left.rows {
        let mut any = false;
        for (j, r) in right.rows.iter().enumerate() {
            if matches(l, r) {
                any = true;
                right_matched[j] = true;
                rows.push(row_of(Some(l), Some(r)));
            }
        }
        if !any && how != JoinKind::Inner {
            rows.push(row_of(Some(l), None));
        }
    }
    if how == JoinKind::Full {
        for (j, r) in right.rows.iter().enumerate() {
            if !right_matched[j] {
                rows.push(row_of(None, Some(r)));
            }
        }
    }
    rows
}

fn append(ids: &[String], inputs: &[Table], out: &Schema, distinct: bool, origin: Option<&str>) -> Vec<Vec<Value>> {
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (id, t) in ids.iter().zip(inputs) {
        for r in &t.rows {
            let row: Vec<Value> = out
                .fields
                .iter()
                .map(|f| {
                    if Some(f.name.as_str()) == origin {
                        return Value::Text(id.clone());
                    }
                    let v = t.schema.index_of(&f.name).map(|i| r[i].clone()).unwrap_or(Value::Null);
                    match (f.dtype, v) {
                        (DataType::Float, Value::Int(i)) => Value::Float(i as f64),
                        (DataType::Timestamp, Value::Date(d)) => Value::Timestamp(d as i64 * crate::table::MICROS_PER_DAY),
                        (_, v) => v,
                    }
                })
                .collect();
            if distinct && !seen.insert(row.iter().map(Value::key).collect::<Vec<_>>().join("\u{1f}")) {
                continue;
            }
            rows.push(row);
        }
    }
    rows
}

fn holds_cmp(op: CmpOp, ord: Ordering) -> bool {
    match op {
        CmpOp::Eq => ord == Ordering::Equal,
        CmpOp::Ne => ord != Ordering::Equal,
        CmpOp::Lt => ord == Ordering::Less,
        CmpOp::Le => ord != Ordering::Greater,
        CmpOp::Gt => ord == Ordering::Greater,
        CmpOp::Ge => ord != Ordering::Less,
        CmpOp::IsNull | CmpOp::IsNotNull => false,
    }
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
        Step { id: "s".into(), inputs: vec![SOURCE_ID.into()], label: None, kind }
    }

    #[test]
    fn filter_excludes_nulls_from_comparisons() {
        let s = step(StepKind::Filter {
            predicates: vec![Predicate { column: "amount".into(), op: CmpOp::Gt, value: Value::Int(5) }],
        });
        let out = eval_step(&s, &[data()]).unwrap();
        assert_eq!(out.rows.len(), 2); // 10.5 and 7; NULL amount excluded
        let ne = step(StepKind::Filter {
            predicates: vec![Predicate { column: "region".into(), op: CmpOp::Ne, value: Value::Text("East".into()) }],
        });
        assert_eq!(eval_step(&ne, &[data()]).unwrap().rows.len(), 2); // NULL region excluded
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
        let out = eval_step(&s, &[data()]).unwrap();
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
        assert_eq!(eval_step(&s, std::slice::from_ref(&empty)).unwrap().rows, vec![vec![Value::Int(0), Value::Null]]);
    }
}
