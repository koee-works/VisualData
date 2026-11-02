//! On-disk format for analyses (`analysis.json`).
//!
//! This format is the durable contract between the core, the desktop app and
//! future tooling. Bump `FORMAT_VERSION` on any incompatible change.

use crate::json::Json;
use crate::model::*;
use crate::table::Value;
use crate::Error;
use std::fs;
use std::path::Path;

pub const FORMAT_VERSION: f64 = 1.0;

pub fn value_to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        // Integers are strings so values beyond 2^53 survive the round trip.
        Value::Int(i) => Json::obj(vec![("int", Json::str(i.to_string()))]),
        Value::Float(f) => Json::obj(vec![("float", Json::Num(*f))]),
        Value::Text(s) => Json::str(s.clone()),
        Value::Date(_) => Json::obj(vec![("date", Json::str(v.to_string()))]),
        Value::Timestamp(_) => Json::obj(vec![("timestamp", Json::str(v.to_string()))]),
    }
}

pub fn value_from_json(j: &Json) -> Result<Value, Error> {
    Ok(match j {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Str(s) => Value::Text(s.clone()),
        Json::Obj(_) => {
            if let Some(s) = j.get("int").and_then(Json::as_str) {
                Value::Int(s.parse().map_err(|_| Error::Format(format!("bad int `{s}`")))?)
            } else if let Some(f) = j.get("float").and_then(Json::as_f64) {
                Value::Float(f)
            } else if let Some(d) = j.get("date").and_then(Json::as_str) {
                Value::Date(crate::table::parse_date(d).ok_or_else(|| Error::Format(format!("bad date `{d}`")))?)
            } else if let Some(t) = j.get("timestamp").and_then(Json::as_str) {
                Value::Timestamp(
                    crate::table::parse_timestamp(t).ok_or_else(|| Error::Format(format!("bad timestamp `{t}`")))?,
                )
            } else {
                return Err(Error::Format("unknown value object".into()));
            }
        }
        _ => return Err(Error::Format("unsupported value".into())),
    })
}

