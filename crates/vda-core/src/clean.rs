//! The Clean step: find missing values in some columns and fill them in.
//!
//! Finding is built from formulas, so it means the same in DuckDB and the
//! reference engine (ADR-0006): a value counts as missing when it is empty,
//! matches one of the step's markers ("N/A", -999), makes the column's
//! `missing_when` formula true, or doesn't convert to the column's new type.
//!
//! Filling works on the values found, within the column's groups:
//! mean, median, mode, a constant, a random known value (a seeded hash of
//! the row number, identical in SQL and Rust), the previous or next known
//! value, a fitted line, or the nearest neighbours' values. The last two are
//! fitted in Rust before the step runs ([`fit_regression`], [`neighbours`])
//! and resolve to [`Fill::Linear`] and [`Fill::Rows`], which both engines
//! apply the same way. [`sql`] compiles the step for DuckDB; [`run`] is the
//! reference implementation the cross-engine tests hold it to.

use crate::compile::quote_ident;
use crate::formula::{self, Expr, Op};
use crate::json::Json;
use crate::model::{Fill, SortKey, Step, StepKind};
use crate::table::{parse_date, DataType, Field, Schema, Table, Value};
use crate::Error;
use std::cmp::Ordering;
use std::collections::HashMap;

/// At most this many predictors per fitted fill.
pub const MAX_PREDICTORS: usize = 10;
/// At most this many neighbours.
pub const MAX_K: u32 = 50;
/// Rows a regression is fitted on (an even spread when there are more).
pub const FIT_ROWS: u64 = 50_000;
/// Rows nearest neighbours compare with.
pub const NEIGHBOUR_ROWS: u64 = 5_000;
/// Missing values nearest neighbours fill at most; more is an error that
/// suggests a faster method.
pub const MAX_NEIGHBOUR_FILLS: u64 = 20_000;

/// Internal column names (see `StepKind::Clean::internal`).
pub const ROW: &str = "__vda_r";
pub const DROP: &str = "__vda_drop";
pub fn why_column(i: usize) -> String {
    format!("__vda_w{i}")
}

/// Why a value counts as missing, as numbered in `__vda_w<n>`.
pub const WHY: [&str; 5] = ["known", "empty", "marker", "condition", "conversion"];

fn err(step: &Step, m: impl std::fmt::Display) -> Error {
    Error::Validation(format!("step `{}`: {m}", step.id))
}

fn numeric(t: DataType) -> bool {
    matches!(t, DataType::Int | DataType::Float)
}

/// One column the step cleans.
#[derive(Debug, Clone)]
pub struct ColPlan {
    /// Position in `step.columns`.
    pub index: usize,
    pub name: String,
    /// Position in the input (and output) schema.
    pub pos: usize,
    /// Type after conversion.
    pub dtype: DataType,
    /// Why it is missing: 0 known, 1 empty, 2 marker, 3 condition, 4 conversion.
    pub why: Expr,
    /// The value found: missing where `why` > 0, converted otherwise.
    pub value: Expr,
    pub fill: Fill,
    pub group: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub cols: Vec<ColPlan>,
    /// The input with converted columns retyped: what fills read.
    pub marked: Schema,
    /// The step's output schema.
    pub out: Schema,
    pub flag: bool,
    pub internal: bool,
    pub seed: u64,
}

fn col(name: &str) -> Expr {
    Expr::Col(name.to_string())
}
fn call(f: &str, args: Vec<Expr>) -> Expr {
    Expr::Call(f.into(), args)
}
fn bin(op: Op, a: Expr, b: Expr) -> Expr {
    Expr::Bin(op, Box::new(a), Box::new(b))
}
fn int(i: i64) -> Expr {
    Expr::Lit(Value::Int(i))
}

/// A marker as a test of the column, if it can match a value of this type.
fn marker_test(name: &str, dtype: DataType, marker: &str) -> Option<Expr> {
    let m = marker.trim();
    match dtype {
        DataType::Int | DataType::Float => {
            let v = if let Ok(i) = m.parse::<i64>() {
                Value::Int(i)
            } else {
                Value::Float(m.parse::<f64>().ok().filter(|x| x.is_finite())?)
            };
            Some(bin(Op::Eq, col(name), Expr::Lit(v)))
        }
        // Text compares without case and surrounding spaces; other types by
        // their text ("1900-01-01").
        _ => {
            let side = if dtype == DataType::Text { col(name) } else { call("text", vec![col(name)]) };
            let norm = |e: Expr| call("lower", vec![call("trim", vec![e])]);
            Some(bin(Op::Eq, norm(side), norm(Expr::Lit(Value::Text(marker.to_string())))))
        }
    }
}

/// Parse a constant as typed for a column of this type.
pub fn parse_constant(dtype: DataType, text: &str) -> Option<Value> {
    let t = text.trim();
    match dtype {
        DataType::Int => t.parse::<i64>().ok().map(Value::Int),
        DataType::Float => t.parse::<f64>().ok().filter(|x| x.is_finite()).map(Value::Float),
        DataType::Text => Some(Value::Text(text.to_string())),
        DataType::Date => parse_date(t).map(Value::Date),
        DataType::Bool => match t.to_ascii_lowercase().as_str() {
            "true" | "yes" => Some(Value::Bool(true)),
            "false" | "no" => Some(Value::Bool(false)),
            _ => None,
        },
        DataType::Timestamp => None,
    }
}

/// Validate the step against its input and work out what it does.
pub fn plan(step: &Step, input: &Schema) -> Result<Plan, Error> {
    let StepKind::Clean { markers, columns, flag, seed, internal } = &step.kind else { unreachable!("not a clean step") };
    if columns.is_empty() {
        return Err(err(step, "choose a column to clean"));
    }
    let mut marked = input.clone();
    // Conversions first: groups, orders and predictors read converted values.
    for c in columns {
        let pos = input.index_of(&c.column).ok_or_else(|| err(step, format!("no column `{}`", c.column)))?;
        if columns.iter().filter(|d| d.column == c.column).count() > 1 {
            return Err(err(step, format!("`{}` is listed twice", c.column)));
        }
        if let Some(t) = c.as_type {
            let from = input.fields[pos].dtype;
            if t != from {
                if from != DataType::Text || !matches!(t, DataType::Int | DataType::Float | DataType::Date) {
                    return Err(err(
                        step,
                        format!("`{}` is {}; only text can be read as numbers, whole numbers or dates", c.column, from.plain()),
                    ));
                }
                marked.fields[pos].dtype = t;
            }
        }
    }
    let mut cols = Vec::new();
    for (index, c) in columns.iter().enumerate() {
        let pos = input.index_of(&c.column).expect("checked");
        let from = input.fields[pos].dtype;
        let dtype = marked.fields[pos].dtype;
        let name = c.column.as_str();
        let tests: Vec<Expr> = markers.iter().filter_map(|m| marker_test(name, from, m)).collect();
        let any_marker = tests.into_iter().reduce(|a, b| bin(Op::Or, a, b));
        let cond = match c.missing_when.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
            Some(f) => {
                let e = formula::parse(f).map_err(|m| err(step, format!("`{name}`, missing when: {m}")))?;
                let t = formula::result_type(&e, input).map_err(|m| err(step, format!("`{name}`, missing when: {m}")))?;
                if t != DataType::Bool {
                    return Err(err(step, format!("`{name}`, missing when: the formula must be true or false (like `[{name}] < 0`)")));
                }
                Some(call("coalesce", vec![e, Expr::Lit(Value::Bool(false))]))
            }
            None => None,
        };
        let conv = if dtype != from {
            let f = match dtype {
                DataType::Int => "integer",
                DataType::Float => "number",
                _ => "date",
            };
            Some(call(f, vec![call("trim", vec![col(name)])]))
        } else {
            None
        };
        // why = if(empty, 1, if(marker, 2, if(condition, 3, if(no conversion, 4, 0))))
        let mut why = int(0);
        if let Some(cv) = &conv {
            why = call("if", vec![Expr::IsMissing(Box::new(cv.clone()), false), int(4), why]);
        }
        if let Some(cd) = &cond {
            why = call("if", vec![cd.clone(), int(3), why]);
        }
        if let Some(mk) = &any_marker {
            why = call("if", vec![mk.clone(), int(2), why]);
        }
        why = call("if", vec![Expr::IsMissing(Box::new(col(name)), false), int(1), why]);
        let found = match (&any_marker, &cond) {
            (Some(a), Some(b)) => Some(bin(Op::Or, a.clone(), b.clone())),
            (Some(a), None) | (None, Some(a)) => Some(a.clone()),
            (None, None) => None,
        };
        let base = conv.unwrap_or_else(|| col(name));
        let value = match found {
            Some(f) => call("if", vec![f, Expr::Lit(Value::Null), base]),
            None => base,
        };
        for e in [&why, &value] {
            formula::result_type(e, input).map_err(|m| err(step, format!("`{name}`: {m}")))?;
        }
        // Groups and fills, against the converted columns.
        let need = |c2: &str, what: &str| -> Result<DataType, Error> {
            if c2 == name {
                return Err(err(step, format!("`{name}` can't be its own {what}")));
            }
            marked.index_of(c2).map(|i| marked.fields[i].dtype).ok_or_else(|| err(step, format!("`{name}`: no column `{c2}` to use as {what}")))
        };
        for g in &c.group_by {
            need(g, "group")?;
        }
        let list = |v: &[String], what: &str| -> Result<(), Error> {
            if v.is_empty() {
                return Err(err(step, format!("`{name}`: choose the columns to {what} from")));
            }
            if v.len() > MAX_PREDICTORS {
                return Err(err(step, format!("`{name}`: at most {MAX_PREDICTORS} columns to {what} from")));
            }
            for (i, p) in v.iter().enumerate() {
                if v[..i].contains(p) {
                    return Err(err(step, format!("`{name}`: `{p}` is listed twice")));
                }
                if !numeric(need(p, "predictor")?) {
                    return Err(err(step, format!("`{name}`: `{p}` isn't a number column; predictions use number columns")));
                }
            }
            Ok(())
        };
        match &c.fill {
            Fill::Mean | Fill::Median if !numeric(dtype) => {
                return Err(err(step, format!("`{name}` is {}; a {} needs numbers", dtype.plain(), c.fill.name())))
            }
            Fill::Constant { value } if parse_constant(dtype, value).is_none() => {
                let a = match dtype {
                    DataType::Int => "a whole number",
                    DataType::Float => "a number",
                    DataType::Date => "a date (like 2026-01-31)",
                    DataType::Bool => "true or false",
                    DataType::Timestamp => "a date; fill times with a calculated column",
                    DataType::Text => "text",
                };
                return Err(err(step, format!("`{name}`: \"{value}\" isn't {a}")))
            }
            Fill::Previous { order_by } | Fill::Next { order_by } => {
                for k in order_by {
                    if k.column != name {
                        need(&k.column, "order")?;
                    }
                }
            }
            Fill::Regression { predictors } => {
                if !numeric(dtype) {
                    return Err(err(step, format!("`{name}` is {}; a regression predicts numbers (try nearest neighbours)", dtype.plain())));
                }
                list(predictors, "predict")?;
            }
            Fill::Neighbours { predictors, k } => {
                list(predictors, "compare")?;
                if *k == 0 || *k > MAX_K {
                    return Err(err(step, format!("`{name}`: use between 1 and {MAX_K} neighbours")));
                }
            }
            Fill::Linear { terms, .. } => {
                for (p, _) in terms {
                    need(p, "predictor")?;
                }
            }
            _ => {}
        }
        cols.push(ColPlan { index, name: name.to_string(), pos, dtype, why, value, fill: c.fill.clone(), group: c.group_by.clone() });
    }
    let mut out = marked.clone();
    if *flag {
        for c in &cols {
            let f = format!("{}_missing", c.name);
            if out.index_of(&f).is_some() {
                return Err(err(step, format!("there is already a column `{f}`; turn off the missing-value columns or rename it")));
            }
            out.fields.push(Field { name: f, dtype: DataType::Bool });
        }
    }
    if *internal {
        out.fields.push(Field { name: ROW.into(), dtype: DataType::Int });
        for c in &cols {
            out.fields.push(Field { name: why_column(c.index), dtype: DataType::Int });
        }
        out.fields.push(Field { name: DROP.into(), dtype: DataType::Bool });
    }
    Ok(Plan { cols, marked, out, flag: *flag, internal: *internal, seed: *seed })
}

