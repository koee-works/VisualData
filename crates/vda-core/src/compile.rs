//! Compile visual operations to DuckDB SQL, validate them against an input
//! schema, and build the inspectable SQL for a chain of steps.

use crate::model::{AggFunc, Analysis, Step, StepKind, SOURCE_ID};
use crate::table::{DataType, Field, Schema, Value};
use crate::Error;

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
    }
}

/// SQL for a single relational step, reading from the relation `input`.
/// Returns `None` for steps that do not execute as SQL (Python, Chart).
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
        StepKind::Python { .. } | StepKind::Chart { .. } => None,
    }
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
        _ => false,
    }
}

/// Validate a visual step against its actual input schema and return the
/// output schema. Custom SQL/Python return `Ok(None)`: their schema is only
/// known after they run.
pub fn output_schema(step: &Step, input: &Schema) -> Result<Option<Schema>, Error> {
    let id = &step.id;
    match &step.kind {
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

/// Inspectable SQL for a step: the chain of SQL-executable steps back to the
/// source (or to the nearest Python step), as one query with CTEs.
pub fn pipeline_sql(analysis: &Analysis, step_id: &str) -> Result<String, Error> {
    let mut chain: Vec<&Step> = Vec::new();
    let mut cursor = step_id;
    let base: String;
    loop {
        if cursor == SOURCE_ID {
            base = format!(
                "SELECT * FROM read_csv_auto({})",
                quote_literal(&Value::Text(analysis.source.path.clone()))
            );
            break;
        }
        let step = analysis
            .step(cursor)
            .ok_or_else(|| Error::Validation(format!("unknown step `{cursor}`")))?;
        if let StepKind::Python { .. } = step.kind {
            if chain.is_empty() {
                return Err(Error::Unsupported(format!(
                    "step `{}` is custom Python; it has no SQL form",
                    step.id
                )));
            }
            base = format!(
                "SELECT * FROM {} /* materialized output of Python step `{}` */",
                quote_ident(&format!("{}_output", step.id)),
                step.id
            );
            break;
        }
        if let StepKind::Chart { .. } = step.kind {
            return Err(Error::Unsupported(format!("step `{}` is a chart; it has no SQL form", step.id)));
        }
        chain.push(step);
        cursor = &step.input;
    }
    chain.reverse();
    let mut ctes = vec![format!("input_0 AS ({base})")];
    for (i, step) in chain.iter().enumerate() {
        let body = step_sql(&step.kind).expect("chain holds only SQL steps");
        // Each step reads the previous CTE as `input`. The body is wrapped as a
        // subquery so custom SQL with its own WITH clause stays valid.
        let body = format!("WITH input AS (SELECT * FROM input_{i}) SELECT * FROM ({body})");
        let label = if step.kind.is_visual() { "visual" } else { "custom SQL" };
        ctes.push(format!(
            "-- step `{}` ({}, {label})\ninput_{} AS ({body})",
            step.id,
            step.kind.type_name(),
            i + 1
        ));
    }
    Ok(format!("WITH\n{}\nSELECT * FROM input_{}", ctes.join(",\n"), chain.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CmpOp, Measure, Predicate};

    fn schema() -> Schema {
        Schema::new(vec![("region", DataType::Text), ("amount", DataType::Float), ("units", DataType::Int)])
    }

    fn step(kind: StepKind) -> Step {
        Step { id: "s".into(), input: SOURCE_ID.into(), label: None, kind }
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
        assert!(matches!(output_schema(&bad_col, &schema()), Err(Error::Validation(_))));

        let bad_type = step(StepKind::Filter {
            predicates: vec![Predicate { column: "region".into(), op: CmpOp::Gt, value: Value::Int(1) }],
        });
        assert!(output_schema(&bad_type, &schema()).is_err());

        let eq_null = step(StepKind::Filter {
            predicates: vec![Predicate { column: "units".into(), op: CmpOp::Eq, value: Value::Null }],
        });
        let msg = output_schema(&eq_null, &schema()).unwrap_err().to_string();
        assert!(msg.contains("IS NULL"), "{msg}");

        let sum_text = step(StepKind::Aggregate {
            group_by: vec![],
            measures: vec![Measure { func: AggFunc::Sum, column: Some("region".into()), alias: "x".into() }],
        });
        assert!(output_schema(&sum_text, &schema()).is_err());
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
        let out = output_schema(&s, &schema()).unwrap().unwrap();
        let types: Vec<DataType> = out.fields.iter().map(|f| f.dtype).collect();
        assert_eq!(types, vec![DataType::Text, DataType::Float, DataType::Float]);
    }
}
