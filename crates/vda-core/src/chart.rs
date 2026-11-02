//! Chart steps produce a Vega-Lite v5 specification with the data inlined.

use crate::json::Json;
use crate::model::Mark;
use crate::persist::value_to_json;
use crate::table::{DataType, Table, Value};
use crate::Error;

/// Rows above this are not inlined; the chart is labelled as partial.
pub const MAX_CHART_ROWS: usize = 5_000;

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

pub fn vega_lite(table: &Table, mark: Mark, x: &str, y: &str, title: Option<&str>) -> Result<Json, Error> {
    let field = |n: &str| {
        table.schema.field(n).ok_or_else(|| Error::Validation(format!("chart column `{n}` not found")))
    };
    let (xf, yf) = (field(x)?, field(y)?);
    let (xi, yi) = (table.schema.index_of(x).unwrap(), table.schema.index_of(y).unwrap());
    let total = table.rows.len();
    let values: Vec<Json> = table
        .rows
        .iter()
        .take(MAX_CHART_ROWS)
        .map(|r| Json::Obj([(x.to_string(), plain(&r[xi])), (y.to_string(), plain(&r[yi]))].into_iter().collect()))
        .collect();
    let mut title = title.map(str::to_string).unwrap_or_else(|| format!("{y} by {x}"));
    if total > MAX_CHART_ROWS {
        title.push_str(&format!(" (first {MAX_CHART_ROWS} of {total} rows)"));
    }
    Ok(Json::obj(vec![
        ("$schema", Json::str("https://vega.github.io/schema/vega-lite/v5.json")),
        ("title", Json::str(title)),
        ("mark", Json::obj(vec![("type", Json::str(mark.name())), ("tooltip", Json::Bool(true))])),
        ("data", Json::obj(vec![("values", Json::Arr(values))])),
        (
            "encoding",
            Json::obj(vec![
                (
                    "x",
                    Json::obj(vec![
                        ("field", Json::str(x)),
                        ("type", Json::str(encoding_type(xf.dtype))),
                        ("title", Json::str(x)),
                    ]),
                ),
                (
                    "y",
                    Json::obj(vec![
                        ("field", Json::str(y)),
                        ("type", Json::str(encoding_type(yf.dtype))),
                        ("title", Json::str(y)),
                    ]),
                ),
            ]),
        ),
    ]))
}