/// The step's output schema.
pub fn validate(step: &Step, input: &Schema) -> Result<Schema, Error> {
    Ok(plan(step, input)?.out)
}

// ------------------------------------------------------------ random draws

/// Mixes the step's seed and the column's position into the draw.
fn draw_salt(seed: u64, index: usize) -> u64 {
    ((seed as u128 % (1 << 32)) * 40503 + (index as u128 + 1) * 9973) as u64 % (1 << 32)
}

/// A 32-bit hash of the row number: which known value a missing one draws.
/// The same arithmetic in SQL ([`draw_sql`]) and here.
pub fn draw(row: u64, salt: u64) -> u64 {
    let m = 1u64 << 32;
    let mut h = ((row as u128 * 2_654_435_761 + salt as u128) % m as u128) as u64;
    h = ((h ^ (h >> 16)) * 73_244_475) % m;
    h = ((h ^ (h >> 16)) * 73_244_475) % m;
    h ^ (h >> 16)
}

// --------------------------------------------------------------------- SQL

fn sql_type(t: DataType) -> &'static str {
    match t {
        DataType::Bool => "BOOLEAN",
        DataType::Int => "BIGINT",
        DataType::Float => "DOUBLE",
        DataType::Text => "VARCHAR",
        DataType::Date => "DATE",
        DataType::Timestamp => "TIMESTAMP",
    }
}

fn sql_value(v: &Value) -> String {
    match v {
        Value::Float(x) => format!("CAST('{x:?}' AS DOUBLE)"),
        v => crate::compile::quote_literal(v),
    }
}

fn float_sql(x: f64) -> String {
    format!("CAST('{x:?}' AS DOUBLE)")
}

