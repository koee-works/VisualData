//! Chart steps produce a Vega-Lite v5 specification with the data inlined.
//!
//! Anything a chart summarizes (a sum per bar, histogram counts, box-plot
//! quartiles) is computed by the engine over the whole input, by running the
//! same visual steps users can add themselves (aggregate, calculate). Only
//! the summarized rows are inlined, at most [`MAX_CHART_ROWS`] of them, so
//! a chart of 50 million rows shows totals of all of them, not of a sample.

use crate::compile::require_column;
use crate::dataset::Dataset;
use crate::engine::Engine;
use crate::json::Json;
use crate::model::{AggFunc, Derived, Mark, Measure, Step, StepKind};
use crate::persist::value_to_json;
use crate::table::{DataType, Schema, Table, Value};
use crate::Error;
use std::path::PathBuf;

/// Rows above this are not inlined; the chart is labelled as partial.
pub const MAX_CHART_ROWS: usize = 5_000;
/// Bars a histogram aims for.
pub const HISTOGRAM_BINS: f64 = 20.0;

/// The parts of a chart step.
pub struct ChartDef<'a> {
    pub mark: Mark,
    pub x: &'a str,
    pub y: &'a str,
    pub color: Option<&'a str>,
    pub summary: Option<AggFunc>,
    pub title: Option<&'a str>,
}

impl<'a> ChartDef<'a> {
    pub fn of(kind: &'a StepKind) -> ChartDef<'a> {
        let StepKind::Chart { mark, x, y, title, color, summary } = kind else { unreachable!("not a chart") };
        ChartDef { mark: *mark, x, y, color: color.as_deref(), summary: *summary, title: title.as_deref() }
    }
}

fn encoding_type(t: DataType) -> &'static str {
    if t.is_numeric() {
        "quantitative"
    } else if t.is_temporal() {
        "temporal"
    } else {
        "nominal"
    }
}

fn plain(v: &Value) -> Json {
    match v {
        Value::Int(i) => Json::Num(*i as f64),
        Value::Float(f) => Json::Num(*f),
        // ISO strings, which Vega-Lite parses for temporal encodings.
        Value::Date(_) => Json::str(v.to_string()),
        Value::Timestamp(_) => Json::str(v.to_string().replace(' ', "T")),
        other => value_to_json(other),
    }
}

fn err(id: &str, m: String) -> Error {
    Error::Validation(format!("step `{id}`: {m}"))
}

/// Check a chart against its input's columns.
pub fn validate(step: &Step, input: &Schema) -> Result<(), Error> {
    let c = ChartDef::of(&step.kind);
    let id = &step.id;
    let numeric = |name: &str, what: &str| -> Result<(), Error> {
        let f = require_column(input, name, id)?;
        if f.dtype.is_numeric() {
            Ok(())
        } else {
            Err(err(id, format!("{what} needs a numeric column; `{name}` is {}", f.dtype.plain())))
        }
    };
    match c.mark {
        Mark::Histogram => {
            numeric(c.x, "a histogram")?;
            if c.color.is_some() || c.summary.is_some() {
                return Err(err(id, "a histogram counts rows of one column; it has no color or summary".into()));
            }
        }
        Mark::Boxplot => {
            numeric(c.y, "a box plot")?;
            if !c.x.is_empty() {
                require_column(input, c.x, id)?;
            }
            if c.color.is_some() || c.summary.is_some() {
                return Err(err(id, "a box plot shows a column's spread per group; it has no color or summary".into()));
            }
        }
        _ => {
            if c.x.is_empty() {
                return Err(err(id, "choose the column along the bottom".into()));
            }
            require_column(input, c.x, id)?;
            match c.summary {
                Some(AggFunc::Count) => {}
                Some(f) => {
                    if c.y.is_empty() {
                        return Err(err(id, format!("choose the column to {}", f.token().to_lowercase())));
                    }
                    crate::compile::measure_type(id, f, Some(c.y), input)?;
                }
                None => {
                    if c.y.is_empty() {
                        return Err(err(id, "choose the column going up".into()));
                    }
                    require_column(input, c.y, id)?;
                }
            }
            if let Some(col) = c.color {
                require_column(input, col, id)?;
                if col == c.x {
                    return Err(err(id, format!("`{col}` is already along the bottom; color by another column")));
                }
            }
        }
    }
    Ok(())
}

