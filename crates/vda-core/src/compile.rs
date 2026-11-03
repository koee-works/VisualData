//! Compile visual operations to DuckDB SQL, validate them against an input
//! schema, and build the inspectable SQL for a chain of steps.

use crate::model::{AggFunc, Analysis, CmpOp, JoinKind, SortKey, SourceFormat, Step, StepKind, WindowFunc};
use crate::table::{DataType, Field, Schema, Value};
use crate::Error;

/// CSV import options shared by the DuckDB engine and the inspectable SQL.
/// Type inference is restricted to the types the core models, so both engines
/// agree. Only ISO dates and timestamps are detected.
pub const CSV_OPTIONS: &str =
    "auto_type_candidates = ['BOOLEAN', 'BIGINT', 'DOUBLE', 'DATE', 'TIMESTAMP', 'VARCHAR'], header = true";

/// JSON import options: one object per line or an array of objects, types
/// detected from all records.
pub const JSON_OPTIONS: &str = "format = 'auto', records = 'true', sample_size = -1";

pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn quote_literal(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format!("{f:?}::DOUBLE"),
        Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Date(_) => format!("DATE '{v}'"),
        Value::Timestamp(_) => format!("TIMESTAMP '{v}'"),
    }
}

/// SQL for a single-input relational step, reading from the relation `input`.
/// Returns `None` for steps that do not execute as SQL (Python, Chart) and for
/// joins, whose SQL depends on their input columns (see [`step_sql_for`]).
pub fn step_sql(kind: &StepKind) -> Option<String> {
    match kind {
        StepKind::Filter { predicates } => {
            if predicates.is_empty() {
                return Some("SELECT * FROM input".into());
            }
            let conds: Vec<String> = predicates
                .iter()
                .map(|p| {
                    if p.op.takes_value() {
                        format!("{} {} {}", quote_ident(&p.column), p.op.token(), quote_literal(&p.value))
                    } else {
                        format!("{} {}", quote_ident(&p.column), p.op.token())
                    }
                })
                .collect();
            Some(format!("SELECT * FROM input WHERE {}", conds.join(" AND ")))
        }
        StepKind::Aggregate { group_by, measures } => {
            let mut cols: Vec<String> = group_by.iter().map(|g| quote_ident(g)).collect();
            for m in measures {
                let arg = match &m.column {
                    Some(c) => quote_ident(c),
                    None => "*".into(),
                };
                cols.push(format!("{} AS {}", m.func.sql_over(&arg), quote_ident(&m.alias)));
            }
            let mut sql = format!("SELECT {} FROM input", cols.join(", "));
            if !group_by.is_empty() {
                let keys: Vec<String> = group_by.iter().map(|g| quote_ident(g)).collect();
                // ORDER BY keeps output deterministic so replay hashes are stable.
                sql.push_str(&format!(
                    " GROUP BY {} ORDER BY {}",
                    keys.join(", "),
                    keys.iter().map(|k| format!("{k} ASC NULLS LAST")).collect::<Vec<_>>().join(", ")
                ));
            }
            Some(sql)
        }
        StepKind::Select { columns } => Some(format!(
            "SELECT {} FROM input",
            columns
                .iter()
                .map(|c| match &c.rename {
                    Some(n) => format!("{} AS {}", quote_ident(&c.column), quote_ident(n)),
                    None => quote_ident(&c.column),
                })
                .collect::<Vec<_>>()
                .join(", ")
        )),
        StepKind::Sort { by, limit } => {
            // Row numbers keep ties in input order; missing values sort last
            // in both directions.
            let mut keys: Vec<String> = by
                .iter()
                .map(|k| format!("{} {} NULLS LAST", quote_ident(&k.column), if k.descending { "DESC" } else { "ASC" }))
                .collect();
            keys.push("__vda_r".into());
            let limit = limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default();
            Some(format!(
                "SELECT * EXCLUDE (__vda_r) FROM (SELECT *, row_number() OVER () AS __vda_r FROM input) ORDER BY {}{limit}",
                keys.join(", ")
            ))
        }
        StepKind::Sql { sql } => Some(sql.trim().trim_end_matches(';').to_string()),
        StepKind::Join { .. }
        | StepKind::Append { .. }
        | StepKind::Derive { .. }
        | StepKind::Window { .. }
        | StepKind::Dedupe { .. }
        | StepKind::Pivot { .. }
        | StepKind::Unpivot { .. }
        | StepKind::Python { .. }
        | StepKind::Chart { .. } => None,
    }
}