/// The SQL for the step, reading the relation `input`. Fitted fills must be
/// resolved first (see the module notes).
pub fn sql(step: &Step, input: &Schema) -> Result<String, Error> {
    let p = plan(step, input)?;
    if let Some(c) = p.cols.iter().find(|c| c.fill.is_fitted()) {
        return Err(Error::Unsupported(format!(
            "step `{}`: `{}` is filled by a {} fitted when the step runs",
            step.id,
            c.name,
            if matches!(c.fill, Fill::Regression { .. }) { "regression" } else { "nearest-neighbour search" }
        )));
    }
    let tosql = |e: &Expr| formula::to_sql(e, input).map_err(|m| err(step, m));
    let q = |n: &str| quote_ident(n);
    let mut ctes = vec![format!("__vda_base AS (SELECT *, row_number() OVER () AS {ROW} FROM input)")];
    let mut replace = Vec::new();
    let mut whys = Vec::new();
    for c in &p.cols {
        if c.value != col(&c.name) {
            replace.push(format!("{} AS {}", tosql(&c.value)?, q(&c.name)));
        }
        whys.push(format!("{} AS {}", tosql(&c.why)?, why_column(c.index)));
    }
    let replace = if replace.is_empty() { String::new() } else { format!(" REPLACE ({})", replace.join(", ")) };
    ctes.push(format!("__vda_m AS (SELECT *{replace}, {} FROM __vda_base)", whys.join(", ")));

    let mut joins = Vec::new();
    let mut filled: HashMap<usize, String> = HashMap::new();
    let fit = |c: &ColPlan, e: String| if c.dtype == DataType::Int { format!("CAST(round({e}) AS BIGINT)") } else { e };
    for c in &p.cols {
        let qc = format!("m.{}", q(&c.name));
        let i = c.index;
        let part = if c.group.is_empty() {
            String::new()
        } else {
            format!("PARTITION BY {}", c.group.iter().map(|g| format!("m.{}", q(g))).collect::<Vec<_>>().join(", "))
        };
        let gsel = |prefix: &str| c.group.iter().enumerate().map(|(j, g)| format!("{prefix}{} AS __g{j}", q(g))).collect::<Vec<_>>();
        let gmatch = |t: &str| {
            if c.group.is_empty() {
                "TRUE".to_string()
            } else {
                c.group.iter().enumerate().map(|(j, g)| format!("m.{} IS NOT DISTINCT FROM {t}.__g{j}", q(g))).collect::<Vec<_>>().join(" AND ")
            }
        };
        let e = match &c.fill {
            Fill::Keep | Fill::DropRows => qc.clone(),
            Fill::Mean => format!("COALESCE({qc}, {})", fit(c, format!("avg({qc}) OVER ({part})"))),
            Fill::Median => format!("COALESCE({qc}, {})", fit(c, format!("median({qc}) OVER ({part})"))),
            Fill::Constant { value } => format!("COALESCE({qc}, {})", sql_value(&parse_constant(c.dtype, value).expect("validated"))),
            Fill::Mode => {
                let g: Vec<String> = c.group.iter().map(|g| q(g)).collect();
                let by = if g.is_empty() { String::new() } else { format!("PARTITION BY {} ", g.join(", ")) };
                let keys: Vec<String> = g.iter().chain(std::iter::once(&q(&c.name))).cloned().collect();
                let gs = gsel("");
                ctes.push(format!(
                    "__vda_o{i} AS (SELECT * EXCLUDE (rn) FROM (SELECT {}{} AS v, row_number() OVER ({by}ORDER BY count(*) DESC, {} ASC) AS rn \
                     FROM __vda_m WHERE {} IS NOT NULL GROUP BY {}) WHERE rn = 1)",
                    gs.iter().map(|s| format!("{s}, ")).collect::<String>(),
                    q(&c.name),
                    q(&c.name),
                    q(&c.name),
                    keys.join(", ")
                ));
                joins.push(format!("LEFT JOIN __vda_o{i} ON {}", gmatch(&format!("__vda_o{i}"))));
                format!("COALESCE({qc}, __vda_o{i}.v)")
            }
            Fill::Random => {
                let gs = gsel("");
                let g: Vec<String> = c.group.iter().map(|g| q(g)).collect();
                let by = if g.is_empty() { String::new() } else { format!("PARTITION BY {} ", g.join(", ")) };
                let extra = gs.iter().map(|s| format!(", {s}")).collect::<String>();
                let carry = (0..c.group.len()).map(|j| format!(", __g{j}")).collect::<String>();
                let salt = draw_salt(p.seed, i);
                let gm = |a: &str, b: &str| {
                    if c.group.is_empty() {
                        "TRUE".to_string()
                    } else {
                        (0..c.group.len()).map(|j| format!("{a}.__g{j} IS NOT DISTINCT FROM {b}.__g{j}")).collect::<Vec<_>>().join(" AND ")
                    }
                };
                ctes.push(format!(
                    "__vda_k{i} AS (SELECT {} AS v{extra}, row_number() OVER ({by}ORDER BY {ROW}) AS k, count(*) OVER ({}) AS n \
                     FROM __vda_m WHERE {} IS NOT NULL)",
                    q(&c.name),
                    by.trim(),
                    q(&c.name)
                ));
                ctes.push(format!(
                    "__vda_h{i} AS (SELECT {ROW}{extra}, ({ROW} * 2654435761 + {salt}) % 4294967296 AS h FROM __vda_m WHERE {} IS NULL)",
                    q(&c.name)
                ));
                ctes.push(format!("__vda_h{i}b AS (SELECT {ROW}{carry}, (xor(h, h >> 16) * 73244475) % 4294967296 AS h FROM __vda_h{i})"));
                ctes.push(format!("__vda_h{i}c AS (SELECT {ROW}{carry}, (xor(h, h >> 16) * 73244475) % 4294967296 AS h FROM __vda_h{i}b)"));
                ctes.push(format!(
                    "__vda_p{i} AS (SELECT h.{ROW}, k.v FROM (SELECT {ROW}{carry}, xor(h, h >> 16) AS h FROM __vda_h{i}c) AS h \
                     JOIN __vda_k{i} AS k ON {} AND k.k = h.h % k.n + 1)",
                    gm("h", "k")
                ));
                joins.push(format!("LEFT JOIN __vda_p{i} ON __vda_p{i}.{ROW} = m.{ROW}"));
                format!("COALESCE({qc}, __vda_p{i}.v)")
            }
            Fill::Previous { order_by } | Fill::Next { order_by } => {
                let mut keys: Vec<String> = order_by
                    .iter()
                    .map(|k: &SortKey| format!("m.{} {} NULLS LAST", q(&k.column), if k.descending { "DESC" } else { "ASC" }))
                    .collect();
                keys.push(format!("m.{ROW}"));
                let (f, frame) = if matches!(c.fill, Fill::Previous { .. }) {
                    ("last_value", "ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW")
                } else {
                    ("first_value", "ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING")
                };
                format!("COALESCE({qc}, {f}({qc} IGNORE NULLS) OVER ({part} ORDER BY {} {frame}))", keys.join(", "))
            }
            Fill::Linear { intercept, terms } => {
                let mut sum = float_sql(*intercept);
                for (x, b) in terms {
                    sum = format!("({sum} + {} * CAST(m.{} AS DOUBLE))", float_sql(*b), q(x));
                }
                format!("COALESCE({qc}, {})", fit(c, sum))
            }
            Fill::Rows { values } => {
                if values.is_empty() {
                    qc.clone()
                } else {
                    let rows: Vec<String> = values.iter().map(|(r, v)| format!("({r}, {})", sql_value(v))).collect();
                    ctes.push(format!(
                        "__vda_n{i} AS (SELECT CAST(r AS BIGINT) AS {ROW}, CAST(v AS {}) AS v FROM (VALUES {}) AS t(r, v))",
                        sql_type(c.dtype),
                        rows.join(", ")
                    ));
                    joins.push(format!("LEFT JOIN __vda_n{i} ON __vda_n{i}.{ROW} = m.{ROW}"));
                    format!("COALESCE({qc}, __vda_n{i}.v)")
                }
            }
            Fill::Regression { .. } | Fill::Neighbours { .. } => unreachable!("resolved above"),
        };
        filled.insert(c.pos, e);
    }
    let mut sel: Vec<String> = p
        .marked
        .fields
        .iter()
        .enumerate()
        .map(|(pos, f)| match filled.get(&pos) {
            Some(e) => format!("{e} AS {}", q(&f.name)),
            None => format!("m.{}", q(&f.name)),
        })
        .collect();
    if p.flag {
        for c in &p.cols {
            sel.push(format!("(m.{} > 0) AS {}", why_column(c.index), q(&format!("{}_missing", c.name))));
        }
    }
    sel.push(format!("m.{ROW}"));
    for c in &p.cols {
        sel.push(format!("m.{}", why_column(c.index)));
    }
    let drops: Vec<String> = p.cols.iter().filter(|c| c.fill == Fill::DropRows).map(|c| format!("m.{} IS NULL", q(&c.name))).collect();
    sel.push(format!("({}) AS {DROP}", if drops.is_empty() { "FALSE".into() } else { drops.join(" OR ") }));
    ctes.push(format!("__vda_f AS (SELECT {} FROM __vda_m AS m {})", sel.join(", "), joins.join(" ")));
    let outs: Vec<String> = p.out.fields.iter().map(|f| q(&f.name)).collect();
    Ok(format!(
        "WITH {} SELECT {} FROM __vda_f{} ORDER BY {ROW}",
        ctes.join(",\n"),
        outs.join(", "),
        if p.internal { "" } else { &" WHERE NOT __vda_drop" }
    ))
}

// ---------------------------------------------------- reference (Rust)

/// A group's key: the group columns' values, as text that tells types apart.
fn key(row: &[Value], idx: &[usize]) -> String {
    idx.iter().map(|&i| format!("{:?}", row[i])).collect::<Vec<_>>().join("\u{1}")
}

fn fit_value(dtype: DataType, x: f64) -> Value {
    if dtype == DataType::Int { Value::Int(x.round() as i64) } else { Value::Float(x) }
}

/// The middle of sorted numbers, interpolated as DuckDB's `median` does.
pub fn median(sorted: &[f64]) -> f64 {
    let pos = (sorted.len() - 1) as f64 * 0.5;
    let (lo, hi) = (sorted[pos.floor() as usize], sorted[pos.ceil() as usize]);
    lo + (hi - lo) * (pos - pos.floor())
}