/// The summarized data a chart shows, and how many rows it had in all.
pub struct ChartData {
    pub table: Table,
    pub rows: u64,
    /// Histogram bar width.
    pub width: Option<f64>,
    /// Rows in the chart's input.
    pub input_rows: u64,
}

/// A round bin width giving about [`HISTOGRAM_BINS`] bars over `range`.
pub fn bin_width(min: f64, max: f64, integer: bool) -> f64 {
    let range = max - min;
    if range.is_nan() || range <= 0.0 {
        return 1.0;
    }
    let raw = range / HISTOGRAM_BINS;
    let mag = 10f64.powf(raw.log10().floor());
    let w = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * mag).find(|w| *w >= raw * 0.999).unwrap_or(10.0 * mag);
    if integer { w.max(1.0).ceil() } else { w }
}

/// Compute what the chart shows, running summaries through `engine`.
pub fn data(engine: &mut dyn Engine, step: &Step, input: &Dataset, scratch: &mut dyn FnMut() -> PathBuf) -> Result<ChartData, Error> {
    let c = ChartDef::of(&step.kind);
    let mut made: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<ChartData, Error> {
        let mut run = |engine: &mut dyn Engine, kind: StepKind, from: &Dataset| -> Result<Dataset, Error> {
            let s = Step { id: step.id.clone(), inputs: vec![step.input().to_string()], label: None, kind };
            let out = scratch();
            made.push(out.clone());
            engine.run_step(&s, std::slice::from_ref(from), &out)
        };
        let agg = |group_by: Vec<String>, measures: Vec<Measure>| StepKind::Aggregate { group_by, measures };
        let m = |func: AggFunc, col: Option<&str>, alias: &str| Measure { func, column: col.map(str::to_string), alias: alias.into() };
        let (ds, width) = match (c.mark, c.summary) {
            (Mark::Histogram, _) => {
                let range = run(engine, agg(vec![], vec![m(AggFunc::Min, Some(c.x), "lo"), m(AggFunc::Max, Some(c.x), "hi")]), input)?;
                let t = engine.preview(&range, 0, 1)?;
                let (lo, hi) = (t.rows[0][0].as_f64(), t.rows[0][1].as_f64());
                let integer = input.schema.field(c.x).is_some_and(|f| f.dtype == DataType::Int);
                let w = match (lo, hi) {
                    (Some(lo), Some(hi)) => bin_width(lo, hi, integer),
                    _ => 1.0,
                };
                let width = if integer { format!("{}", w as i64) } else { format!("{w:?}") };
                let col = format!("[{}]", c.x.replace(']', ""));
                let binned = run(
                    engine,
                    StepKind::Derive { columns: vec![Derived { name: "__vda_bin".into(), formula: format!("bucket({col}, {width})") }] },
                    input,
                )?;
                let counted = run(engine, agg(vec!["__vda_bin".into()], vec![m(AggFunc::Count, None, "rows")]), &binned)?;
                (counted, Some(w))
            }
            (Mark::Boxplot, _) => {
                let group: Vec<String> = if c.x.is_empty() { vec![] } else { vec![c.x.to_string()] };
                let stats = vec![
                    m(AggFunc::Min, Some(c.y), "min"),
                    m(AggFunc::Percentile(25), Some(c.y), "q1"),
                    m(AggFunc::Median, Some(c.y), "median"),
                    m(AggFunc::Percentile(75), Some(c.y), "q3"),
                    m(AggFunc::Max, Some(c.y), "max"),
                    m(AggFunc::Count, Some(c.y), "values"),
                ];
                (run(engine, agg(group, stats), input)?, None)
            }
            (_, Some(f)) => {
                let mut group = vec![c.x.to_string()];
                group.extend(c.color.map(str::to_string));
                let alias = y_field(&c);
                let col = if f == AggFunc::Count { None } else { Some(c.y) };
                (run(engine, agg(group, vec![m(f, col, &alias)]), input)?, None)
            }
            (_, None) => {
                let mut cols = vec![c.x, c.y];
                cols.extend(c.color);
                let table = engine.columns(input, &cols, MAX_CHART_ROWS as u64)?;
                return Ok(ChartData { table, rows: input.rows, width: None, input_rows: input.rows });
            }
        };
        let table = engine.preview(&ds, 0, MAX_CHART_ROWS as u64)?;
        Ok(ChartData { table, rows: ds.rows, width, input_rows: input.rows })
    })();
    for p in made {
        let _ = std::fs::remove_file(&p);
    }
    result
}

