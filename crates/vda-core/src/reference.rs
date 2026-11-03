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
    if let StepKind::Pivot { rows, column, value, func } = &step.kind {
        output_schema(step, &schemas)?;
        return pivot(&inputs[0], rows, column, value.as_deref(), *func);
    }
    let schema = output_schema(step, &schemas)?
        .ok_or_else(|| Error::Unsupported(format!("step `{}` is not a visual table step", step.id)))?;
    let input = &inputs[0];
    let rows = match &step.kind {
        StepKind::Filter { predicates } => filter(input, predicates),
        StepKind::Aggregate { group_by, measures } => aggregate(input, group_by, measures)?,
        StepKind::Join { how, on } => join(*how, on, &inputs[0], &inputs[1], &schema),
        StepKind::Append { distinct, origin } => append(&step.inputs, inputs, &schema, *distinct, origin.as_deref()),
        StepKind::Derive { columns } => derive(step, columns, input)?,
        StepKind::Select { columns } => {
            let idx: Vec<usize> = columns.iter().map(|c| input.schema.index_of(&c.column).unwrap()).collect();
            input.rows.iter().map(|r| idx.iter().map(|&i| r[i].clone()).collect()).collect()
        }
        StepKind::Sort { by, limit } => sort(by, *limit, input),
        StepKind::Window { partition_by, order_by, columns } => window(input, partition_by, order_by, columns, &schema)?,
        StepKind::Dedupe { keys, order_by } => dedupe(input, keys, order_by),
        StepKind::Unpivot { columns, .. } => unpivot(input, columns, &schema),
        StepKind::Check { .. } => input.rows.clone(),
        StepKind::Test { .. } => {
            return Err(Error::Unsupported(format!("step `{}`: a test runs its summaries as steps (stat_test.rs)", step.id)))
        }
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

fn derive(step: &Step, columns: &[crate::model::Derived], input: &Table) -> Result<Vec<Vec<Value>>, Error> {
    let plan = crate::compile::derive_plan(step, &input.schema)?;
    let mut rows = Vec::with_capacity(input.rows.len());
    for r in &input.rows {
        let mut row = r.clone();
        for ((expr, schema), c) in plan.iter().zip(columns) {
            let v = crate::formula::eval(expr, schema, &row)?;
            match schema.index_of(&c.name) {
                Some(i) => row[i] = v,
                None => row.push(v),
            }
        }
        rows.push(row);
    }
    Ok(rows)
}

fn sort(by: &[crate::model::SortKey], limit: Option<u64>, input: &Table) -> Vec<Vec<Value>> {
    let idx: Vec<(usize, bool)> = by.iter().map(|k| (input.schema.index_of(&k.column).unwrap(), k.descending)).collect();
    let mut rows = input.rows.clone();
    // A stable sort keeps ties in input order. Missing values go last either way.
    rows.sort_by(|a, b| {
        for &(i, desc) in &idx {
            let o = match (a[i].is_null(), b[i].is_null()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                _ => {
                    let o = a[i].sql_cmp(&b[i]).unwrap_or(Ordering::Equal);
                    if desc { o.reverse() } else { o }
                }
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    });
    if let Some(n) = limit {
        rows.truncate(n as usize);
    }
    rows
}

/// Row indexes grouped by the values of `cols` (missing values are equal),
/// groups in order of first appearance.
fn partitions(input: &Table, cols: &[usize]) -> Vec<Vec<usize>> {
    let mut order: Vec<Vec<usize>> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for (i, r) in input.rows.iter().enumerate() {
        let key = cols.iter().map(|&c| r[c].key()).collect::<Vec<_>>().join("\u{1f}");
        let g = *at.entry(key).or_insert_with(|| {
            order.push(Vec::new());
            order.len() - 1
        });
        order[g].push(i);
    }
    order
}

/// Compare rows by sort keys, missing values last; `Equal` for ties.
fn by_keys(keys: &[(usize, bool)], a: &[Value], b: &[Value]) -> Ordering {
    for &(i, desc) in keys {
        let o = match (a[i].is_null(), b[i].is_null()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => {
                let o = a[i].sql_cmp(&b[i]).unwrap_or(Ordering::Equal);
                if desc { o.reverse() } else { o }
            }
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

fn key_index(input: &Table, by: &[crate::model::SortKey]) -> Vec<(usize, bool)> {
    by.iter().map(|k| (input.schema.index_of(&k.column).unwrap(), k.descending)).collect()
}

fn window(
    input: &Table,
    partition_by: &[String],
    order_by: &[crate::model::SortKey],
    columns: &[crate::model::WindowColumn],
    out: &Schema,
) -> Result<Vec<Vec<Value>>, Error> {
    use crate::model::WindowFunc as W;
    let pidx: Vec<usize> = partition_by.iter().map(|c| input.schema.index_of(c).unwrap()).collect();
    let keys = key_index(input, order_by);
    let mut rows: Vec<Vec<Value>> = input.rows.clone();
    for mut part in partitions(input, &pidx) {
        // A stable sort: ties stay in input order.
        part.sort_by(|&a, &b| by_keys(&keys, &input.rows[a], &input.rows[b]));
        for c in columns {
            let col = c.column.as_ref().map(|n| input.schema.index_of(n).unwrap());
            let val = |pos: usize| input.rows[part[pos]][col.unwrap()].clone();
            let n = c.n.unwrap_or(1) as usize;
            let total: Option<f64> = if c.func == W::ShareOfTotal {
                let vals: Vec<Value> = (0..part.len()).map(val).filter(|v| !v.is_null()).collect();
                if vals.is_empty() {
                    None
                } else if vals.iter().all(|v| matches!(v, Value::Int(_))) {
                    Some(vals.iter().map(|v| if let Value::Int(i) = v { *i as i128 } else { 0 }).sum::<i128>() as f64)
                } else {
                    Some(vals.iter().filter_map(Value::as_f64).sum())
                }
            } else {
                None
            };
            let is_int = col.is_some_and(|i| input.schema.fields[i].dtype == DataType::Int);
            let mut running_i: Option<i64> = None;
            let mut running_f: Option<f64> = None;
            for pos in 0..part.len() {
                let v = match c.func {
                    W::RowNumber => Value::Int(pos as i64 + 1),
                    W::Rank => {
                        let mut first = pos;
                        while first > 0 && by_keys(&keys, &input.rows[part[first - 1]], &input.rows[part[pos]]) == Ordering::Equal {
                            first -= 1;
                        }
                        Value::Int(first as i64 + 1)
                    }
                    W::Previous => if pos >= n { val(pos - n) } else { Value::Null },
                    W::Next => if pos + n < part.len() { val(pos + n) } else { Value::Null },
                    W::RunningSum => {
                        match val(pos) {
                            Value::Int(i) if is_int => {
                                running_i = Some(running_i.unwrap_or(0).checked_add(i).ok_or_else(|| {
                                    Error::Data(format!("running sum overflow in `{}`", c.name))
                                })?)
                            }
                            v => {
                                if let Some(x) = v.as_f64() {
                                    running_f = Some(running_f.unwrap_or(0.0) + x)
                                }
                            }
                        }
                        if is_int { running_i.map(Value::Int).unwrap_or(Value::Null) } else { running_f.map(Value::Float).unwrap_or(Value::Null) }
                    }
                    W::MovingAverage => {
                        let from = (pos + 1).saturating_sub(n);
                        let xs: Vec<f64> = (from..=pos).filter_map(|p| val(p).as_f64()).collect();
                        if xs.is_empty() { Value::Null } else { Value::Float(xs.iter().sum::<f64>() / xs.len() as f64) }
                    }
                    W::Change => match (val(pos), if pos >= 1 { val(pos - 1) } else { Value::Null }) {
                        (Value::Int(a), Value::Int(b)) => Value::Int(
                            a.checked_sub(b).ok_or_else(|| Error::Data(format!("change overflow in `{}`", c.name)))?,
                        ),
                        (a, b) => match (a.as_f64(), b.as_f64()) {
                            (Some(a), Some(b)) => Value::Float(a - b),
                            _ => Value::Null,
                        },
                    },
                    W::PercentChange => match (val(pos).as_f64(), if pos >= 1 { val(pos - 1).as_f64() } else { None }) {
                        (Some(a), Some(b)) if b != 0.0 => Value::Float((a - b) / b),
                        _ => Value::Null,
                    },
                    W::ShareOfTotal => match (val(pos).as_f64(), total) {
                        (Some(x), Some(t)) if t != 0.0 => Value::Float(x / t),
                        _ => Value::Null,
                    },
                };
                rows[part[pos]].push(v);
            }
        }
    }
    debug_assert!(rows.iter().all(|r| r.len() == out.fields.len()));
    Ok(rows)
}

fn dedupe(input: &Table, keys: &[String], order_by: &[crate::model::SortKey]) -> Vec<Vec<Value>> {
    let kidx: Vec<usize> = if keys.is_empty() {
        (0..input.schema.fields.len()).collect()
    } else {
        keys.iter().map(|k| input.schema.index_of(k).unwrap()).collect()
    };
    let order = key_index(input, order_by);
    let mut keep: Vec<usize> = partitions(input, &kidx)
        .into_iter()
        .map(|g| *g.iter().min_by(|&&a, &&b| by_keys(&order, &input.rows[a], &input.rows[b]).then(a.cmp(&b))).unwrap())
        .collect();
    keep.sort_unstable();
    keep.into_iter().map(|i| input.rows[i].clone()).collect()
}

fn unpivot(input: &Table, columns: &[String], out: &Schema) -> Vec<Vec<Value>> {
    let idx: Vec<usize> = columns.iter().map(|c| input.schema.index_of(c).unwrap()).collect();
    let kept: Vec<usize> = (0..input.schema.fields.len()).filter(|i| !idx.contains(i)).collect();
    let float = out.fields.last().unwrap().dtype == DataType::Float;
    let mut rows = Vec::new();
    for r in &input.rows {
        for (c, &i) in columns.iter().zip(&idx) {
            let mut row: Vec<Value> = kept.iter().map(|&k| r[k].clone()).collect();
            row.push(Value::Text(c.clone()));
            row.push(match &r[i] {
                Value::Int(n) if float => Value::Float(*n as f64),
                v => v.clone(),
            });
            rows.push(row);
        }
    }
    rows
}

/// One row per group of `rows` (sorted, missing last), one column per value
/// of `column` (sorted; missing values are left out), named by the value.
fn pivot(input: &Table, rows: &[String], column: &str, value: Option<&str>, func: AggFunc) -> Result<Table, Error> {
    let cidx = input.schema.index_of(column).unwrap();
    let mut values: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in &input.rows {
        if !r[cidx].is_null() && seen.insert(r[cidx].key()) {
            values.push(r[cidx].clone());
        }
    }
    values.sort_by(|a, b| a.sort_cmp(b));
    let dtype = crate::compile::measure_type("pivot", func, value, &input.schema)?;
    let mut fields: Vec<crate::table::Field> =
        rows.iter().map(|r| input.schema.field(r).unwrap().clone()).collect();
    for v in &values {
        // A value named like an earlier column gets a suffix, as in DuckDB.
        let mut name = v.to_string();
        let mut k = 1;
        while fields.iter().any(|f| f.name == name) {
            name = format!("{v}_{k}");
            k += 1;
        }
        fields.push(crate::table::Field { name, dtype });
    }
    let gidx: Vec<usize> = rows.iter().map(|r| input.schema.index_of(r).unwrap()).collect();
    let mut groups = partitions(input, &gidx);
    if rows.is_empty() && groups.is_empty() {
        groups.push(Vec::new());
    }
    groups.sort_by(|a, b| {
        let (ra, rb) = (&input.rows[a[0]], &input.rows[b[0]]);
        gidx.iter().map(|&i| ra[i].sort_cmp(&rb[i])).find(|o| *o != Ordering::Equal).unwrap_or(Ordering::Equal)
    });
    let m = Measure { func, column: value.map(str::to_string), alias: "pivot".into() };
    let mut out = Vec::new();
    for g in groups {
        let mut row: Vec<Value> = gidx.iter().map(|&i| input.rows[g[0]][i].clone()).collect();
        for v in &values {
            let cell: Vec<&Vec<Value>> =
                g.iter().map(|&i| &input.rows[i]).filter(|r| r[cidx].key() == v.key()).collect();
            row.push(measure(&m, &cell, input)?);
        }
        out.push(row);
    }
    Ok(Table { schema: Schema { fields }, rows: out })
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

/// Continuous quantile: linear interpolation between the closest ranks.
fn quantile(vals: &[&Value], p: f64) -> Value {
    let mut xs: Vec<f64> = vals.iter().filter_map(|v| v.as_f64()).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let h = (xs.len() - 1) as f64 * p;
    let lo = h.floor() as usize;
    let hi = (lo + 1).min(xs.len() - 1);
    Value::Float(xs[lo] + (h - lo as f64) * (xs[hi] - xs[lo]))
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
    if m.func == AggFunc::CountDistinct {
        let keys: std::collections::HashSet<String> = vals.iter().map(|v| v.key()).collect();
        return Ok(Value::Int(keys.len() as i64));
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
        AggFunc::Median => quantile(&vals, 0.5),
        AggFunc::Percentile(p) => quantile(&vals, p as f64 / 100.0),
        AggFunc::Stddev => {
            let xs: Vec<f64> = vals.iter().filter_map(|v| v.as_f64()).collect();
            if xs.len() < 2 {
                Value::Null
            } else {
                let mean = xs.iter().sum::<f64>() / xs.len() as f64;
                let ss: f64 = xs.iter().map(|x| (x - mean) * (x - mean)).sum();
                Value::Float((ss / (xs.len() - 1) as f64).sqrt())
            }
        }
        AggFunc::CountDistinct => unreachable!(),
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
            predicates: vec![Predicate { param: None, column: "amount".into(), op: CmpOp::Gt, value: Value::Int(5) }],
        });
        let out = eval_step(&s, &[data()]).unwrap();
        assert_eq!(out.rows.len(), 2); // 10.5 and 7; NULL amount excluded
        let ne = step(StepKind::Filter {
            predicates: vec![Predicate { param: None, column: "region".into(), op: CmpOp::Ne, value: Value::Text("East".into()) }],
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