/// The executable meaning of a step, excluding display-only fields. Used for
/// cache keys, so a rename never invalidates results.
pub fn step_kind_to_json(kind: &StepKind) -> Json {
    match kind {
        StepKind::Filter { predicates } => Json::obj(vec![
            ("type", Json::str("filter")),
            (
                "predicates",
                Json::Arr(
                    predicates
                        .iter()
                        .map(|p| {
                            Json::obj(vec![
                                ("column", Json::str(p.column.clone())),
                                ("op", Json::str(p.op.token())),
                                ("value", value_to_json(&p.value)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
        StepKind::Aggregate { group_by, measures } => Json::obj(vec![
            ("type", Json::str("aggregate")),
            ("group_by", Json::Arr(group_by.iter().map(|g| Json::str(g.clone())).collect())),
            (
                "measures",
                Json::Arr(
                    measures
                        .iter()
                        .map(|m| {
                            Json::obj(vec![
                                ("func", Json::str(m.func.sql())),
                                ("column", m.column.clone().map(Json::Str).unwrap_or(Json::Null)),
                                ("alias", Json::str(m.alias.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
        StepKind::Sql { sql } => Json::obj(vec![("type", Json::str("sql")), ("sql", Json::str(sql.clone()))]),
        StepKind::Python { code, timeout_ms } => Json::obj(vec![
            ("type", Json::str("python")),
            ("code", Json::str(code.clone())),
            ("timeout_ms", Json::Num(*timeout_ms as f64)),
        ]),
        StepKind::Chart { mark, x, y, title } => Json::obj(vec![
            ("type", Json::str("chart")),
            ("mark", Json::str(mark.name())),
            ("x", Json::str(x.clone())),
            ("y", Json::str(y.clone())),
            ("title", title.clone().map(Json::Str).unwrap_or(Json::Null)),
        ]),
    }
}

fn step_kind_from_json(j: &Json) -> Result<StepKind, Error> {
    let opt_str = |k: &str| j.get(k).and_then(Json::as_str).map(str::to_string);
    Ok(match j.req_str("type")? {
        "filter" => StepKind::Filter {
            predicates: j
                .req_arr("predicates")?
                .iter()
                .map(|p| {
                    Ok(Predicate {
                        column: p.req_str("column")?.to_string(),
                        op: CmpOp::from_token(p.req_str("op")?)
                            .ok_or_else(|| Error::Format("unknown operator".into()))?,
                        value: value_from_json(p.get("value").unwrap_or(&Json::Null))?,
                    })
                })
                .collect::<Result<_, Error>>()?,
        },
        "aggregate" => StepKind::Aggregate {
            group_by: j
                .req_arr("group_by")?
                .iter()
                .map(|g| g.as_str().map(str::to_string).ok_or_else(|| Error::Format("bad group_by".into())))
                .collect::<Result<_, Error>>()?,
            measures: j
                .req_arr("measures")?
                .iter()
                .map(|m| {
                    Ok(Measure {
                        func: AggFunc::from_sql(m.req_str("func")?)
                            .ok_or_else(|| Error::Format("unknown aggregate".into()))?,
                        column: m.get("column").and_then(Json::as_str).map(str::to_string),
                        alias: m.req_str("alias")?.to_string(),
                    })
                })
                .collect::<Result<_, Error>>()?,
        },
        "sql" => StepKind::Sql { sql: j.req_str("sql")?.to_string() },
        "python" => StepKind::Python {
            code: j.req_str("code")?.to_string(),
            timeout_ms: j.get("timeout_ms").and_then(Json::as_f64).unwrap_or(30_000.0) as u64,
        },
        "chart" => StepKind::Chart {
            mark: Mark::parse(j.req_str("mark")?).ok_or_else(|| Error::Format("unknown mark".into()))?,
            x: j.req_str("x")?.to_string(),
            y: j.req_str("y")?.to_string(),
            title: opt_str("title"),
        },
        other => return Err(Error::Format(format!("unknown step type `{other}`"))),
    })
}

pub fn analysis_to_json(a: &Analysis) -> Json {
    Json::obj(vec![
        ("format_version", Json::Num(FORMAT_VERSION)),
        ("name", Json::str(a.name.clone())),
        ("source", Json::obj(vec![("path", Json::str(a.source.path.clone())), ("format", Json::str("csv"))])),
        (
            "steps",
            Json::Arr(
                a.steps
                    .iter()
                    .map(|s| {
                        Json::obj(vec![
                            ("id", Json::str(s.id.clone())),
                            ("input", Json::str(s.input.clone())),
                            ("label", s.label.clone().map(Json::Str).unwrap_or(Json::Null)),
                            ("op", step_kind_to_json(&s.kind)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "cells",
            Json::Arr(
                a.cells
                    .iter()
                    .map(|c| match c {
                        Cell::Markdown { text } => {
                            Json::obj(vec![("type", Json::str("markdown")), ("text", Json::str(text.clone()))])
                        }
                        Cell::Step { step_id } => {
                            Json::obj(vec![("type", Json::str("step")), ("step", Json::str(step_id.clone()))])
                        }
                    })
                    .collect(),
            ),
        ),
    ])
}

pub fn analysis_from_json(j: &Json) -> Result<Analysis, Error> {
    let version = j.get("format_version").and_then(Json::as_f64).unwrap_or(0.0);
    if version.trunc() != FORMAT_VERSION.trunc() {
        return Err(Error::Format(format!("unsupported analysis format version {version}")));
    }
    let source = j.get("source").ok_or_else(|| Error::Format("missing source".into()))?;
    Ok(Analysis {
        name: j.req_str("name")?.to_string(),
        source: Source { path: source.req_str("path")?.to_string() },
        steps: j
            .req_arr("steps")?
            .iter()
            .map(|s| {
                Ok(Step {
                    id: s.req_str("id")?.to_string(),
                    input: s.req_str("input")?.to_string(),
                    label: s.get("label").and_then(Json::as_str).map(str::to_string),
                    kind: step_kind_from_json(s.get("op").ok_or_else(|| Error::Format("missing op".into()))?)?,
                })
            })
            .collect::<Result<_, Error>>()?,
        cells: j
            .req_arr("cells")?
            .iter()
            .map(|c| {
                Ok(match c.req_str("type")? {
                    "markdown" => Cell::Markdown { text: c.req_str("text")?.to_string() },
                    "step" => Cell::Step { step_id: c.req_str("step")?.to_string() },
                    other => return Err(Error::Format(format!("unknown cell type `{other}`"))),
                })
            })
            .collect::<Result<_, Error>>()?,
    })
}

pub fn save_analysis(a: &Analysis, project_dir: &Path) -> Result<(), Error> {
    fs::create_dir_all(project_dir)?;
    let tmp = project_dir.join("analysis.json.tmp");
    fs::write(&tmp, analysis_to_json(a).to_string_pretty())?;
    fs::rename(tmp, project_dir.join("analysis.json"))?; // atomic replace
    Ok(())
}

pub fn load_analysis(project_dir: &Path) -> Result<Analysis, Error> {
    let text = fs::read_to_string(project_dir.join("analysis.json"))?;
    analysis_from_json(&Json::parse(&text)?)
}