/// The most common value; ties go to the smallest.
pub fn mode(values: &[&Value]) -> Option<Value> {
    let mut counts: Vec<(&Value, usize)> = Vec::new();
    for v in values {
        match counts.iter_mut().find(|(w, _)| w.sql_cmp(v) == Some(Ordering::Equal)) {
            Some(e) => e.1 += 1,
            None => counts.push((v, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.sort_cmp(b.0)));
    counts.first().map(|(v, _)| (*v).clone())
}

/// The mean of a column's known numbers, as DuckDB's `avg` computes it.
pub fn mean(values: &[&Value]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    if values.iter().all(|v| matches!(v, Value::Int(_))) {
        let s: i128 = values.iter().map(|v| if let Value::Int(i) = v { *i as i128 } else { 0 }).sum();
        return Some(s as f64 / values.len() as f64);
    }
    Some(values.iter().filter_map(|v| v.as_f64()).sum::<f64>() / values.len() as f64)
}

/// The reference implementation: the step over a table in memory.
pub fn run(step: &Step, input: &Table) -> Result<Table, Error> {
    let p = plan(step, &input.schema)?;
    let n = input.rows.len();
    // Find.
    let mut rows: Vec<Vec<Value>> = input.rows.clone();
    let mut whys: Vec<Vec<i64>> = vec![Vec::with_capacity(n); p.cols.len()];
    for (r, row) in input.rows.iter().enumerate() {
        for (ci, c) in p.cols.iter().enumerate() {
            let w = formula::eval(&c.why, &input.schema, row)?;
            whys[ci].push(if let Value::Int(i) = w { i } else { 0 });
            rows[r][c.pos] = formula::eval(&c.value, &input.schema, row)?;
        }
    }
    let marked = rows.clone();
    let mi = |name: &str| p.marked.index_of(name).expect("validated");
    // Fill, from the values found.
    for c in &p.cols {
        let gidx: Vec<usize> = c.group.iter().map(|g| mi(g)).collect();
        let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for (r, row) in marked.iter().enumerate() {
            let k = key(row, &gidx);
            groups.entry(k.clone()).or_insert_with(|| {
                order.push(k.clone());
                Vec::new()
            });
            groups.get_mut(&k).unwrap().push(r);
        }
        let known = |rs: &[usize]| -> Vec<usize> { rs.iter().copied().filter(|&r| !marked[r][c.pos].is_null()).collect() };
        let mut set = |r: usize, v: Value| {
            if rows[r][c.pos].is_null() {
                rows[r][c.pos] = v;
            }
        };
        for g in &order {
            let rs = &groups[g];
            let kn = known(rs);
            let vals: Vec<&Value> = kn.iter().map(|&r| &marked[r][c.pos]).collect();
            let missing: Vec<usize> = rs.iter().copied().filter(|&r| marked[r][c.pos].is_null()).collect();
            if missing.is_empty() {
                continue;
            }
            match &c.fill {
                Fill::Keep | Fill::DropRows => {}
                Fill::Mean => {
                    if let Some(m) = mean(&vals) {
                        missing.iter().for_each(|&r| set(r, fit_value(c.dtype, m)));
                    }
                }
                Fill::Median => {
                    let mut xs: Vec<f64> = vals.iter().filter_map(|v| v.as_f64()).collect();
                    if !xs.is_empty() {
                        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
                        let m = median(&xs);
                        missing.iter().for_each(|&r| set(r, fit_value(c.dtype, m)));
                    }
                }
                Fill::Mode => {
                    if let Some(m) = mode(&vals) {
                        missing.iter().for_each(|&r| set(r, m.clone()));
                    }
                }
                Fill::Constant { value } => {
                    let v = parse_constant(c.dtype, value).expect("validated");
                    missing.iter().for_each(|&r| set(r, v.clone()));
                }
                Fill::Random => {
                    if !vals.is_empty() {
                        let salt = draw_salt(p.seed, c.index);
                        for &r in &missing {
                            let k = draw(r as u64 + 1, salt) % vals.len() as u64;
                            set(r, vals[k as usize].clone());
                        }
                    }
                }
                Fill::Previous { order_by } | Fill::Next { order_by } => {
                    let keys: Vec<(usize, bool)> = order_by.iter().map(|k| (mi(&k.column), k.descending)).collect();
                    let mut sorted = rs.clone();
                    sorted.sort_by(|&a, &b| {
                        for &(i, desc) in &keys {
                            let (x, y) = (&marked[a][i], &marked[b][i]);
                            let o = match (x.is_null(), y.is_null()) {
                                (true, true) => Ordering::Equal,
                                (true, false) => Ordering::Greater,
                                (false, true) => Ordering::Less,
                                _ => {
                                    let o = x.sql_cmp(y).unwrap_or(Ordering::Equal);
                                    if desc { o.reverse() } else { o }
                                }
                            };
                            if o != Ordering::Equal {
                                return o;
                            }
                        }
                        a.cmp(&b)
                    });
                    if matches!(c.fill, Fill::Next { .. }) {
                        sorted.reverse();
                    }
                    let mut last: Option<Value> = None;
                    for r in sorted {
                        let v = &marked[r][c.pos];
                        if v.is_null() {
                            if let Some(l) = &last {
                                set(r, l.clone());
                            }
                        } else {
                            last = Some(v.clone());
                        }
                    }
                }
                Fill::Linear { intercept, terms } => {
                    let idx: Vec<(usize, f64)> = terms.iter().map(|(x, b)| (mi(x), *b)).collect();
                    for &r in &missing {
                        let mut sum = *intercept;
                        let mut ok = true;
                        for &(i, b) in &idx {
                            match marked[r][i].as_f64() {
                                Some(x) => sum += b * x,
                                None => ok = false,
                            }
                        }
                        if ok {
                            set(r, fit_value(c.dtype, sum));
                        }
                    }
                }
                Fill::Rows { values } => {
                    let map: HashMap<u64, &Value> = values.iter().map(|(r, v)| (*r, v)).collect();
                    for &r in &missing {
                        if let Some(v) = map.get(&(r as u64 + 1)) {
                            set(r, (*v).clone());
                        }
                    }
                }
                Fill::Regression { .. } | Fill::Neighbours { .. } => {
                    return Err(Error::Unsupported(format!("step `{}`: `{}` must be fitted before it runs", step.id, c.name)))
                }
            }
        }
    }
    // Assemble.
    let mut out = Vec::with_capacity(n);
    for (r, mut row) in rows.into_iter().enumerate() {
        let drop = p.cols.iter().any(|c| c.fill == Fill::DropRows && row[c.pos].is_null());
        if drop && !p.internal {
            continue;
        }
        if p.flag {
            for (ci, _) in p.cols.iter().enumerate() {
                row.push(Value::Bool(whys[ci][r] > 0));
            }
        }
        if p.internal {
            row.push(Value::Int(r as i64 + 1));
            for w in &whys {
                row.push(Value::Int(w[r]));
            }
            row.push(Value::Bool(drop));
        }
        out.push(row);
    }
    Ok(Table { schema: p.out, rows: out })
}

// ------------------------------------------------------------- fitting

/// A fitted straight line: intercept and one slope per predictor.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub intercept: f64,
    pub slopes: Vec<f64>,
    /// Rows it was fitted on.
    pub rows: usize,
    /// Share of the variation it explains (R²) on those rows.
    pub r2: f64,
}

/// Least squares with an intercept on rows of `[x1, ..., xp, y]` (all known).
/// Predictors are standardized while solving, for accuracy.
pub fn fit_regression(names: &[String], rows: &[Vec<f64>]) -> Result<Line, String> {
    let p = names.len();
    if rows.len() < p + 2 {
        return Err(format!("only {} rows have every value known; a regression on {p} column{} needs at least {}", rows.len(), if p == 1 { "" } else { "s" }, p + 2));
    }
    let n = rows.len() as f64;
    let means: Vec<f64> = (0..=p).map(|j| rows.iter().map(|r| r[j]).sum::<f64>() / n).collect();
    let sds: Vec<f64> = (0..p).map(|j| (rows.iter().map(|r| (r[j] - means[j]).powi(2)).sum::<f64>() / n).sqrt()).collect();
    if let Some(j) = sds.iter().position(|s| *s == 0.0 || !s.is_finite()) {
        return Err(format!("`{}` has the same value in every row, so it can't predict anything", names[j]));
    }
    // Normal equations on standardized predictors and a centred target.
    let mut a = vec![vec![0.0; p + 1]; p];
    for r in rows {
        let z: Vec<f64> = (0..p).map(|j| (r[j] - means[j]) / sds[j]).collect();
        let y = r[p] - means[p];
        for i in 0..p {
            for j in 0..p {
                a[i][j] += z[i] * z[j];
            }
            a[i][p] += z[i] * y;
        }
    }
    for i in 0..p {
        let piv = (i..p).max_by(|&x, &y| a[x][i].abs().partial_cmp(&a[y][i].abs()).unwrap_or(Ordering::Equal)).unwrap();
        if a[piv][i].abs() < 1e-9 * n {
            return Err(format!("`{}` moves together with the other predictors; leave one of them out", names[i]));
        }
        a.swap(i, piv);
        let pivot = a[i].clone();
        for (k, row) in a.iter_mut().enumerate() {
            if k != i {
                let f = row[i] / pivot[i];
                for (x, y) in row[i..].iter_mut().zip(&pivot[i..]) {
                    *x -= f * y;
                }
            }
        }
    }
    let beta: Vec<f64> = (0..p).map(|i| a[i][p] / a[i][i]).collect();
    let slopes: Vec<f64> = (0..p).map(|j| beta[j] / sds[j]).collect();
    let intercept = means[p] - (0..p).map(|j| slopes[j] * means[j]).sum::<f64>();
    let (mut ss_res, mut ss_tot) = (0.0, 0.0);
    for r in rows {
        let pred = intercept + (0..p).map(|j| slopes[j] * r[j]).sum::<f64>();
        ss_res += (r[p] - pred).powi(2);
        ss_tot += (r[p] - means[p]).powi(2);
    }
    let r2 = if ss_tot > 0.0 { 1.0 - ss_res / ss_tot } else { 1.0 };
    Ok(Line { intercept, slopes, rows: rows.len(), r2 })
}

/// Nearest neighbours: for each query (predictor values), the `k` closest
/// training rows by standardized distance (ties to the earlier row) give
/// the value: their mean for numbers, else their most common value (ties
/// to the nearest).
pub fn neighbours(train_x: &[Vec<f64>], train_y: &[Value], queries: &[Vec<f64>], k: usize, dtype: DataType) -> Vec<Option<Value>> {
    if train_x.is_empty() {
        return vec![None; queries.len()];
    }
    let p = train_x[0].len();
    let n = train_x.len() as f64;
    let means: Vec<f64> = (0..p).map(|j| train_x.iter().map(|r| r[j]).sum::<f64>() / n).collect();
    let sds: Vec<f64> = (0..p)
        .map(|j| {
            let s = (train_x.iter().map(|r| (r[j] - means[j]).powi(2)).sum::<f64>() / n).sqrt();
            if s > 0.0 && s.is_finite() { s } else { 1.0 }
        })
        .collect();
    let z = |r: &[f64]| -> Vec<f64> { (0..p).map(|j| (r[j] - means[j]) / sds[j]).collect() };
    let tz: Vec<Vec<f64>> = train_x.iter().map(|r| z(r)).collect();
    let k = k.min(tz.len());
    queries
        .iter()
        .map(|q| {
            let qz = z(q);
            let mut d: Vec<(f64, usize)> =
                tz.iter().enumerate().map(|(i, t)| (t.iter().zip(&qz).map(|(a, b)| (a - b).powi(2)).sum::<f64>(), i)).collect();
            d.select_nth_unstable_by(k - 1, |a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)));
            let mut near: Vec<(f64, usize)> = d[..k].to_vec();
            near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)));
            if numeric(dtype) {
                let m = near.iter().filter_map(|(_, i)| train_y[*i].as_f64()).sum::<f64>() / near.len() as f64;
                Some(fit_value(dtype, m))
            } else {
                // Most common, ties to the one met first (the nearest).
                let mut counts: Vec<(&Value, usize)> = Vec::new();
                for (_, i) in &near {
                    let v = &train_y[*i];
                    match counts.iter_mut().find(|(w, _)| *w == v) {
                        Some(e) => e.1 += 1,
                        None => counts.push((v, 1)),
                    }
                }
                let best = counts.iter().map(|c| c.1).max().unwrap_or(0);
                counts.into_iter().find(|c| c.1 == best).map(|c| c.0.clone())
            }
        })
        .collect()
}

