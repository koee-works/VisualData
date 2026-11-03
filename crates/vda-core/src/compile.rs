//! Compile visual operations to DuckDB SQL, validate them against an input
//! schema, and build the inspectable SQL for a chain of steps.

use crate::model::{AggFunc, Analysis, CmpOp, JoinKind, SourceFormat, Step, StepKind};
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
                cols.push(format!("{}({}) AS {}", m.func.sql(), arg, quote_ident(&m.alias)));
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
        StepKind::Sql { sql } => Some(sql.trim().trim_end_matches(';').to_string()),
        StepKind::Join { .. } | StepKind::Append { .. } | StepKind::Python { .. } | StepKind::Chart { .. } => None,
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
        kind => Ok(step_sql(kind)),
    }
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
        StepKind::Aggregate { group_by, measures } => {
            let mut fields = Vec::new();
            for g in group_by {
                fields.push(require(input, g, id)?.clone());
            }
            if measures.is_empty() && group_by.is_empty() {
                return Err(Error::Validation(format!("step `{id}`: aggregate needs a group or a measure")));
            }
            for m in measures {
                let dtype = match (m.func, &m.column) {
                    (AggFunc::Count, _) => DataType::Int,
                    (_, None) => {
                        return Err(Error::Validation(format!(
                            "step `{id}`: {} needs a column",
                            m.func.sql()
                        )))
                    }
                    (func, Some(c)) => {
                        let f = require(input, c, id)?;
                        match func {
                            AggFunc::Sum | AggFunc::Avg if !f.dtype.is_numeric() => {
                                return Err(Error::Validation(format!(
                                    "step `{id}`: {} needs a numeric column, `{c}` is {}",
                                    func.sql(),
                                    f.dtype.name()
                                )))
                            }
                            AggFunc::Avg => DataType::Float,
                            _ => f.dtype,
                        }
                    }
                };
                if fields.iter().any(|f: &Field| f.name == m.alias) {
                    return Err(Error::Validation(format!("step `{id}`: duplicate output column `{}`", m.alias)));
                }
                fields.push(Field { name: m.alias.clone(), dtype });
            }
            Ok(Some(Schema { fields }))
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
            (StepKind::Join { .. } | StepKind::Append { .. }, None) => Err(Error::Unsupported(format!(
                "step `{}` combines tables; its SQL is shown once its inputs have run",
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