/// SQL for any relational step, given its input schemas in input order. The
/// first input is the relation `input`; every input is also its own id.
pub fn step_sql_for(step: &Step, inputs: &[Schema]) -> Result<Option<String>, Error> {
    match &step.kind {
        StepKind::Join { how, on } => {
            let (left, right) = match inputs {
                [l, r] => (l, r),
                _ => return Err(Error::Validation(format!("step `{}`: a join reads two inputs", step.id))),
            };
            let plan = join_plan(step, left, right)?;
            let mut cols = Vec::new();
            for f in &left.fields {
                let l = format!("l.{}", quote_ident(&f.name));
                match on.iter().find(|k| k.left == f.name && k.op == CmpOp::Eq) {
                    // In a full join a key exists on either side.
                    Some(k) if *how == JoinKind::Full => {
                        cols.push(format!("COALESCE({l}, r.{}) AS {}", quote_ident(&k.right), quote_ident(&f.name)))
                    }
                    _ => cols.push(l),
                }
            }
            for (src, out) in &plan.right_columns {
                let r = format!("r.{}", quote_ident(src));
                cols.push(if src == out { r } else { format!("{r} AS {}", quote_ident(out)) });
            }
            let cond: Vec<String> = on
                .iter()
                .map(|k| format!("l.{} {} r.{}", quote_ident(&k.left), k.op.token(), quote_ident(&k.right)))
                .collect();
            // Row numbers fix the output order: left rows in their order, each
            // followed by its matches in right order; unmatched right rows last.
            Ok(Some(format!(
                "SELECT {} FROM (SELECT *, row_number() OVER () AS __vda_l FROM input) AS l \
                 {} (SELECT *, row_number() OVER () AS __vda_r FROM {}) AS r ON {} \
                 ORDER BY l.__vda_l ASC NULLS LAST, r.__vda_r ASC NULLS LAST",
                cols.join(", "),
                how.sql(),
                quote_ident(&step.inputs[1]),
                cond.join(" AND ")
            )))
        }
        StepKind::Append { distinct, origin } => {
            let plan = append_plan(step, inputs)?;
            let mut parts = Vec::new();
            for (i, (id, schema)) in step.inputs.iter().zip(inputs).enumerate() {
                let mut cols: Vec<String> = plan
                    .schema
                    .fields
                    .iter()
                    .filter(|f| Some(&f.name) != origin.as_ref())
                    .map(|f| {
                        let src = if schema.field(&f.name).is_some() { quote_ident(&f.name) } else { "NULL".into() };
                        format!("CAST({src} AS {}) AS {}", f.dtype.name(), quote_ident(&f.name))
                    })
                    .collect();
                if let Some(o) = origin {
                    cols.push(format!("{} AS {}", quote_literal(&Value::Text(id.clone())), quote_ident(o)));
                }
                parts.push(format!(
                    "SELECT {}, {i} AS __vda_i, row_number() OVER () AS __vda_r FROM {}",
                    cols.join(", "),
                    quote_ident(id)
                ));
            }
            let names: Vec<String> = plan.schema.fields.iter().map(|f| quote_ident(&f.name)).collect();
            // Rows keep input order, then row order; with `distinct`, the
            // first of each set of identical rows is kept (missing values
            // count as equal here, as in SQL DISTINCT).
            let dedup = if *distinct {
                format!(" QUALIFY row_number() OVER (PARTITION BY {} ORDER BY __vda_i, __vda_r) = 1", names.join(", "))
            } else {
                String::new()
            };
            Ok(Some(format!(
                "SELECT {} FROM ({}) AS u{dedup} ORDER BY __vda_i, __vda_r",
                names.join(", "),
                parts.join(" UNION ALL ")
            )))
        }
        StepKind::Derive { columns } => {
            let mut schema = inputs.first().cloned().ok_or_else(|| Error::Validation(format!("step `{}` has no input", step.id)))?;
            let mut sql = "SELECT * FROM input".to_string();
            for c in columns {
                let (expr, dtype) = derived(step, c, &schema)?;
                let body = crate::formula::to_sql(&expr, &schema).map_err(|m| derive_error(step, c, m))?;
                sql = if schema.field(&c.name).is_some() {
                    format!("SELECT * REPLACE ({body} AS {}) FROM ({sql})", quote_ident(&c.name))
                } else {
                    format!("SELECT *, {body} AS {} FROM ({sql})", quote_ident(&c.name))
                };
                set_field(&mut schema, &c.name, dtype);
            }
            Ok(Some(sql))
        }
        StepKind::Window { partition_by, order_by, columns } => {
            let input = first(step, inputs)?;
            window_schema(step, input)?;
            let part = if partition_by.is_empty() {
                String::new()
            } else {
                format!("PARTITION BY {} ", partition_by.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", "))
            };
            // Ties in the order keep input order, except for rank.
            let keys = order_keys(order_by);
            let ordered = format!("({part}ORDER BY {})", keys.iter().cloned().chain(["__vda_r".to_string()]).collect::<Vec<_>>().join(", "));
            let ranked = if keys.is_empty() { format!("({part})") } else { format!("({part}ORDER BY {})", keys.join(", ")) };
            let frame = |f: &str| format!("({part}ORDER BY {} {f})", keys.iter().cloned().chain(["__vda_r".to_string()]).collect::<Vec<_>>().join(", "));
            let mut exprs = Vec::new();
            for c in columns {
                let col = c.column.as_deref().map(quote_ident).unwrap_or_default();
                let is_int = c.column.as_deref().and_then(|n| input.field(n)).is_some_and(|f| f.dtype == DataType::Int);
                let n = c.n.unwrap_or(1);
                let prev_d = format!("CAST(lag({col}, 1) OVER {ordered} AS DOUBLE)");
                let e = match c.func {
                    WindowFunc::RunningSum => {
                        let s = format!("sum({col}) OVER {}", frame("ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW"));
                        if is_int { format!("CAST({s} AS BIGINT)") } else { s }
                    }
                    WindowFunc::RowNumber => format!("row_number() OVER {ordered}"),
                    WindowFunc::Rank => format!("rank() OVER {ranked}"),
                    WindowFunc::Previous => format!("lag({col}, {n}) OVER {ordered}"),
                    WindowFunc::Next => format!("lead({col}, {n}) OVER {ordered}"),
                    WindowFunc::MovingAverage => format!(
                        "CAST(avg({col}) OVER {} AS DOUBLE)",
                        frame(&format!("ROWS BETWEEN {} PRECEDING AND CURRENT ROW", n - 1))
                    ),
                    WindowFunc::Change => format!("({col} - lag({col}, 1) OVER {ordered})"),
                    WindowFunc::PercentChange => format!("((CAST({col} AS DOUBLE) - {prev_d}) / NULLIF({prev_d}, 0))"),
                    WindowFunc::ShareOfTotal => format!(
                        "(CAST({col} AS DOUBLE) / NULLIF(CAST(sum({col}) OVER ({part}) AS DOUBLE), 0))"
                    ),
                };
                exprs.push(format!("{e} AS {}", quote_ident(&c.name)));
            }
            Ok(Some(format!(
                "SELECT * EXCLUDE (__vda_r) FROM (SELECT *, {} FROM (SELECT *, row_number() OVER () AS __vda_r FROM input)) ORDER BY __vda_r",
                exprs.join(", ")
            )))
        }
        StepKind::Dedupe { keys, order_by } => {
            let input = first(step, inputs)?;
            dedupe_check(step, input)?;
            let part: Vec<String> = if keys.is_empty() {
                input.fields.iter().map(|f| quote_ident(&f.name)).collect()
            } else {
                keys.iter().map(|k| quote_ident(k)).collect()
            };
            let mut order = order_keys(order_by);
            order.push("__vda_r".into());
            // Missing values count as equal keys, as in SQL DISTINCT.
            Ok(Some(format!(
                "SELECT * EXCLUDE (__vda_r) FROM (SELECT *, row_number() OVER () AS __vda_r FROM input) \
                 QUALIFY row_number() OVER (PARTITION BY {} ORDER BY {}) = 1 ORDER BY __vda_r",
                part.join(", "),
                order.join(", ")
            )))
        }
        StepKind::Unpivot { columns, name, value } => {
            let input = first(step, inputs)?;
            let schema = unpivot_schema(step, input)?;
            let vtype = schema.fields.last().unwrap().dtype;
            let kept: Vec<String> =
                input.fields.iter().filter(|f| !columns.contains(&f.name)).map(|f| format!("t.{}", quote_ident(&f.name))).collect();
            let names: Vec<String> =
                columns.iter().enumerate().map(|(i, c)| format!("({i}, {})", quote_literal(&Value::Text(c.clone())))).collect();
            let cases: Vec<String> = columns
                .iter()
                .enumerate()
                .map(|(i, c)| format!("WHEN {i} THEN CAST(t.{} AS {})", quote_ident(c), vtype.name()))
                .collect();
            let mut cols = kept;
            cols.push(format!("v.__vda_name AS {}", quote_ident(name)));
            cols.push(format!("CASE v.__vda_c {} END AS {}", cases.join(" "), quote_ident(value)));
            // One row per input row and listed column, in that order.
            Ok(Some(format!(
                "SELECT {} FROM (SELECT *, row_number() OVER () AS __vda_r FROM input) AS t \
                 CROSS JOIN (VALUES {}) AS v(__vda_c, __vda_name) ORDER BY t.__vda_r, v.__vda_c",
                cols.join(", "),
                names.join(", ")
            )))
        }
        StepKind::Pivot { rows, column, value, func } => {
            let input = first(step, inputs)?;
            pivot_check(step, input)?;
            let mut picked: Vec<String> = rows.iter().map(|r| quote_ident(r)).collect();
            picked.push(quote_ident(column));
            if let Some(v) = value {
                picked.push(quote_ident(v));
            }
            let agg = match value {
                Some(v) => func.sql_over(&quote_ident(v)),
                None => "count(*)".into(),
            };
            let keys: Vec<String> = rows.iter().map(|r| quote_ident(r)).collect();
            // DuckDB names the new columns by value, in value order, and
            // leaves out rows whose pivot value is missing.
            let mut sql = format!("PIVOT (SELECT {} FROM input) ON {} USING {agg}", picked.join(", "), quote_ident(column));
            if !keys.is_empty() {
                sql = format!(
                    "SELECT * FROM ({sql} GROUP BY {}) ORDER BY {}",
                    keys.join(", "),
                    keys.iter().map(|k| format!("{k} ASC NULLS LAST")).collect::<Vec<_>>().join(", ")
                );
            }
            Ok(Some(sql))
        }
        kind => Ok(step_sql(kind)),
    }
}