// ------------------------------------------------------- in a session

use crate::dataset::Dataset;
use crate::engine::Engine;
use crate::model::{AggFunc, CmpOp, Measure, Pick, Predicate};
use std::path::{Path, PathBuf};

fn present(c: &str) -> Predicate {
    Predicate { column: c.into(), op: CmpOp::IsNotNull, value: Value::Null, param: None }
}
fn absent(c: &str) -> Predicate {
    Predicate { column: c.into(), op: CmpOp::IsNull, value: Value::Null, param: None }
}

/// Runs sub-steps of a Clean step through the engine, in scratch files it
/// removes afterwards.
struct Runner<'a> {
    engine: &'a mut dyn Engine,
    step: &'a Step,
    scratch: &'a mut dyn FnMut() -> PathBuf,
    made: Vec<PathBuf>,
}

impl Runner<'_> {
    fn run(&mut self, kind: StepKind, from: &Dataset) -> Result<Dataset, Error> {
        let out = (self.scratch)();
        self.made.push(out.clone());
        self.run_to(kind, from, &out)
    }
    fn run_to(&mut self, kind: StepKind, from: &Dataset, out: &Path) -> Result<Dataset, Error> {
        let s = Step { id: self.step.id.clone(), inputs: vec![self.step.input().to_string()], label: None, kind };
        self.engine.run_step(&s, std::slice::from_ref(from), out)
    }
    /// Rows of `columns` where every one is known (`where_missing`: and that
    /// column is missing), at most `cap`, spread evenly through the data.
    fn read(&mut self, from: &Dataset, columns: &[String], known: &[String], where_missing: Option<&str>, cap: u64) -> Result<(Table, u64), Error> {
        let mut preds: Vec<Predicate> = known.iter().map(|c| present(c)).collect();
        if let Some(m) = where_missing {
            preds.push(absent(m));
        }
        let rows = self.run(StepKind::Filter { predicates: preds }, from)?;
        let picked = self.run(StepKind::Select { columns: columns.iter().map(|c| Pick { column: c.clone(), rename: None }).collect() }, &rows)?;
        let total = picked.rows;
        if total <= cap {
            return Ok((self.engine.preview(&picked, 0, total)?, total));
        }
        let chunks = 50u64;
        let each = cap / chunks;
        let mut t = self.engine.preview(&picked, 0, each)?;
        for c in 1..chunks {
            t.rows.extend(self.engine.preview(&picked, c * total / chunks, each)?.rows);
        }
        Ok((t, total))
    }
}

fn floats(t: &Table) -> Vec<Vec<f64>> {
    t.rows.iter().map(|r| r.iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect()).collect()
}

fn number(x: f64) -> String {
    let r = (x * 10_000.0).round() / 10_000.0;
    if r == r.trunc() && r.abs() < 1e15 { format!("{}", r as i64) } else { format!("{r}") }
}

/// Fit regressions and neighbour searches to the data, giving a step whose
/// fills both engines can apply; and say how each was fitted.
pub fn resolve(engine: &mut dyn Engine, step: &Step, input: &Dataset, scratch: &mut dyn FnMut() -> PathBuf) -> Result<(Step, HashMap<usize, String>), Error> {
    let StepKind::Clean { markers, columns, flag, seed, .. } = &step.kind else { unreachable!("not a clean step") };
    let mut notes = HashMap::new();
    if !columns.iter().any(|c| c.fill.is_fitted()) {
        return Ok((step.clone(), notes));
    }
    let mut r = Runner { engine, step, scratch, made: vec![] };
    let result = (|| {
        // The values found, before any fill.
        let found: Vec<_> = columns
            .iter()
            .map(|c| {
                let mut c = c.clone();
                if c.fill.is_fitted() {
                    c.fill = Fill::Keep;
                }
                c
            })
            .collect();
        let marked = r.run(StepKind::Clean { markers: markers.clone(), columns: found, flag: false, seed: *seed, internal: true }, input)?;
        let mut resolved = columns.clone();
        for (i, c) in columns.iter().enumerate() {
            let dtype = marked.schema.fields[marked.schema.index_of(&c.column).expect("validated")].dtype;
            match &c.fill {
                Fill::Regression { predictors } => {
                    let mut cols = predictors.clone();
                    cols.push(c.column.clone());
                    let (t, total) = r.read(&marked, &cols, &cols, None, FIT_ROWS)?;
                    let line = fit_regression(predictors, &floats(&t)).map_err(|m| err(step, format!("`{}`: {m}", c.column)))?;
                    let terms: Vec<(String, f64)> = predictors.iter().cloned().zip(line.slopes.iter().copied()).collect();
                    let eq = terms.iter().fold(number(line.intercept), |acc, (x, b)| {
                        format!("{acc} {} {} × {x}", if *b < 0.0 { "−" } else { "+" }, number(b.abs()))
                    });
                    notes.insert(
                        i,
                        format!(
                            "a line fitted on {} rows{} (R² {:.2}): {} = {eq}",
                            line.rows,
                            if total > line.rows as u64 { format!(" of {total}") } else { String::new() },
                            line.r2,
                            c.column
                        ),
                    );
                    resolved[i].fill = Fill::Linear { intercept: line.intercept, terms };
                }
                Fill::Neighbours { predictors, k } => {
                    let mut cols = predictors.clone();
                    cols.push(c.column.clone());
                    let (train, total) = r.read(&marked, &cols, &cols, None, NEIGHBOUR_ROWS)?;
                    let mut qcols = vec![ROW.to_string()];
                    qcols.extend(predictors.iter().cloned());
                    let (queries, wanted) = r.read(&marked, &qcols, predictors, Some(&c.column), MAX_NEIGHBOUR_FILLS)?;
                    if wanted > MAX_NEIGHBOUR_FILLS {
                        return Err(err(
                            step,
                            format!(
                                "`{}` has {wanted} missing values to fill; nearest neighbours fills at most {MAX_NEIGHBOUR_FILLS}. Use a regression, the median or a random draw",
                                c.column
                            ),
                        ));
                    }
                    if train.rows.is_empty() {
                        return Err(err(step, format!("`{}`: no rows have it and every predictor known, so there are no neighbours", c.column)));
                    }
                    let tx: Vec<Vec<f64>> = floats(&train).into_iter().map(|mut r| {
                        r.pop();
                        r
                    }).collect();
                    let ty: Vec<Value> = train.rows.iter().map(|r| r[predictors.len()].clone()).collect();
                    let qx: Vec<Vec<f64>> = floats(&queries).into_iter().map(|r| r[1..].to_vec()).collect();
                    let got = neighbours(&tx, &ty, &qx, *k as usize, dtype);
                    let values: Vec<(u64, Value)> = queries
                        .rows
                        .iter()
                        .zip(got)
                        .filter_map(|(q, v)| Some((q[0].as_f64()? as u64, v?)))
                        .collect();
                    notes.insert(
                        i,
                        format!(
                            "the {} of the {k} most similar rows by {}{}",
                            if numeric(dtype) { "average" } else { "most common value" },
                            predictors.join(", "),
                            if total > train.rows.len() as u64 { format!(" (compared with {} of {total} rows)", train.rows.len()) } else { String::new() }
                        ),
                    );
                    resolved[i].fill = Fill::Rows { values };
                }
                _ => {}
            }
        }
        Ok(Step {
            kind: StepKind::Clean { markers: markers.clone(), columns: resolved, flag: *flag, seed: *seed, internal: false },
            ..step.clone()
        })
    })();
    for f in r.made {
        let _ = std::fs::remove_file(f);
    }
    Ok((result?, notes))
}