/// The field holding the vertical value.
fn y_field(c: &ChartDef) -> String {
    match c.summary {
        Some(AggFunc::Count) => "rows".into(),
        Some(f) => format!("{} of {}", crate::model::agg_words(f), c.y),
        None => c.y.to_string(),
    }
}

/// The Vega-Lite specification for a chart and its data.
pub fn vega_lite(kind: &StepKind, d: &ChartData) -> Result<Json, Error> {
    let c = ChartDef::of(kind);
    let t = &d.table;
    let values: Vec<Json> = t
        .rows
        .iter()
        .map(|r| Json::Obj(t.schema.fields.iter().zip(r).map(|(f, v)| (f.name.clone(), plain(v))).collect()))
        .collect();
    let dtype = |n: &str| t.schema.field(n).map(|f| f.dtype).unwrap_or(DataType::Text);
    let enc = |field: &str, title: &str, ty: &str| {
        Json::obj(vec![("field", Json::str(field)), ("type", Json::str(ty)), ("title", Json::str(title))])
    };
    let mut title = match c.title {
        Some(t) => t.to_string(),
        None => match c.mark {
            Mark::Histogram => format!("Distribution of {}", c.x),
            Mark::Boxplot if c.x.is_empty() => format!("Spread of {}", c.y),
            Mark::Boxplot => format!("Spread of {} by {}", c.y, c.x),
            _ => {
                let y = y_field(&c);
                let y = if c.summary.is_some() { capitalize(&y) } else { y };
                match c.color {
                    Some(col) => format!("{y} by {} and {col}", c.x),
                    None => format!("{y} by {}", c.x),
                }
            }
        },
    };
    let what = if c.summary.is_some() || matches!(c.mark, Mark::Histogram | Mark::Boxplot) { "groups" } else { "rows" };
    if d.rows > MAX_CHART_ROWS as u64 {
        title.push_str(&format!(" (first {MAX_CHART_ROWS} of {} {what})", d.rows));
    }
    let head = |mark: Json, encoding: Json| {
        Json::obj(vec![
            ("$schema", Json::str("https://vega.github.io/schema/vega-lite/v5.json")),
            ("title", Json::str(title.clone())),
            ("data", Json::obj(vec![("values", Json::Arr(values.clone()))])),
            ("mark", mark),
            ("encoding", encoding),
        ])
    };
    let mark = |m: &str| Json::obj(vec![("type", Json::str(m)), ("tooltip", Json::Bool(true))]);
    Ok(match c.mark {
        Mark::Histogram => {
            let w = d.width.unwrap_or(1.0);
            let values: Vec<Json> = t
                .rows
                .iter()
                .filter(|r| !r[0].is_null())
                .map(|r| {
                    let start = r[0].as_f64().unwrap_or(0.0);
                    Json::obj(vec![("start", Json::Num(start)), ("end", Json::Num(start + w)), ("rows", plain(&r[1]))])
                })
                .collect();
            Json::obj(vec![
                ("$schema", Json::str("https://vega.github.io/schema/vega-lite/v5.json")),
                ("title", Json::str(title)),
                ("data", Json::obj(vec![("values", Json::Arr(values))])),
                ("mark", mark("bar")),
                (
                    "encoding",
                    Json::obj(vec![
                        (
                            "x",
                            Json::obj(vec![
                                ("field", Json::str("start")),
                                ("type", Json::str("quantitative")),
                                ("bin", Json::obj(vec![("binned", Json::Bool(true)), ("step", Json::Num(w))])),
                                ("title", Json::str(c.x)),
                            ]),
                        ),
                        ("x2", Json::obj(vec![("field", Json::str("end"))])),
                        ("y", enc("rows", "rows", "quantitative")),
                    ]),
                ),
            ])
        }
        Mark::Boxplot => {
            let x = if c.x.is_empty() { None } else { Some(enc(c.x, c.x, if dtype(c.x).is_temporal() { "temporal" } else { "nominal" })) };
            let layer = |m: Json, from: &str, to: Option<&str>| {
                let mut e = vec![("y", enc(from, c.y, "quantitative"))];
                if let Some(to) = to {
                    e.push(("y2", Json::obj(vec![("field", Json::str(to))])));
                }
                if let Some(x) = &x {
                    e.push(("x", x.clone()));
                }
                Json::obj(vec![("mark", m), ("encoding", Json::obj(e))])
            };
            Json::obj(vec![
                ("$schema", Json::str("https://vega.github.io/schema/vega-lite/v5.json")),
                ("title", Json::str(title)),
                ("description", Json::str("Lines span the smallest to the largest value; boxes the middle half; the tick is the median.")),
                ("data", Json::obj(vec![("values", Json::Arr(values))])),
                (
                    "layer",
                    Json::Arr(vec![
                        layer(mark("rule"), "min", Some("max")),
                        layer(Json::obj(vec![("type", Json::str("bar")), ("size", Json::Num(18.0)), ("tooltip", Json::Bool(true))]), "q1", Some("q3")),
                        layer(
                            Json::obj(vec![("type", Json::str("tick")), ("color", Json::str("white")), ("size", Json::Num(18.0))]),
                            "median",
                            None,
                        ),
                    ]),
                ),
            ])
        }
        m => {
            let y = y_field(&c);
            let x_ty = encoding_type(dtype(c.x));
            // Bars of a temporal or numeric x read as categories unless summarized over time.
            let x_ty = if m == Mark::Bar && x_ty == "quantitative" { "ordinal" } else { x_ty };
            let mut e = vec![("x", enc(c.x, c.x, x_ty)), ("y", enc(&y, &y, encoding_type(dtype(&y))))];
            if let Some(col) = c.color {
                e.push(("color", enc(col, col, if dtype(col).is_numeric() { "quantitative" } else { "nominal" })));
            }
            let mark = match m {
                Mark::Area => Json::obj(vec![("type", Json::str("area")), ("tooltip", Json::Bool(true)), ("opacity", Json::Num(0.85))]),
                Mark::Line if c.color.is_none() => Json::obj(vec![("type", Json::str("line")), ("tooltip", Json::Bool(true)), ("point", Json::Bool(true))]),
                m => mark(m.name()),
            };
            head(mark, Json::obj(e))
        }
    })
}

fn capitalize(s: &str) -> String {
    let mut cs = s.chars();
    cs.next().map(|f| f.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_widths_are_round() {
        assert_eq!(bin_width(0.0, 100.0, false), 5.0);
        assert_eq!(bin_width(0.0, 7.0, false), 0.5);
        assert_eq!(bin_width(12.0, 499.5, false), 50.0);
        assert_eq!(bin_width(1.0, 9.0, true), 1.0);
        assert_eq!(bin_width(3.0, 3.0, false), 1.0);
    }
}