fn first<'a>(step: &Step, inputs: &'a [Schema]) -> Result<&'a Schema, Error> {
    inputs.first().ok_or_else(|| Error::Validation(format!("step `{}` has no input", step.id)))
}

fn order_keys(by: &[SortKey]) -> Vec<String> {
    by.iter()
        .map(|k| format!("{} {} NULLS LAST", quote_ident(&k.column), if k.descending { "DESC" } else { "ASC" }))
        .collect()
}

/// The most rows back or ahead a window calculation can look.
pub const MAX_WINDOW_OFFSET: u32 = 100_000;

fn window_schema(step: &Step, input: &Schema) -> Result<Schema, Error> {
    let StepKind::Window { partition_by, order_by, columns } = &step.kind else { unreachable!() };
    let id = &step.id;
    for c in partition_by {
        require(input, c, id)?;
    }
    for k in order_by {
        require(input, &k.column, id)?;
    }
    if columns.is_empty() {
        return Err(Error::Validation(format!("step `{id}`: add a calculation")));
    }
    let mut schema = input.clone();
    for c in columns {
        let label = c.func.name().replace('_', " ");
        if c.name.trim().is_empty() {
            return Err(Error::Validation(format!("step `{id}`: name the {label} column")));
        }
        if schema.field(&c.name).is_some() {
            return Err(Error::Validation(format!("step `{id}`: there is already a column `{}`; choose another name", c.name)));
        }
        let src = match (&c.column, c.func.needs_column()) {
            (Some(col), true) => Some(require(input, col, id)?.dtype),
            (None, true) => return Err(Error::Validation(format!("step `{id}`: choose the column for {label}"))),
            (_, false) => None,
        };
        if let Some(t) = src {
            if c.func.needs_number() && !t.is_numeric() {
                return Err(Error::Validation(format!(
                    "step `{id}`: {label} needs a numeric column, `{}` is {}",
                    c.column.as_deref().unwrap_or(""),
                    t.plain()
                )));
            }
        }
        if let Some(n) = c.n {
            if !(1..=MAX_WINDOW_OFFSET).contains(&n) {
                return Err(Error::Validation(format!("step `{id}`: the number of rows for `{}` must be from 1 to {MAX_WINDOW_OFFSET}", c.name)));
            }
        }
        let dtype = match c.func {
            WindowFunc::RowNumber | WindowFunc::Rank => DataType::Int,
            WindowFunc::MovingAverage | WindowFunc::PercentChange | WindowFunc::ShareOfTotal => DataType::Float,
            WindowFunc::RunningSum | WindowFunc::Previous | WindowFunc::Next | WindowFunc::Change => src.unwrap(),
        };
        if c.func == WindowFunc::MovingAverage && c.n.is_none() {
            return Err(Error::Validation(format!("step `{id}`: choose how many rows the moving average of `{}` covers", c.name)));
        }
        schema.fields.push(Field { name: c.name.clone(), dtype });
    }
    Ok(schema)
}