/// Run a Clean step: fit, fill, and count what was found and filled.
/// Writes the result at `out`.
pub fn evaluate(
    engine: &mut dyn Engine,
    step: &Step,
    input: &Dataset,
    scratch: &mut dyn FnMut() -> PathBuf,
    out: &Path,
) -> Result<(Dataset, CleanReport), Error> {
    let p = plan(step, &input.schema)?;
    let (resolved, notes) = resolve(engine, step, input, scratch)?;
    let StepKind::Clean { markers, columns, flag, seed, .. } = &resolved.kind else { unreachable!() };
    let mut r = Runner { engine, step, scratch, made: vec![] };
    let result = (|| {
        let all = r.run(StepKind::Clean { markers: markers.clone(), columns: columns.clone(), flag: *flag, seed: *seed, internal: true }, input)?;
        let any_drop = p.cols.iter().any(|c| c.fill == Fill::DropRows);
        let kept = if any_drop {
            r.run(
                StepKind::Filter { predicates: vec![Predicate { column: DROP.into(), op: CmpOp::Eq, value: Value::Bool(false), param: None }] },
                &all,
            )?
        } else {
            all.clone()
        };
        let mut report = CleanReport { rows: input.rows, dropped: all.rows - kept.rows, columns: vec![] };
        for c in &p.cols {
            let w = why_column(c.index);
            let count = |r: &mut Runner, from: &Dataset| -> Result<Vec<(i64, u64, u64)>, Error> {
                let t = r.run(
                    StepKind::Aggregate {
                        group_by: vec![w.clone()],
                        measures: vec![
                            Measure { func: AggFunc::Count, column: None, alias: "__vda_n".into() },
                            Measure { func: AggFunc::Count, column: Some(c.name.clone()), alias: "__vda_known".into() },
                        ],
                    },
                    from,
                )?;
                let t = r.engine.preview(&t, 0, 10)?;
                Ok(t.rows
                    .iter()
                    .map(|row| {
                        let n = |v: &Value| v.as_f64().unwrap_or(0.0) as u64;
                        (row[0].as_f64().unwrap_or(0.0) as i64, n(&row[1]), n(&row[2]))
                    })
                    .collect())
            };
            let mut rep = ColumnReport { column: c.name.clone(), method: c.fill.name().into(), ..Default::default() };
            for (why, n, _) in count(&mut r, &all)? {
                match why {
                    1 => rep.empty = n,
                    2 => rep.markers = n,
                    3 => rep.condition = n,
                    4 => rep.conversion = n,
                    _ => {}
                }
            }
            for (why, n, known) in count(&mut r, &kept)? {
                if why > 0 {
                    rep.filled += known;
                    rep.left += n - known;
                }
            }
            let per = if c.group.is_empty() { String::new() } else { format!(" per {}", c.group.join(" and ")) };
            let value_of = |r: &mut Runner| -> Result<Option<Value>, Error> {
                let rows = r.run(
                    StepKind::Filter {
                        predicates: vec![Predicate { column: w.clone(), op: CmpOp::Gt, value: Value::Int(0), param: None }, present(&c.name)],
                    },
                    &kept,
                )?;
                if rows.rows == 0 {
                    return Ok(None);
                }
                let t = r.engine.preview(&rows, 0, 1)?;
                Ok(t.schema.index_of(&c.name).map(|i| t.rows[0][i].clone()))
            };
            let shown = |v: Option<Value>| match v {
                Some(Value::Float(x)) => format!(", {}", number(x)),
                Some(Value::Text(t)) => format!(", \"{t}\""),
                Some(v) => format!(", {v}"),
                None => String::new(),
            };
            rep.detail = match &c.fill {
                Fill::Keep => "nothing (left missing)".into(),
                Fill::DropRows => "nothing (rows dropped)".into(),
                Fill::Mean | Fill::Median | Fill::Mode => {
                    let what = match c.fill {
                        Fill::Mean => "the mean",
                        Fill::Median => "the median",
                        _ => "the most common value",
                    };
                    if per.is_empty() && rep.filled > 0 { format!("{what}{}", shown(value_of(&mut r)?)) } else { format!("{what}{per}") }
                }
                Fill::Constant { value } => format!("\"{value}\""),
                Fill::Random => format!("a random known value{per} (seed {seed})"),
                Fill::Previous { order_by } | Fill::Next { order_by } => format!(
                    "the {} known value{per}{}",
                    if matches!(c.fill, Fill::Previous { .. }) { "previous" } else { "next" },
                    if order_by.is_empty() {
                        " (in row order)".to_string()
                    } else {
                        format!(" by {}", order_by.iter().map(|k| if k.descending { format!("{} (descending)", k.column) } else { k.column.clone() }).collect::<Vec<_>>().join(", "))
                    }
                ),
                _ => notes.get(&c.index).cloned().unwrap_or_default(),
            };
            report.columns.push(rep);
        }
        let keep: Vec<Pick> = p
            .out
            .fields
            .iter()
            .filter(|f| !f.name.starts_with("__vda_"))
            .map(|f| Pick { column: f.name.clone(), rename: None })
            .collect();
        let ds = r.run_to(StepKind::Select { columns: keep }, &kept, out)?;
        Ok((ds, report))
    })();
    for f in r.made {
        let _ = std::fs::remove_file(f);
    }
    result
}

// ------------------------------------------------- comparing methods

/// Rows the comparison reads (the first ones, so neighbouring rows stay
/// neighbours for previous and next values).
pub const COMPARE_ROWS: u64 = 20_000;
/// Share of known values hidden, at most this many.
pub const HIDE_SHARE: f64 = 0.10;
pub const MAX_HIDDEN: usize = 2_000;

/// How one method did on the hidden values.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodScore {
    /// The fill as saved (`{"method": "median"}`), to use it.
    pub fill: Json,
    pub label: String,
    /// Hidden values it filled (some methods can't fill every one).
    pub filled: usize,
    /// Numbers: the average and root-mean-square distance from the truth.
    pub mae: Option<f64>,
    pub rmse: Option<f64>,
    /// Other types: the share filled exactly right.
    pub accuracy: Option<f64>,
    /// How much of the hidden values' spread the fills keep: their standard
    /// deviation over the truth's (numbers), or distinct values over the
    /// truth's (other types). A fill with one value keeps none.
    pub spread: Option<f64>,
    /// Why it couldn't be tried.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub column: String,
    /// Rows read, known values among them, and how many were hidden.
    pub rows: usize,
    pub known: usize,
    pub hidden: usize,
    /// Predictors used for the regression and neighbours.
    pub predictors: Vec<String>,
    pub methods: Vec<MethodScore>,
}

