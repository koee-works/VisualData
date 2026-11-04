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

/// Format 2 (multi-input): `sources` is a list of `{id, path}` and each step
/// has `inputs`. Format 1 had one `source` and one `input` per step; it is
/// still read, and its source gets the id `source`.
pub const FORMAT_VERSION: f64 = 2.0;

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
fn strs(v: &[String]) -> Json {
    Json::Arr(v.iter().map(|s| Json::str(s.clone())).collect())
}

fn sort_keys(v: &[SortKey]) -> Json {
    Json::Arr(
        v.iter()
            .map(|k| Json::obj(vec![("column", Json::str(k.column.clone())), ("descending", Json::Bool(k.descending))]))
            .collect(),
    )
}

fn read_strs(j: &Json, key: &str) -> Result<Vec<String>, Error> {
    match j.get(key) {
        None | Some(Json::Null) => Ok(vec![]),
        Some(_) => j
            .req_arr(key)?
            .iter()
            .map(|s| s.as_str().map(str::to_string).ok_or_else(|| Error::Format(format!("bad {key}"))))
            .collect(),
    }
}

fn read_sort_keys(j: &Json, key: &str) -> Result<Vec<SortKey>, Error> {
    match j.get(key) {
        None | Some(Json::Null) => Ok(vec![]),
        Some(_) => j
            .req_arr(key)?
            .iter()
            .map(|k| Ok(SortKey { column: k.req_str("column")?.to_string(), descending: k.get("descending") == Some(&Json::Bool(true)) }))
            .collect(),
    }
}

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
                            let mut pairs = vec![
                                ("column", Json::str(p.column.clone())),
                                ("op", Json::str(p.op.token())),
                                ("value", value_to_json(&p.value)),
                            ];
                            if let Some(name) = &p.param {
                                pairs.push(("param", Json::str(name.clone())));
                            }
                            Json::obj(pairs)
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
                                ("func", Json::str(m.func.token())),
                                ("column", m.column.clone().map(Json::Str).unwrap_or(Json::Null)),
                                ("alias", Json::str(m.alias.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
        StepKind::Join { how, on } => Json::obj(vec![
            ("type", Json::str("join")),
            ("how", Json::str(how.name())),
            (
                "on",
                Json::Arr(
                    on.iter()
                        .map(|k| {
                            // `=` is the default and is left out, so plain key joins stay compact.
                            let mut pairs = vec![("left", Json::str(k.left.clone()))];
                            if k.op != CmpOp::Eq {
                                pairs.push(("op", Json::str(k.op.token())));
                            }
                            pairs.push(("right", Json::str(k.right.clone())));
                            Json::obj(pairs)
                        })
                        .collect(),
                ),
            ),
        ]),
        StepKind::Append { distinct, origin } => Json::obj(vec![
            ("type", Json::str("append")),
            ("distinct", Json::Bool(*distinct)),
            ("origin", origin.clone().map(Json::Str).unwrap_or(Json::Null)),
        ]),
        StepKind::Derive { columns } => Json::obj(vec![
            ("type", Json::str("derive")),
            (
                "columns",
                Json::Arr(
                    columns
                        .iter()
                        .map(|c| Json::obj(vec![("name", Json::str(c.name.clone())), ("formula", Json::str(c.formula.clone()))]))
                        .collect(),
                ),
            ),
        ]),
        StepKind::Select { columns } => Json::obj(vec![
            ("type", Json::str("select")),
            (
                "columns",
                Json::Arr(
                    columns
                        .iter()
                        .map(|c| {
                            Json::obj(vec![
                                ("column", Json::str(c.column.clone())),
                                ("as", c.rename.clone().map(Json::Str).unwrap_or(Json::Null)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
        StepKind::Sort { by, limit } => Json::obj(vec![
            ("type", Json::str("sort")),
            (
                "by",
                Json::Arr(
                    by.iter()
                        .map(|k| Json::obj(vec![("column", Json::str(k.column.clone())), ("descending", Json::Bool(k.descending))]))
                        .collect(),
                ),
            ),
            ("limit", limit.map(|n| Json::Num(n as f64)).unwrap_or(Json::Null)),
        ]),
        StepKind::Window { partition_by, order_by, columns } => Json::obj(vec![
            ("type", Json::str("window")),
            ("partition_by", strs(partition_by)),
            ("order_by", sort_keys(order_by)),
            (
                "columns",
                Json::Arr(
                    columns
                        .iter()
                        .map(|c| {
                            Json::obj(vec![
                                ("name", Json::str(c.name.clone())),
                                ("func", Json::str(c.func.name())),
                                ("column", c.column.clone().map(Json::Str).unwrap_or(Json::Null)),
                                ("n", c.n.map(|n| Json::Num(n as f64)).unwrap_or(Json::Null)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
        StepKind::Dedupe { keys, order_by } => {
            Json::obj(vec![("type", Json::str("dedupe")), ("keys", strs(keys)), ("order_by", sort_keys(order_by))])
        }
        StepKind::Pivot { rows, column, value, func } => Json::obj(vec![
            ("type", Json::str("pivot")),
            ("rows", strs(rows)),
            ("column", Json::str(column.clone())),
            ("value", value.clone().map(Json::Str).unwrap_or(Json::Null)),
            ("func", Json::str(func.token())),
        ]),
        StepKind::Unpivot { columns, name, value } => Json::obj(vec![
            ("type", Json::str("unpivot")),
            ("columns", strs(columns)),
            ("name", Json::str(name.clone())),
            ("value", Json::str(value.clone())),
        ]),
        StepKind::Test { test, a, b } => Json::obj(vec![
            ("type", Json::str("test")),
            ("test", Json::str(test.name())),
            ("a", Json::str(a.clone())),
            ("b", Json::str(b.clone())),
        ]),
        StepKind::Check { rules, stop } => Json::obj(vec![
            ("type", Json::str("check")),
            ("stop", Json::Bool(*stop)),
            (
                "rules",
                Json::Arr(
                    rules
                        .iter()
                        .map(|r| match r {
                            Rule::Expect { formula } => Json::obj(vec![("rule", Json::str("expect")), ("formula", Json::str(formula.clone()))]),
                            Rule::Unique { columns } => Json::obj(vec![("rule", Json::str("unique")), ("columns", strs(columns))]),
                            Rule::RowCount { min, max } => Json::obj(vec![
                                ("rule", Json::str("rows")),
                                ("min", min.map(|n| Json::Num(n as f64)).unwrap_or(Json::Null)),
                                ("max", max.map(|n| Json::Num(n as f64)).unwrap_or(Json::Null)),
                            ]),
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
        StepKind::R { code, timeout_ms } => Json::obj(vec![
            ("type", Json::str("r")),
            ("code", Json::str(code.clone())),
            ("timeout_ms", Json::Num(*timeout_ms as f64)),
        ]),
        StepKind::Chart { mark, x, y, title, color, summary } => {
            let mut pairs = vec![
                ("type", Json::str("chart")),
                ("mark", Json::str(mark.name())),
                ("x", Json::str(x.clone())),
                ("y", Json::str(y.clone())),
                ("title", title.clone().map(Json::Str).unwrap_or(Json::Null)),
            ];
            // Written only when set, so charts saved before keep their keys.
            if let Some(c) = color {
                pairs.push(("color", Json::str(c.clone())));
            }
            if let Some(f) = summary {
                pairs.push(("summary", Json::str(f.token())));
            }
            Json::obj(pairs)
        }
    }
}

/// Inverse of [`step_kind_to_json`]; also the IPC format for steps.
pub fn step_kind_from_json(j: &Json) -> Result<StepKind, Error> {
    let opt_str = |k: &str| j.get(k).and_then(Json::as_str).map(str::to_string);
    Ok(match j.req_str("type")? {
        "filter" => StepKind::Filter {
            predicates: j
                .req_arr("predicates")?
                .iter()
                .map(|p| {
                    Ok(Predicate {
                        param: p.get("param").and_then(Json::as_str).map(str::to_string),
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
                        func: AggFunc::parse(m.req_str("func")?)
                            .ok_or_else(|| Error::Format("unknown aggregate".into()))?,
                        column: m.get("column").and_then(Json::as_str).map(str::to_string),
                        alias: m.req_str("alias")?.to_string(),
                    })
                })
                .collect::<Result<_, Error>>()?,
        },
        "join" => StepKind::Join {
            how: JoinKind::parse(j.req_str("how")?).ok_or_else(|| Error::Format("unknown join type".into()))?,
            on: j
                .req_arr("on")?
                .iter()
                .map(|k| {
                    let op = match k.get("op").and_then(Json::as_str) {
                        None => CmpOp::Eq,
                        Some(t) => CmpOp::from_token(t).ok_or_else(|| Error::Format(format!("unknown join operator `{t}`")))?,
                    };
                    Ok(JoinKey { left: k.req_str("left")?.to_string(), op, right: k.req_str("right")?.to_string() })
                })
                .collect::<Result<_, Error>>()?,
        },
        "append" => StepKind::Append {
            distinct: j.get("distinct") == Some(&Json::Bool(true)),
            origin: opt_str("origin").filter(|o| !o.is_empty()),
        },
        "derive" => StepKind::Derive {
            columns: j
                .req_arr("columns")?
                .iter()
                .map(|c| Ok(Derived { name: c.req_str("name")?.to_string(), formula: c.req_str("formula")?.to_string() }))
                .collect::<Result<_, Error>>()?,
        },
        "select" => StepKind::Select {
            columns: j
                .req_arr("columns")?
                .iter()
                .map(|c| {
                    Ok(Pick {
                        column: c.req_str("column")?.to_string(),
                        rename: c.get("as").and_then(Json::as_str).filter(|s| !s.is_empty()).map(str::to_string),
                    })
                })
                .collect::<Result<_, Error>>()?,
        },
        "sort" => StepKind::Sort {
            by: j
                .req_arr("by")?
                .iter()
                .map(|k| {
                    Ok(SortKey { column: k.req_str("column")?.to_string(), descending: k.get("descending") == Some(&Json::Bool(true)) })
                })
                .collect::<Result<_, Error>>()?,
            limit: match j.get("limit").and_then(Json::as_f64) {
                Some(n) if n >= 0.0 && n.fract() == 0.0 => Some(n as u64),
                Some(_) => return Err(Error::Format("limit must be a whole number".into())),
                None => None,
            },
        },
        "window" => StepKind::Window {
            partition_by: read_strs(j, "partition_by")?,
            order_by: read_sort_keys(j, "order_by")?,
            columns: j
                .req_arr("columns")?
                .iter()
                .map(|c| {
                    Ok(WindowColumn {
                        name: c.req_str("name")?.to_string(),
                        func: WindowFunc::parse(c.req_str("func")?).ok_or_else(|| Error::Format("unknown window function".into()))?,
                        column: c.get("column").and_then(Json::as_str).map(str::to_string),
                        n: c.get("n").and_then(Json::as_f64).map(|n| n as u32),
                    })
                })
                .collect::<Result<_, Error>>()?,
        },
        "dedupe" => StepKind::Dedupe { keys: read_strs(j, "keys")?, order_by: read_sort_keys(j, "order_by")? },
        "pivot" => StepKind::Pivot {
            rows: read_strs(j, "rows")?,
            column: j.req_str("column")?.to_string(),
            value: opt_str("value"),
            func: AggFunc::parse(j.req_str("func")?).ok_or_else(|| Error::Format("unknown aggregate".into()))?,
        },
        "unpivot" => StepKind::Unpivot {
            columns: read_strs(j, "columns")?,
            name: j.req_str("name")?.to_string(),
            value: j.req_str("value")?.to_string(),
        },
        "test" => StepKind::Test {
            test: crate::model::TestKind::parse(j.req_str("test")?).ok_or_else(|| Error::Format("unknown statistical test".into()))?,
            a: j.req_str("a")?.to_string(),
            b: j.req_str("b")?.to_string(),
        },
        "check" => StepKind::Check {
            stop: j.get("stop") == Some(&Json::Bool(true)),
            rules: j
                .req_arr("rules")?
                .iter()
                .map(|r| {
                    let n = |k: &str| r.get(k).and_then(Json::as_f64).map(|x| x as u64);
                    Ok(match r.req_str("rule")? {
                        "expect" => Rule::Expect { formula: r.req_str("formula")?.to_string() },
                        "unique" => Rule::Unique { columns: read_strs(r, "columns")? },
                        "rows" => Rule::RowCount { min: n("min"), max: n("max") },
                        other => return Err(Error::Format(format!("unknown rule `{other}`"))),
                    })
                })
                .collect::<Result<_, Error>>()?,
        },
        "sql" => StepKind::Sql { sql: j.req_str("sql")?.to_string() },
        "python" => StepKind::Python {
            code: j.req_str("code")?.to_string(),
            timeout_ms: j.get("timeout_ms").and_then(Json::as_f64).unwrap_or(30_000.0) as u64,
        },
        "r" => StepKind::R {
            code: j.req_str("code")?.to_string(),
            timeout_ms: j.get("timeout_ms").and_then(Json::as_f64).unwrap_or(30_000.0) as u64,
        },
        "chart" => StepKind::Chart {
            mark: Mark::parse(j.req_str("mark")?).ok_or_else(|| Error::Format("unknown mark".into()))?,
            x: j.req_str("x")?.to_string(),
            y: j.req_str("y")?.to_string(),
            title: opt_str("title"),
            color: opt_str("color"),
            summary: match opt_str("summary") {
                Some(f) => Some(AggFunc::parse(&f).ok_or_else(|| Error::Format(format!("unknown summary `{f}`")))?),
                None => None,
            },
        },
        other => return Err(Error::Format(format!("unknown step type `{other}`"))),
    })
}

/// `[{"name": …, "value": …}, …]`, values as in step definitions.
pub fn parameters_to_json(params: &[crate::params::Parameter]) -> Json {
    Json::Arr(
        params
            .iter()
            .map(|p| Json::obj(vec![("name", Json::str(p.name.clone())), ("value", value_to_json(&p.value))]))
            .collect(),
    )
}

pub fn parameters_from_json(j: &Json) -> Result<Vec<crate::params::Parameter>, Error> {
    match j {
        Json::Null => Ok(vec![]),
        Json::Arr(items) => items
            .iter()
            .map(|p| {
                Ok(crate::params::Parameter {
                    name: p.req_str("name")?.to_string(),
                    value: value_from_json(p.get("value").unwrap_or(&Json::Null))?,
                })
            })
            .collect(),
        _ => Err(Error::Format("parameters must be a list".into())),
    }
}

pub fn analysis_to_json(a: &Analysis) -> Json {
    Json::obj(vec![
        ("format_version", Json::Num(FORMAT_VERSION)),
        ("name", Json::str(a.name.clone())),
        (
            "sources",
            Json::Arr(
                a.sources
                    .iter()
                    .map(|src| {
                        let mut pairs = vec![
                            ("id", Json::str(src.id.clone())),
                            ("path", Json::str(src.path.clone())),
                            ("format", Json::str(src.format.name())),
                        ];
                        if let Some(sheet) = &src.sheet {
                            pairs.push((if src.format.is_database() { "table" } else { "sheet" }, Json::str(sheet.clone())));
                        }
                        if let Some(q) = &src.query {
                            pairs.push(("query", Json::str(q.clone())));
                        }
                        Json::obj(pairs)
                    })
                    .collect(),
            ),
        ),
        (
            "steps",
            Json::Arr(
                a.steps
                    .iter()
                    .map(|s| {
                        Json::obj(vec![
                            ("id", Json::str(s.id.clone())),
                            ("inputs", Json::Arr(s.inputs.iter().map(|i| Json::str(i.clone())).collect())),
                            ("label", s.label.clone().map(Json::Str).unwrap_or(Json::Null)),
                            ("op", step_kind_to_json(&s.kind)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("parameters", parameters_to_json(&a.parameters)),
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
    let v1 = version.trunc() == 1.0;
    if !v1 && version.trunc() != FORMAT_VERSION.trunc() {
        return Err(Error::Format(format!(
            "unsupported analysis format version {version}; this app reads versions 1 and 2"
        )));
    }
    let sources = if v1 {
        let source = j.get("source").ok_or_else(|| Error::Format("missing source".into()))?;
        vec![Source { format: SourceFormat::Csv, ..Source::primary(source.req_str("path")?) }]
    } else {
        j.req_arr("sources")?
            .iter()
            .map(|s| {
                let format = match s.get("format").and_then(Json::as_str) {
                    None => SourceFormat::Csv,
                    Some(f) => SourceFormat::parse(f).ok_or_else(|| Error::Format(format!("unknown source format `{f}`")))?,
                };
                Ok(Source {
                    id: s.req_str("id")?.to_string(),
                    path: s.req_str("path")?.to_string(),
                    format,
                    sheet: s.get("sheet").or_else(|| s.get("table")).and_then(Json::as_str).map(str::to_string),
                    query: s.get("query").and_then(Json::as_str).map(str::to_string),
                })
            })
            .collect::<Result<_, Error>>()?
    };
    let parameters = parameters_from_json(j.get("parameters").unwrap_or(&Json::Null))?;
    Ok(Analysis {
        parameters,
        name: j.req_str("name")?.to_string(),
        sources,
        steps: j
            .req_arr("steps")?
            .iter()
            .map(|s| {
                Ok(Step {
                    id: s.req_str("id")?.to_string(),
                    inputs: if v1 {
                        vec![s.req_str("input")?.to_string()]
                    } else {
                        s.req_arr("inputs")?
                            .iter()
                            .map(|i| i.as_str().map(str::to_string).ok_or_else(|| Error::Format("bad inputs".into())))
                            .collect::<Result<_, Error>>()?
                    },
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