fn dedupe_check(step: &Step, input: &Schema) -> Result<(), Error> {
    let StepKind::Dedupe { keys, order_by } = &step.kind else { unreachable!() };
    for k in keys {
        require(input, k, &step.id)?;
    }
    for k in order_by {
        require(input, &k.column, &step.id)?;
    }
    Ok(())
}

/// Pivot columns are named after their values, so only values with one
/// exact written form can be pivoted.
pub fn pivotable(t: DataType) -> bool {
    matches!(t, DataType::Text | DataType::Int | DataType::Bool | DataType::Date)
}

fn pivot_check(step: &Step, input: &Schema) -> Result<(), Error> {
    let StepKind::Pivot { rows, column, value, func } = &step.kind else { unreachable!() };
    let id = &step.id;
    if column.is_empty() {
        return Err(Error::Validation(format!("step `{id}`: choose the column whose values become columns")));
    }
    let c = require(input, column, id)?;
    if !pivotable(c.dtype) {
        return Err(Error::Validation(format!(
            "step `{id}`: cannot make columns from {} column `{column}`; pivot a text, whole-number, true/false or date column (use bucket() or date_trunc() first)",
            c.dtype.plain()
        )));
    }
    for (i, r) in rows.iter().enumerate() {
        require(input, r, id)?;
        if r == column || rows[..i].contains(r) || value.as_ref() == Some(r) {
            return Err(Error::Validation(format!("step `{id}`: `{r}` is used twice; each column has one role")));
        }
    }
    if value.as_ref() == Some(column) {
        return Err(Error::Validation(format!("step `{id}`: summarize a different column than `{column}`")));
    }
    measure_type(id, *func, value.as_deref(), input)?;
    Ok(())
}

/// The type an aggregate gives over a column (None: count rows).
pub(crate) fn measure_type(id: &str, func: AggFunc, column: Option<&str>, input: &Schema) -> Result<DataType, Error> {
    Ok(match (func, column) {
        (AggFunc::Count, _) => DataType::Int,
        (_, None) => return Err(Error::Validation(format!("step `{id}`: {} needs a column", func.token()))),
        (func, Some(c)) => {
            let f = require(input, c, id)?;
            let numeric_only = matches!(func, AggFunc::Sum | AggFunc::Avg) || func.is_statistical();
            if numeric_only && !f.dtype.is_numeric() {
                return Err(Error::Validation(format!(
                    "step `{id}`: {} needs a numeric column, `{c}` is {}",
                    func.token(),
                    f.dtype.plain()
                )));
            }
            match func {
                AggFunc::CountDistinct => DataType::Int,
                f2 if f2.is_statistical() => DataType::Float,
                AggFunc::Avg => DataType::Float,
                _ => f.dtype,
            }
        }
    })
}