impl Comparison {
    pub fn to_json(&self) -> Json {
        let opt = |x: Option<f64>| x.map(Json::Num).unwrap_or(Json::Null);
        Json::obj(vec![
            ("column", Json::str(self.column.clone())),
            ("rows", Json::Num(self.rows as f64)),
            ("known", Json::Num(self.known as f64)),
            ("hidden", Json::Num(self.hidden as f64)),
            ("predictors", Json::Arr(self.predictors.iter().map(|p| Json::str(p.clone())).collect())),
            (
                "methods",
                Json::Arr(
                    self.methods
                        .iter()
                        .map(|m| {
                            Json::obj(vec![
                                ("fill", m.fill.clone()),
                                ("label", Json::str(m.label.clone())),
                                ("filled", Json::Num(m.filled as f64)),
                                ("mae", opt(m.mae)),
                                ("rmse", opt(m.rmse)),
                                ("accuracy", opt(m.accuracy)),
                                ("spread", opt(m.spread)),
                                ("problem", m.problem.clone().map(Json::Str).unwrap_or(Json::Null)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

fn is_id(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "id" || n.ends_with("_id")
}

fn sd(xs: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let m = xs.iter().sum::<f64>() / n;
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / n).sqrt()
}

/// Hide some known values of `column` (as the step finds them), fill them
/// with every method that suits its type, and score each against the truth.
pub fn compare(engine: &mut dyn Engine, step: &Step, input: &Dataset, column: &str, scratch: &mut dyn FnMut() -> PathBuf) -> Result<Comparison, Error> {
    let StepKind::Clean { markers, columns, seed, .. } = &step.kind else { unreachable!("not a clean step") };
    let rule = columns.iter().find(|c| c.column == column).ok_or_else(|| err(step, format!("`{column}` isn't one of the columns this step cleans")))?;
    let index = columns.iter().position(|c| c.column == column).unwrap();
    // The values as found, with nothing filled.
    let found: Vec<_> = columns.iter().map(|c| crate::model::CleanColumn { fill: Fill::Keep, ..c.clone() }).collect();
    let probe = Step { kind: StepKind::Clean { markers: markers.clone(), columns: found, flag: false, seed: *seed, internal: true }, ..step.clone() };
    let plan_found = plan(&probe, &input.schema)?;
    let marked_schema = plan_found.marked.clone();
    let dtype = marked_schema.fields[marked_schema.index_of(column).unwrap()].dtype;
    let order: Vec<SortKey> = match &rule.fill {
        Fill::Previous { order_by } | Fill::Next { order_by } => order_by.clone(),
        _ => vec![],
    };
    let predictors: Vec<String> = match &rule.fill {
        Fill::Regression { predictors } | Fill::Neighbours { predictors, .. } => predictors.clone(),
        _ => marked_schema
            .fields
            .iter()
            // Measures, not identifiers.
            .filter(|f| f.name != column && numeric(f.dtype) && !rule.group_by.contains(&f.name) && !is_id(&f.name))
            .map(|f| f.name.clone())
            .take(5)
            .collect(),
    };
    let mut wanted: Vec<String> = vec![ROW.to_string(), column.to_string()];
    for c in rule.group_by.iter().chain(order.iter().map(|k| &k.column)).chain(predictors.iter()) {
        if !wanted.contains(c) {
            wanted.push(c.clone());
        }
    }
    let mut r = Runner { engine, step, scratch, made: vec![] };
    let sample = (|| {
        let marked = r.run(probe.kind.clone(), input)?;
        let picked = r.run(StepKind::Select { columns: wanted.iter().map(|c| Pick { column: c.clone(), rename: None }).collect() }, &marked)?;
        r.engine.preview(&picked, 0, COMPARE_ROWS)
    })();
    for f in r.made {
        let _ = std::fs::remove_file(f);
    }
    let mut sample = sample?;
    // Without the row number: the reference implementation numbers rows itself.
    for row in &mut sample.rows {
        row.remove(0);
    }
    sample.schema.fields.remove(0);
    let ti = 0;
    let known: Vec<usize> = (0..sample.rows.len()).filter(|&i| !sample.rows[i][ti].is_null()).collect();
    if known.len() < 10 {
        return Err(err(step, format!("`{column}` has only {} known values in the first {} rows; there's too little to test on", known.len(), sample.rows.len())));
    }
    let salt = draw_salt(*seed, 1000 + index);
    let mut shuffled = known.clone();
    shuffled.sort_by_key(|&i| (draw(i as u64 + 1, salt), i));
    let hide = ((known.len() as f64 * HIDE_SHARE).ceil() as usize).clamp(1, MAX_HIDDEN);
    let mut hidden: Vec<usize> = shuffled[..hide].to_vec();
    hidden.sort();
    let truth: Vec<Value> = hidden.iter().map(|&i| sample.rows[i][ti].clone()).collect();
    let mut test = sample.clone();
    for &i in &hidden {
        test.rows[i][ti] = Value::Null;
    }

    let per = if rule.group_by.is_empty() { String::new() } else { format!(" per {}", rule.group_by.join(", ")) };
    let by = if order.is_empty() { " (row order)".to_string() } else { format!(" by {}", order.iter().map(|k| k.column.clone()).collect::<Vec<_>>().join(", ")) };
    let mut tries: Vec<(Fill, String)> = Vec::new();
    if numeric(dtype) {
        tries.push((Fill::Mean, format!("Mean{per}")));
        tries.push((Fill::Median, format!("Median{per}")));
    }
    tries.push((Fill::Mode, format!("Most common{per}")));
    tries.push((Fill::Random, format!("Random draw{per}")));
    tries.push((Fill::Previous { order_by: order.clone() }, format!("Previous value{by}")));
    tries.push((Fill::Next { order_by: order.clone() }, format!("Next value{by}")));
    if numeric(dtype) {
        tries.push((Fill::Regression { predictors: predictors.clone() }, "Regression".to_string()));
    }
    let k = match &rule.fill {
        Fill::Neighbours { k, .. } => *k,
        _ => 5,
    };
    tries.push((Fill::Neighbours { predictors: predictors.clone(), k }, format!("{k} nearest neighbours")));

    let mut methods = Vec::new();
    for (fill, label) in tries {
        let saved = crate::persist::fill_to_json(&fill);
        let attempt = (|| -> Result<Table, String> {
            let fill = match &fill {
                Fill::Regression { predictors } => {
                    if predictors.is_empty() {
                        return Err("no number columns to predict from".into());
                    }
                    let pi: Vec<usize> = predictors.iter().map(|p| test.schema.index_of(p).unwrap()).collect();
                    let rows: Vec<Vec<f64>> = test
                        .rows
                        .iter()
                        .filter_map(|row| {
                            let mut v: Vec<f64> = pi.iter().map(|&i| row[i].as_f64()).collect::<Option<_>>()?;
                            v.push(row[ti].as_f64()?);
                            Some(v)
                        })
                        .collect();
                    let line = fit_regression(predictors, &rows)?;
                    Fill::Linear { intercept: line.intercept, terms: predictors.iter().cloned().zip(line.slopes).collect() }
                }
                Fill::Neighbours { predictors, k } => {
                    if predictors.is_empty() {
                        return Err("no number columns to compare rows by".into());
                    }
                    let pi: Vec<usize> = predictors.iter().map(|p| test.schema.index_of(p).unwrap()).collect();
                    let xs = |row: &[Value]| -> Option<Vec<f64>> { pi.iter().map(|&i| row[i].as_f64()).collect() };
                    let mut tx = vec![];
                    let mut ty = vec![];
                    for row in &test.rows {
                        if let (Some(x), false) = (xs(row), row[ti].is_null()) {
                            if tx.len() < NEIGHBOUR_ROWS as usize {
                                tx.push(x);
                                ty.push(row[ti].clone());
                            }
                        }
                    }
                    let q: Vec<(usize, Vec<f64>)> = hidden.iter().filter_map(|&i| Some((i, xs(&test.rows[i])?))).collect();
                    let qx: Vec<Vec<f64>> = q.iter().map(|(_, x)| x.clone()).collect();
                    let got = neighbours(&tx, &ty, &qx, *k as usize, dtype);
                    Fill::Rows { values: q.iter().zip(got).filter_map(|((i, _), v)| Some((*i as u64 + 1, v?))).collect() }
                }
                f => f.clone(),
            };
            let col = crate::model::CleanColumn { column: column.to_string(), as_type: None, missing_when: None, fill, group_by: rule.group_by.clone() };
            let s = Step { kind: StepKind::Clean { markers: vec![], columns: vec![col], flag: false, seed: *seed, internal: false }, ..step.clone() };
            run(&s, &test).map_err(|e| e.to_string())
        })();
        let mut score = MethodScore { fill: saved, label, filled: 0, mae: None, rmse: None, accuracy: None, spread: None, problem: None };
        match attempt {
            Err(m) => score.problem = Some(m),
            Ok(out) => {
                let got: Vec<(Value, Value)> = hidden
                    .iter()
                    .zip(&truth)
                    .filter_map(|(&i, t)| {
                        let v = out.rows[i][ti].clone();
                        (!v.is_null()).then(|| (v, t.clone()))
                    })
                    .collect();
                score.filled = got.len();
                if !got.is_empty() {
                    if numeric(dtype) {
                        let d: Vec<f64> = got.iter().map(|(v, t)| v.as_f64().unwrap() - t.as_f64().unwrap()).collect();
                        score.mae = Some(d.iter().map(|x| x.abs()).sum::<f64>() / d.len() as f64);
                        score.rmse = Some((d.iter().map(|x| x * x).sum::<f64>() / d.len() as f64).sqrt());
                        let fv: Vec<f64> = got.iter().map(|(v, _)| v.as_f64().unwrap()).collect();
                        let tv: Vec<f64> = got.iter().map(|(_, t)| t.as_f64().unwrap()).collect();
                        let st = sd(&tv);
                        score.spread = (st > 0.0).then(|| sd(&fv) / st);
                    } else {
                        score.accuracy = Some(got.iter().filter(|(v, t)| v == t).count() as f64 / got.len() as f64);
                        let distinct = |vs: Vec<&Value>| {
                            let mut seen: Vec<&Value> = vec![];
                            for v in vs {
                                if !seen.contains(&v) {
                                    seen.push(v);
                                }
                            }
                            seen.len() as f64
                        };
                        let dt = distinct(got.iter().map(|(_, t)| t).collect());
                        score.spread = Some(distinct(got.iter().map(|(v, _)| v).collect()) / dt);
                    }
                }
            }
        }
        methods.push(score);
    }
    Ok(Comparison { column: column.to_string(), rows: sample.rows.len(), known: known.len(), hidden: hidden.len(), predictors, methods })
}

// -------------------------------------------------------------- report

/// What the step found and filled in one column.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ColumnReport {
    pub column: String,
    /// Missing values by reason: empty, marker, condition, conversion.
    pub empty: u64,
    pub markers: u64,
    pub condition: u64,
    pub conversion: u64,
    pub filled: u64,
    /// Still missing afterwards (in rows kept).
    pub left: u64,
    pub method: String,
    /// How it was filled, in words ("the median, 4.5"; the fitted line).
    pub detail: String,
}

impl ColumnReport {
    pub fn found(&self) -> u64 {
        self.empty + self.markers + self.condition + self.conversion
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CleanReport {
    pub rows: u64,
    pub dropped: u64,
    pub columns: Vec<ColumnReport>,
}

impl CleanReport {
    /// For the console's Output tab.
    pub fn text(&self) -> String {
        let mut out = vec![];
        for c in &self.columns {
            let mut why = vec![];
            for (n, w) in [(c.empty, "empty"), (c.markers, "a marker"), (c.condition, "the condition"), (c.conversion, "didn't convert")] {
                if n > 0 {
                    why.push(format!("{n} {w}"));
                }
            }
            let found = if why.is_empty() { "nothing missing".to_string() } else { format!("{} missing ({})", c.found(), why.join(", ")) };
            let filled = if c.found() == 0 {
                String::new()
            } else if c.method == "drop" {
                "; those rows dropped".into()
            } else if c.method == "keep" {
                "; left as missing".into()
            } else {
                format!("; filled {} with {}{}", c.filled, c.detail, if c.left > 0 { format!(", {} still missing", c.left) } else { String::new() })
            };
            out.push(format!("{}: {found}{filled}", c.column));
        }
        if self.dropped > 0 {
            out.push(format!("{} of {} rows dropped", self.dropped, self.rows));
        }
        out.join("\n")
    }

    pub fn to_json(&self) -> Json {
        let n = |x: u64| Json::Num(x as f64);
        Json::obj(vec![
            ("kind", Json::str("clean")),
            ("rows", n(self.rows)),
            ("dropped", n(self.dropped)),
            (
                "columns",
                Json::Arr(
                    self.columns
                        .iter()
                        .map(|c| {
                            Json::obj(vec![
                                ("column", Json::str(c.column.clone())),
                                ("empty", n(c.empty)),
                                ("markers", n(c.markers)),
                                ("condition", n(c.condition)),
                                ("conversion", n(c.conversion)),
                                ("filled", n(c.filled)),
                                ("left", n(c.left)),
                                ("method", Json::str(c.method.clone())),
                                ("detail", Json::str(c.detail.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }

    pub fn from_json(j: &Json) -> Result<CleanReport, Error> {
        let n = |j: &Json, k: &str| j.get(k).and_then(Json::as_f64).unwrap_or(0.0) as u64;
        Ok(CleanReport {
            rows: n(j, "rows"),
            dropped: n(j, "dropped"),
            columns: j
                .req_arr("columns")?
                .iter()
                .map(|c| {
                    Ok(ColumnReport {
                        column: c.req_str("column")?.to_string(),
                        empty: n(c, "empty"),
                        markers: n(c, "markers"),
                        condition: n(c, "condition"),
                        conversion: n(c, "conversion"),
                        filled: n(c, "filled"),
                        left: n(c, "left"),
                        method: c.get("method").and_then(Json::as_str).unwrap_or("").to_string(),
                        detail: c.get("detail").and_then(Json::as_str).unwrap_or("").to_string(),
                    })
                })
                .collect::<Result<_, Error>>()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CleanColumn;

    fn table(cols: &[(&str, DataType)], rows: Vec<Vec<Value>>) -> Table {
        Table { schema: Schema { fields: cols.iter().map(|(n, t)| Field { name: n.to_string(), dtype: *t }).collect() }, rows }
    }
    fn step(markers: &[&str], columns: Vec<CleanColumn>) -> Step {
        Step {
            id: "clean".into(),
            inputs: vec!["source".into()],
            label: None,
            kind: StepKind::Clean { markers: markers.iter().map(|s| s.to_string()).collect(), columns, flag: true, seed: 1, internal: false },
        }
    }
    use Value::{Float as F, Int as I, Null as N, Text as T};
    fn t(s: &str) -> Value {
        T(s.into())
    }

    #[test]
    fn finds_empty_cells_markers_and_conditions() {
        let input = table(&[("age", DataType::Int)], vec![vec![I(30)], vec![N], vec![I(-999)], vec![I(-4)], vec![I(50)]]);
        let mut c = CleanColumn::new("age", Fill::Mean);
        c.missing_when = Some("[age] < 0".into());
        let out = run(&step(&["-999", "N/A"], vec![c]), &input).unwrap();
        // Mean of 30 and 50; the flag says which were missing.
        let ages: Vec<&Value> = out.rows.iter().map(|r| &r[0]).collect();
        assert_eq!(ages, vec![&I(30), &I(40), &I(40), &I(40), &I(50)]);
        let flags: Vec<&Value> = out.rows.iter().map(|r| &r[1]).collect();
        assert_eq!(flags, vec![&Value::Bool(false), &Value::Bool(true), &Value::Bool(true), &Value::Bool(true), &Value::Bool(false)]);
    }

    #[test]
    fn text_becomes_numbers_and_what_does_not_convert_is_missing() {
        let input = table(&[("price", DataType::Text)], vec![vec![t("4.5")], vec![t(" n/a ")], vec![t("abc")], vec![t("5.5")], vec![t("")]]);
        let mut c = CleanColumn::new("price", Fill::Median);
        c.as_type = Some(DataType::Float);
        let mut s = step(&["N/A", ""], vec![c]);
        if let StepKind::Clean { internal, .. } = &mut s.kind {
            *internal = true;
        }
        let out = run(&s, &input).unwrap();
        assert_eq!(out.schema.fields[0].dtype, DataType::Float);
        let prices: Vec<&Value> = out.rows.iter().map(|r| &r[0]).collect();
        assert_eq!(prices, vec![&F(4.5), &F(5.0), &F(5.0), &F(5.5), &F(5.0)]);
        let wi = out.schema.index_of("__vda_w0").unwrap();
        let why: Vec<&Value> = out.rows.iter().map(|r| &r[wi]).collect();
        assert_eq!(why, vec![&I(0), &I(2), &I(4), &I(0), &I(2)]);
    }

    #[test]
    fn mode_constant_and_groups() {
        let input = table(
            &[("region", DataType::Text), ("size", DataType::Text), ("n", DataType::Int)],
            vec![
                vec![t("N"), t("S"), I(1)],
                vec![t("N"), t("S"), N],
                vec![t("N"), N, I(3)],
                vec![t("S"), t("L"), I(10)],
                vec![t("S"), t("M"), N],
                vec![t("S"), N, I(20)],
            ],
        );
        let mut mean = CleanColumn::new("n", Fill::Mean);
        mean.group_by = vec!["region".into()];
        let size = CleanColumn::new("size", Fill::Constant { value: "unknown".into() });
        let out = run(&step(&[], vec![size, mean]), &input).unwrap();
        assert_eq!(out.rows[1][2], I(2));
        assert_eq!(out.rows[4][2], I(15));
        assert_eq!(out.rows[2][1], t("unknown"));
        // Mode: L and M tie in S, so the smaller (L).
        let mut m = CleanColumn::new("size", Fill::Mode);
        m.group_by = vec!["region".into()];
        let out = run(&step(&[], vec![m]), &input).unwrap();
        assert_eq!(out.rows[2][1], t("S"));
        assert_eq!(out.rows[5][1], t("L"));
    }

    #[test]
    fn previous_next_and_random_draws() {
        let input = table(&[("day", DataType::Int), ("v", DataType::Int)], vec![vec![I(3), I(30)], vec![I(1), I(10)], vec![I(2), N], vec![I(4), N]]);
        let order = vec![SortKey { column: "day".into(), descending: false }];
        let out = run(&step(&[], vec![CleanColumn::new("v", Fill::Previous { order_by: order.clone() })]), &input).unwrap();
        assert_eq!(out.rows[2][1], I(10));
        assert_eq!(out.rows[3][1], I(30));
        let out = run(&step(&[], vec![CleanColumn::new("v", Fill::Next { order_by: order })]), &input).unwrap();
        assert_eq!(out.rows[2][1], I(30));
        assert_eq!(out.rows[3][1], N);
        // A random draw is one of the known values, the same every time.
        let a = run(&step(&[], vec![CleanColumn::new("v", Fill::Random)]), &input).unwrap();
        let b = run(&step(&[], vec![CleanColumn::new("v", Fill::Random)]), &input).unwrap();
        assert_eq!(a, b);
        assert!([I(10), I(30)].contains(&a.rows[2][1]));
    }

    #[test]
    fn draws_spread_over_the_known_values() {
        let salt = draw_salt(7, 0);
        let mut counts = [0; 4];
        for r in 1..=4000 {
            counts[(draw(r, salt) % 4) as usize] += 1;
        }
        assert!(counts.iter().all(|&c| (850..1150).contains(&c)), "{counts:?}");
    }

    #[test]
    fn a_regression_recovers_its_line() {
        let rows: Vec<Vec<f64>> = (0..50).map(|i| {
            let (x1, x2) = (i as f64, ((i * 7) % 11) as f64);
            vec![x1, x2, 3.0 + 2.0 * x1 - 0.5 * x2]
        }).collect();
        let line = fit_regression(&["a".into(), "b".into()], &rows).unwrap();
        assert!((line.intercept - 3.0).abs() < 1e-9 && (line.slopes[0] - 2.0).abs() < 1e-9 && (line.slopes[1] + 0.5).abs() < 1e-9, "{line:?}");
        assert!((line.r2 - 1.0).abs() < 1e-12);
        let same: Vec<Vec<f64>> = (0..10).map(|i| vec![i as f64, 2.0 * i as f64, i as f64]).collect();
        assert!(fit_regression(&["a".into(), "b".into()], &same).unwrap_err().contains("moves together"));
    }

    #[test]
    fn neighbours_average_numbers_and_vote_on_text() {
        let x = vec![vec![0.0], vec![1.0], vec![10.0], vec![11.0]];
        let nums = vec![I(1), I(3), I(100), I(200)];
        let got = neighbours(&x, &nums, &[vec![0.4], vec![10.6]], 2, DataType::Int);
        assert_eq!(got, vec![Some(I(2)), Some(I(150))]);
        let words = vec![t("a"), t("a"), t("b"), t("c")];
        let got = neighbours(&x, &words, &[vec![0.2], vec![10.4]], 2, DataType::Text);
        assert_eq!(got, vec![Some(t("a")), Some(t("b"))]);
    }

    #[test]
    fn mistakes_are_explained() {
        let input = table(&[("a", DataType::Text), ("b", DataType::Int)], vec![]);
        let e = |c: CleanColumn| validate(&step(&[], vec![c]), &input.schema).unwrap_err().to_string();
        assert!(e(CleanColumn::new("a", Fill::Mean)).contains("needs numbers"));
        assert!(e(CleanColumn::new("b", Fill::Constant { value: "x".into() })).contains("isn't a whole number"), "{}", e(CleanColumn::new("b", Fill::Constant { value: "x".into() })));
        assert!(e(CleanColumn::new("b", Fill::Regression { predictors: vec!["a".into()] })).contains("isn't a number column"));
        assert!(e(CleanColumn::new("zz", Fill::Keep)).contains("no column `zz`"));
        let mut c = CleanColumn::new("b", Fill::Keep);
        c.as_type = Some(DataType::Float);
        assert!(e(c).contains("only text"));
    }
}