fn unpivot_schema(step: &Step, input: &Schema) -> Result<Schema, Error> {
    let StepKind::Unpivot { columns, name, value } = &step.kind else { unreachable!() };
    let id = &step.id;
    if columns.is_empty() {
        return Err(Error::Validation(format!("step `{id}`: choose the columns to turn into rows")));
    }
    let mut dtype: Option<(DataType, &str)> = None;
    for (i, c) in columns.iter().enumerate() {
        if columns[..i].contains(c) {
            return Err(Error::Validation(format!("step `{id}`: `{c}` is listed twice")));
        }
        let t = require(input, c, id)?.dtype;
        dtype = Some(match dtype {
            None => (t, c),
            Some((a, first)) if a == t => (a, first),
            Some((a, first)) if a.is_numeric() && t.is_numeric() => (DataType::Float, first),
            Some((a, first)) => {
                return Err(Error::Validation(format!(
                    "step `{id}`: `{first}` is {} but `{c}` is {}; the values share one column, so unpivot columns of one type",
                    a.plain(),
                    t.plain()
                )))
            }
        });
    }
    let mut fields: Vec<Field> = input.fields.iter().filter(|f| !columns.contains(&f.name)).cloned().collect();
    for (what, n) in [("names", name), ("values", value)] {
        if n.trim().is_empty() {
            return Err(Error::Validation(format!("step `{id}`: name the column for the {what}")));
        }
        if fields.iter().any(|f| &f.name == n) {
            return Err(Error::Validation(format!("step `{id}`: there is already a column `{n}`; choose another name")));
        }
        fields.push(Field { name: n.clone(), dtype: if what == "names" { DataType::Text } else { dtype.unwrap().0 } });
    }
    Ok(Schema { fields })
}

fn derive_error(step: &Step, c: &crate::model::Derived, m: String) -> Error {
    let name = if c.name.trim().is_empty() { "the new column".to_string() } else { format!("`{}`", c.name) };
    Error::Validation(format!("step `{}`: formula for {name}: {m}", step.id))
}

/// Parse and type a derived column against the columns before it.
fn derived(step: &Step, c: &crate::model::Derived, schema: &Schema) -> Result<(crate::formula::Expr, DataType), Error> {
    if c.name.trim().is_empty() {
        return Err(Error::Validation(format!("step `{}`: name each calculated column", step.id)));
    }
    let expr = crate::formula::parse(&c.formula).map_err(|m| derive_error(step, c, m))?;
    let dtype = crate::formula::result_type(&expr, schema).map_err(|m| derive_error(step, c, m))?;
    Ok((expr, dtype))
}

fn set_field(schema: &mut Schema, name: &str, dtype: DataType) {
    match schema.fields.iter_mut().find(|f| f.name == name) {
        Some(f) => f.dtype = dtype,
        None => schema.fields.push(Field { name: name.to_string(), dtype }),
    }
}

/// The schemas a derive step sees before each of its columns, and the
/// parsed formulas (for the reference engine).
pub fn derive_plan(step: &Step, input: &Schema) -> Result<Vec<(crate::formula::Expr, Schema)>, Error> {
    let StepKind::Derive { columns } = &step.kind else { unreachable!("derive_plan is only called for derives") };
    let mut schema = input.clone();
    let mut out = Vec::new();
    for c in columns {
        let (expr, dtype) = derived(step, c, &schema)?;
        out.push((expr, schema.clone()));
        set_field(&mut schema, &c.name, dtype);
    }
    Ok(out)
}

/// Output columns of an append: the first input's columns, then columns
/// that appear only in later inputs, in order; then the origin column.
struct AppendPlan {
    schema: Schema,
}

fn append_plan(step: &Step, inputs: &[Schema]) -> Result<AppendPlan, Error> {
    let StepKind::Append { origin, .. } = &step.kind else { unreachable!("append_plan is only called for appends") };
    let id = &step.id;
    let mut fields: Vec<Field> = Vec::new();
    let mut seen_in: Vec<String> = Vec::new();
    for (input_id, schema) in step.inputs.iter().zip(inputs) {
        for f in &schema.fields {
            match fields.iter().position(|g| g.name == f.name) {
                None => {
                    fields.push(f.clone());
                    seen_in.push(input_id.clone());
                }
                Some(i) => {
                    let (a, b) = (fields[i].dtype, f.dtype);
                    let temporal = |t: DataType| matches!(t, DataType::Date | DataType::Timestamp);
                    fields[i].dtype = if a == b {
                        a
                    } else if a.is_numeric() && b.is_numeric() {
                        DataType::Float
                    } else if temporal(a) && temporal(b) {
                        DataType::Timestamp
                    } else {
                        return Err(Error::Validation(format!(
                            "step `{id}`: column `{}` is {} in `{}` but {} in `{input_id}`; make the types match before appending",
                            f.name,
                            a.name(),
                            seen_in[i],
                            b.name()
                        )));
                    };
                }
            }
        }
    }
    if let Some(o) = origin {
        if o.trim().is_empty() {
            return Err(Error::Validation(format!("step `{id}`: name the column that records each row's input")));
        }
        if fields.iter().any(|f| &f.name == o) {
            return Err(Error::Validation(format!("step `{id}`: the inputs already have a column `{o}`; choose another name")));
        }
        fields.push(Field { name: o.clone(), dtype: DataType::Text });
    }
    Ok(AppendPlan { schema: Schema { fields } })
}

/// How a join's output columns are formed.
struct JoinPlan {
    /// Right non-key columns as (source name, output name).
    right_columns: Vec<(String, String)>,
    schema: Schema,
}

fn join_plan(step: &Step, left: &Schema, right: &Schema) -> Result<JoinPlan, Error> {
    let StepKind::Join { how, on } = &step.kind else { unreachable!("join_plan is only called for joins") };
    let id = &step.id;
    if on.is_empty() {
        return Err(Error::Validation(format!("step `{id}`: choose at least one pair of columns to join on")));
    }
    let mut fields: Vec<Field> = left.fields.clone();
    for k in on {
        if !k.op.takes_value() {
            return Err(Error::Validation(format!(
                "step `{id}`: a join condition compares two columns; `{}` is not a comparison",
                k.op.token()
            )));
        }
        let l = require(left, &k.left, id)?;
        let r = require(right, &k.right, id)?;
        let temporal = |t: DataType| matches!(t, DataType::Date | DataType::Timestamp);
        let compatible = l.dtype == r.dtype
            || (l.dtype.is_numeric() && r.dtype.is_numeric())
            || (temporal(l.dtype) && temporal(r.dtype));
        if !compatible {
            return Err(Error::Validation(format!(
                "step `{id}`: cannot join {} column `{}` with {} column `{}`",
                l.dtype.name(),
                k.left,
                r.dtype.name(),
                k.right
            )));
        }
        if *how == JoinKind::Full && k.op == CmpOp::Eq && l.dtype != r.dtype {
            // COALESCE widens: an integer and a decimal give a decimal, a
            // date and a timestamp give a timestamp.
            let wider = if temporal(l.dtype) { DataType::Timestamp } else { DataType::Float };
            fields.iter_mut().find(|f| f.name == k.left).unwrap().dtype = wider;
        }
    }
    // Only columns matched with `=` duplicate a left column; columns used in
    // comparisons carry their own values and are kept.
    let right_keys: Vec<&str> = on.iter().filter(|k| k.op == CmpOp::Eq).map(|k| k.right.as_str()).collect();
    let suffix = &step.inputs[1];
    let mut right_columns = Vec::new();
    for f in &right.fields {
        if right_keys.contains(&f.name.as_str()) {
            continue; // equal to the left key it matched
        }
        let mut out = f.name.clone();
        if fields.iter().any(|g| g.name == out) {
            out = format!("{}_{suffix}", f.name);
            if fields.iter().any(|g| g.name == out) {
                return Err(Error::Validation(format!(
                    "step `{id}`: column `{}` of `{suffix}` clashes with `{out}`; rename one before joining",
                    f.name
                )));
            }
        }
        fields.push(Field { name: out.clone(), dtype: f.dtype });
        right_columns.push((f.name.clone(), out));
    }
    Ok(JoinPlan { right_columns, schema: Schema { fields } })
}

fn require<'a>(schema: &'a Schema, col: &str, step: &str) -> Result<&'a Field, Error> {
    schema.field(col).ok_or_else(|| {
        let available: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
        Error::Validation(format!(
            "step `{step}`: column `{col}` not found (available: {})",
            available.join(", ")
        ))
    })
}

fn literal_fits(col: DataType, v: &Value) -> bool {
    match (col, v) {
        (_, Value::Null) => false,
        (DataType::Int | DataType::Float, Value::Int(_) | Value::Float(_)) => true,
        (DataType::Text, Value::Text(_)) => true,
        (DataType::Bool, Value::Bool(_)) => true,
        (DataType::Date | DataType::Timestamp, Value::Date(_) | Value::Timestamp(_)) => true,
        _ => false,
    }
}

/// Validate a visual step against its actual input schemas (in input order)
/// and return the output schema. Custom SQL/Python return `Ok(None)`: their
/// schema is only known after they run.
pub fn output_schema(step: &Step, inputs: &[Schema]) -> Result<Option<Schema>, Error> {
    let id = &step.id;
    let input = inputs
        .first()
        .ok_or_else(|| Error::Validation(format!("step `{id}` has no input")))?;
    match &step.kind {
        StepKind::Join { .. } => match inputs {
            [l, r] => Ok(Some(join_plan(step, l, r)?.schema)),
            _ => Err(Error::Validation(format!("step `{id}`: a join reads two inputs"))),
        },
        StepKind::Append { .. } if inputs.len() < 2 => {
            Err(Error::Validation(format!("step `{id}`: appending needs at least two inputs")))
        }
        StepKind::Append { .. } => Ok(Some(append_plan(step, inputs)?.schema)),
        StepKind::Filter { predicates } => {
            for p in predicates {
                let f = require(input, &p.column, id)?;
                if p.op.takes_value() && !literal_fits(f.dtype, &p.value) {
                    let hint = if p.value.is_null() {
                        " (use IS NULL / IS NOT NULL to test for missing values)"
                    } else {
                        ""
                    };
                    return Err(Error::Validation(format!(
                        "step `{id}`: cannot compare {} column `{}` with {}{hint}",
                        f.dtype.name(),
                        p.column,
                        quote_literal(&p.value)
                    )));
                }
            }
            Ok(Some(input.clone()))
        }
        StepKind::Derive { columns } => {
            if columns.is_empty() {
                return Err(Error::Validation(format!("step `{id}`: add a calculated column")));
            }
            let mut schema = input.clone();
            for c in columns {
                let (_, dtype) = derived(step, c, &schema)?;
                set_field(&mut schema, &c.name, dtype);
            }
            Ok(Some(schema))
        }
        StepKind::Select { columns } => {
            if columns.is_empty() {
                return Err(Error::Validation(format!("step `{id}`: choose at least one column to keep")));
            }
            let mut fields: Vec<Field> = Vec::new();
            for c in columns {
                let f = require(input, &c.column, id)?;
                let name = c.rename.clone().unwrap_or_else(|| c.column.clone());
                if name.trim().is_empty() {
                    return Err(Error::Validation(format!("step `{id}`: the new name for `{}` is empty", c.column)));
                }
                if fields.iter().any(|g| g.name == name) {
                    return Err(Error::Validation(format!("step `{id}`: two columns would be called `{name}`")));
                }
                fields.push(Field { name, dtype: f.dtype });
            }
            Ok(Some(Schema { fields }))
        }
        StepKind::Sort { by, limit } => {
            if by.is_empty() && limit.is_none() {
                return Err(Error::Validation(format!("step `{id}`: choose a column to sort by, or a number of rows to keep")));
            }
            for k in by {
                require(input, &k.column, id)?;
            }
            Ok(Some(input.clone()))
        }
        StepKind::Aggregate { group_by, measures } => {
            let mut fields = Vec::new();
            for g in group_by {
                fields.push(require(input, g, id)?.clone());
            }
            if measures.is_empty() && group_by.is_empty() {
                return Err(Error::Validation(format!("step `{id}`: aggregate needs a group or a measure")));
            }
            for m in measures {
                let dtype = measure_type(id, m.func, m.column.as_deref(), input)?;
                if fields.iter().any(|f: &Field| f.name == m.alias) {
                    return Err(Error::Validation(format!("step `{id}`: duplicate output column `{}`", m.alias)));
                }
                fields.push(Field { name: m.alias.clone(), dtype });
            }
            Ok(Some(Schema { fields }))
        }
        StepKind::Window { .. } => Ok(Some(window_schema(step, input)?)),
        StepKind::Dedupe { .. } => {
            dedupe_check(step, input)?;
            Ok(Some(input.clone()))
        }
        StepKind::Unpivot { .. } => Ok(Some(unpivot_schema(step, input)?)),
        // The new columns are the values found when it runs.
        StepKind::Pivot { .. } => {
            pivot_check(step, input)?;
            Ok(None)
        }
        StepKind::Chart { x, y, .. } => {
            require(input, x, id)?;
            require(input, y, id)?;
            Ok(None)
        }
        StepKind::Sql { .. } | StepKind::Python { .. } => Ok(None),
    }
}

/// Inspectable SQL for a step: every SQL-executable step it depends on, back
/// to the sources (or to the nearest Python steps), as one query with one CTE
/// per step, named by step id. Joins need their input columns; without them
/// (`schema_of` returns `None`) a join has no SQL form yet.
pub fn pipeline_sql_with(
    analysis: &Analysis,
    step_id: &str,
    schema_of: &dyn Fn(&str) -> Option<Schema>,
) -> Result<String, Error> {
    let target = analysis
        .step(step_id)
        .ok_or_else(|| Error::Validation(format!("unknown step `{step_id}`")))?;
    match target.kind {
        StepKind::Python { .. } => {
            return Err(Error::Unsupported(format!("step `{step_id}` is custom Python; it has no SQL form")))
        }
        StepKind::Chart { .. } => {
            return Err(Error::Unsupported(format!("step `{step_id}` is a chart; it has no SQL form")))
        }
        _ => {}
    }
    let label = |s: &Step| if s.kind.is_visual() { "visual" } else { "custom SQL" };
    let body_of = |s: &Step| -> Result<String, Error> {
        let schemas: Option<Vec<Schema>> = s.inputs.iter().map(|i| schema_of(i)).collect();
        match (&s.kind, schemas) {
            (
                StepKind::Join { .. }
                | StepKind::Append { .. }
                | StepKind::Derive { .. }
                | StepKind::Window { .. }
                | StepKind::Dedupe { .. }
                | StepKind::Pivot { .. }
                | StepKind::Unpivot { .. },
                None,
            ) => Err(Error::Unsupported(format!(
                "step `{}`: its SQL is shown once its inputs have run",
                s.id
            ))),
            (_, schemas) => Ok(step_sql_for(s, &schemas.unwrap_or_default())?.expect("SQL step")),
        }
    };

    // Dependencies first, each once.
    let mut ctes: Vec<String> = Vec::new();
    let mut done: std::collections::HashSet<String> = Default::default();
    fn walk(
        a: &Analysis,
        id: &str,
        done: &mut std::collections::HashSet<String>,
        ctes: &mut Vec<String>,
        body_of: &dyn Fn(&Step) -> Result<String, Error>,
        label: &dyn Fn(&Step) -> &'static str,
    ) -> Result<(), Error> {
        if !done.insert(id.to_string()) {
            return Ok(());
        }
        if let Some(src) = a.source(id) {
            let path = quote_literal(&Value::Text(src.path.clone()));
            let read = match src.format {
                SourceFormat::Csv => format!("SELECT * FROM read_csv({path}, {CSV_OPTIONS})"),
                SourceFormat::Parquet => format!("SELECT * FROM read_parquet({path})"),
                SourceFormat::Json => format!("SELECT * FROM read_json({path}, {JSON_OPTIONS})"),
                // Spreadsheets are converted on import; there is no SQL reader.
                SourceFormat::Excel => format!(
                    "SELECT * FROM {} /* sheet {} of {}, converted on import */",
                    quote_ident(&format!("{id}_converted")),
                    src.sheet.as_deref().unwrap_or("1"),
                    src.path
                ),
            };
            ctes.push(format!("-- source `{id}`\n{} AS ({read})", quote_ident(id)));
            return Ok(());
        }
        let step = a.step(id).ok_or_else(|| Error::Validation(format!("unknown step `{id}`")))?;
        if let StepKind::Python { .. } = step.kind {
            ctes.push(format!(
                "{} AS (SELECT * FROM {} /* materialized output of Python step `{id}` */)",
                quote_ident(id),
                quote_ident(&format!("{id}_output"))
            ));
            return Ok(());
        }
        for input in &step.inputs {
            walk(a, input, done, ctes, body_of, label)?;
        }
        // The body is wrapped as a subquery so custom SQL with its own WITH
        // clause stays valid; `input` is the step's first input.
        ctes.push(format!(
            "-- step `{id}` ({}, {})\n{} AS (WITH input AS (SELECT * FROM {}) SELECT * FROM ({}))",
            step.kind.type_name(),
            label(step),
            quote_ident(id),
            quote_ident(step.input()),
            body_of(step)?
        ));
        Ok(())
    }
    for input in &target.inputs {
        walk(analysis, input, &mut done, &mut ctes, &body_of, &label)?;
    }
    ctes.push(format!(
        "-- step `{step_id}` ({}, {})\ninput AS (SELECT * FROM {})",
        target.kind.type_name(),
        label(target),
        quote_ident(target.input())
    ));
    // The final step stays at the top level so its ORDER BY is honoured.
    let body = body_of(target)?;
    let head = ctes.join(",\n");
    let upper = body.to_ascii_uppercase();
    Ok(if upper.starts_with("WITH RECURSIVE") {
        format!("WITH\n{head}\nSELECT * FROM ({body})")
    } else if upper.starts_with("WITH") && body[4..].starts_with(char::is_whitespace) {
        format!("WITH\n{head},\n{}", body[4..].trim_start())
    } else {
        format!("WITH\n{head}\n{body}")
    })
}

/// [`pipeline_sql_with`] when no input columns are known (joins excluded).
pub fn pipeline_sql(analysis: &Analysis, step_id: &str) -> Result<String, Error> {
    pipeline_sql_with(analysis, step_id, &|_| None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CmpOp, Measure, Predicate};

    fn schema() -> Schema {
        Schema::new(vec![("region", DataType::Text), ("amount", DataType::Float), ("units", DataType::Int)])
    }

    fn step(kind: StepKind) -> Step {
        Step { id: "s".into(), inputs: vec![crate::model::SOURCE_ID.into()], label: None, kind }
    }

    #[test]
    fn filter_sql_quotes_identifiers_and_literals() {
        let k = StepKind::Filter {
            predicates: vec![
                Predicate { column: "re\"gion".into(), op: CmpOp::Eq, value: Value::Text("O'Hare".into()) },
                Predicate { column: "units".into(), op: CmpOp::IsNotNull, value: Value::Null },
            ],
        };
        assert_eq!(
            step_sql(&k).unwrap(),
            "SELECT * FROM input WHERE \"re\"\"gion\" = 'O''Hare' AND \"units\" IS NOT NULL"
        );
    }

    #[test]
    fn aggregate_sql_is_ordered() {
        let k = StepKind::Aggregate {
            group_by: vec!["region".into()],
            measures: vec![
                Measure { func: AggFunc::Count, column: None, alias: "n".into() },
                Measure { func: AggFunc::Sum, column: Some("amount".into()), alias: "total".into() },
            ],
        };
        assert_eq!(
            step_sql(&k).unwrap(),
            "SELECT \"region\", COUNT(*) AS \"n\", SUM(\"amount\") AS \"total\" FROM input GROUP BY \"region\" ORDER BY \"region\" ASC NULLS LAST"
        );
    }

    #[test]
    fn validation_catches_bad_columns_and_types() {
        let bad_col = step(StepKind::Filter {
            predicates: vec![Predicate { column: "nope".into(), op: CmpOp::Eq, value: Value::Int(1) }],
        });
        assert!(matches!(output_schema(&bad_col, &[schema()]), Err(Error::Validation(_))));

        let bad_type = step(StepKind::Filter {
            predicates: vec![Predicate { column: "region".into(), op: CmpOp::Gt, value: Value::Int(1) }],
        });
        assert!(output_schema(&bad_type, &[schema()]).is_err());

        let eq_null = step(StepKind::Filter {
            predicates: vec![Predicate { column: "units".into(), op: CmpOp::Eq, value: Value::Null }],
        });
        let msg = output_schema(&eq_null, &[schema()]).unwrap_err().to_string();
        assert!(msg.contains("IS NULL"), "{msg}");

        let sum_text = step(StepKind::Aggregate {
            group_by: vec![],
            measures: vec![Measure { func: AggFunc::Sum, column: Some("region".into()), alias: "x".into() }],
        });
        assert!(output_schema(&sum_text, &[schema()]).is_err());
    }

    #[test]
    fn aggregate_output_schema() {
        let s = step(StepKind::Aggregate {
            group_by: vec!["region".into()],
            measures: vec![
                Measure { func: AggFunc::Avg, column: Some("units".into()), alias: "avg_units".into() },
                Measure { func: AggFunc::Max, column: Some("amount".into()), alias: "max_amount".into() },
            ],
        });
        let out = output_schema(&s, &[schema()]).unwrap().unwrap();
        let types: Vec<DataType> = out.fields.iter().map(|f| f.dtype).collect();
        assert_eq!(types, vec![DataType::Text, DataType::Float, DataType::Float]);
    }
}
