//! A step described in a few words ("Keep rows where units at least 3"),
//! for history and anywhere else a step without a name is mentioned.
//!
//! The same words as the app's `describe()` (apps/desktop/src/lib/format.ts);
//! both are checked against `describe-examples.json`.

use crate::json::Json;
use crate::model::{Step, StepKind};

fn s<'a>(j: &'a Json, k: &str) -> &'a str {
    j.get(k).and_then(Json::as_str).unwrap_or("")
}

fn strs(j: &Json, k: &str) -> Vec<String> {
    j.get(k).and_then(Json::as_arr).map(|a| a.iter().filter_map(Json::as_str).map(str::to_string).collect()).unwrap_or_default()
}

fn literal(v: Option<&Json>) -> String {
    match v {
        None | Some(Json::Null) => String::new(),
        Some(Json::Bool(b)) => b.to_string(),
        Some(Json::Str(t)) => t.clone(),
        Some(Json::Num(n)) => num(*n),
        Some(o) => ["int", "float", "date", "timestamp"]
            .iter()
            .find_map(|k| o.get(k))
            .map(|v| match v {
                Json::Num(n) => num(*n),
                Json::Str(t) => t.clone(),
                other => other.to_string_compact(),
            })
            .unwrap_or_default(),
    }
}

fn num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

fn op_words(op: &str) -> &'static str {
    match op {
        "=" => "is",
        "<>" => "is not",
        "<" => "below",
        "<=" => "at most",
        ">" => "above",
        ">=" => "at least",
        "IS NULL" => "is missing",
        "IS NOT NULL" => "is present",
        _ => "?",
    }
}

fn symbol(op: &str) -> &str {
    match op {
        "<>" => "≠",
        "<=" => "≤",
        ">=" => "≥",
        o => o,
    }
}

pub fn func_words(f: &str) -> String {
    match f {
        "COUNT" => "Count".into(),
        "SUM" => "Sum".into(),
        "AVG" => "Average".into(),
        "MIN" => "Minimum".into(),
        "MAX" => "Maximum".into(),
        "MEDIAN" => "Median".into(),
        "COUNT_DISTINCT" => "Distinct count".into(),
        "STDDEV" => "Standard deviation".into(),
        p => {
            let n: u32 = p.get(1..).and_then(|d| d.parse().ok()).unwrap_or(0);
            let suffix = match (n % 10, n % 100) {
                (1, x) if x != 11 => "st",
                (2, x) if x != 12 => "nd",
                (3, x) if x != 13 => "rd",
                _ => "th",
            };
            format!("{n}{suffix} percentile")
        }
    }
}

fn window_words(f: &str) -> &'static str {
    match f {
        "running_sum" => "Running total",
        "row_number" => "Row number",
        "rank" => "Rank",
        "previous" => "Previous value",
        "next" => "Next value",
        "moving_average" => "Moving average",
        "change" => "Change from previous",
        "percent_change" => "% change from previous",
        "share_of_total" => "Share of total",
        _ => "Window calculation",
    }
}

fn or<'a>(v: &'a str, d: &'a str) -> &'a str {
    if v.is_empty() { d } else { v }
}

/// What a step does, from its definition as saved (`analysis.json`).
pub fn describe_json(op: &Json) -> String {
    let arr = |k: &str| op.get(k).and_then(Json::as_arr).cloned().unwrap_or_default();
    match s(op, "type") {
        "filter" => {
            let ps = arr("predicates");
            if ps.is_empty() {
                return "Keep all rows".into();
            }
            let parts: Vec<String> = ps
                .iter()
                .map(|p| {
                    let o = s(p, "op");
                    let value = match p.get("param").and_then(Json::as_str) {
                        Some(name) => format!("@{name}"),
                        None => literal(p.get("value")),
                    };
                    if o.starts_with("IS") { format!("{} {}", s(p, "column"), op_words(o)) } else { format!("{} {} {value}", s(p, "column"), op_words(o)) }
                })
                .collect();
            format!("Keep rows where {}", parts.join(" and "))
        }
        "aggregate" => {
            let ms: Vec<String> = arr("measures")
                .iter()
                .map(|m| {
                    let col = m.get("column").and_then(Json::as_str);
                    if s(m, "func") == "COUNT" && col.is_none() { "count".into() } else { format!("{} of {}", func_words(s(m, "func")).to_lowercase(), col.unwrap_or("")) }
                })
                .collect();
            let by = strs(op, "group_by");
            let head = if ms.is_empty() { "Groups".to_string() } else { ms.join(", ") };
            if by.is_empty() { head } else { format!("{head} by {}", by.join(", ")) }
        }
        "join" => {
            let kind = match s(op, "how") {
                "left" => "Left join",
                "full" => "Full join",
                _ => "Join",
            };
            let keys: Vec<String> = arr("on")
                .iter()
                .map(|k| {
                    let o = k.get("op").and_then(Json::as_str).unwrap_or("=");
                    if o == "=" && s(k, "left") == s(k, "right") { s(k, "left").to_string() } else { format!("{} {} {}", s(k, "left"), symbol(o), s(k, "right")) }
                })
                .collect();
            if keys.is_empty() { kind.into() } else { format!("{kind} on {}", keys.join(", ")) }
        }
        "derive" => {
            let cols = arr("columns");
            if cols.len() == 1 {
                format!("Calculate {}", or(s(&cols[0], "name"), "a column"))
            } else {
                format!("Calculate {}", cols.iter().map(|c| s(c, "name").to_string()).collect::<Vec<_>>().join(", "))
            }
        }
        "select" => format!(
            "Keep {}",
            arr("columns").iter().map(|c| c.get("as").and_then(Json::as_str).unwrap_or(s(c, "column")).to_string()).collect::<Vec<_>>().join(", ")
        ),
        "sort" => {
            let keys: Vec<String> = arr("by").iter().map(|k| format!("{}{}", s(k, "column"), if k.get("descending") == Some(&Json::Bool(true)) { " (highest first)" } else { "" })).collect();
            let first = op.get("limit").and_then(Json::as_f64).map(|n| format!("first {}", n as i64)).unwrap_or_default();
            if keys.is_empty() {
                format!("Keep the {first} rows")
            } else if first.is_empty() {
                format!("Sort by {}", keys.join(", "))
            } else {
                format!("Sort by {}, {first}", keys.join(", "))
            }
        }
        "window" => {
            let what: Vec<String> = arr("columns")
                .iter()
                .map(|c| match c.get("column").and_then(Json::as_str) {
                    Some(col) => format!("{} of {col}", window_words(s(c, "func")).to_lowercase()),
                    None => window_words(s(c, "func")).to_lowercase(),
                })
                .collect();
            let text = if what.is_empty() { "Window calculations".to_string() } else { what.join(", ") };
            let mut chars = text.chars();
            let text = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
            let per = strs(op, "partition_by");
            if per.is_empty() { text } else { format!("{text} per {}", per.join(", ")) }
        }
        "dedupe" => {
            let keys = strs(op, "keys");
            if keys.is_empty() { "Remove repeated rows".into() } else { format!("One row per {}", keys.join(", ")) }
        }
        "pivot" => {
            let rows = strs(op, "rows");
            let head = format!("Columns for each {}", or(s(op, "column"), "value"));
            if rows.is_empty() { head } else { format!("{head}, rows by {}", rows.join(", ")) }
        }
        "unpivot" => {
            let cols = strs(op, "columns");
            format!("Turn {} into rows", if cols.is_empty() { "columns".into() } else { cols.join(", ") })
        }
        "append" => if op.get("distinct") == Some(&Json::Bool(true)) { "Append rows, without repeats" } else { "Append rows" }.into(),
        "sql" => "Custom SQL".into(),
        "python" => "Custom Python".into(),
        "r" => "Custom R".into(),
        "chart" => {
            let (x, y) = (s(op, "x"), s(op, "y"));
            match s(op, "mark") {
                "histogram" => format!("Histogram of {x}"),
                "boxplot" => if x.is_empty() { format!("Box plot of {y}") } else { format!("Box plot of {y} by {x}") },
                mark => {
                    let name = match mark {
                        "line" => "Line",
                        "point" => "Point",
                        "area" => "Area",
                        _ => "Bar",
                    };
                    let yw = match op.get("summary").and_then(Json::as_str) {
                        None => y.to_string(),
                        Some("COUNT") => "rows".into(),
                        Some(f) => format!("{} of {y}", func_words(f).to_lowercase()),
                    };
                    let color = op.get("color").and_then(Json::as_str).map(|c| format!(" and {c}")).unwrap_or_default();
                    format!("{name} chart of {yw} by {x}{color}")
                }
            }
        }
        "check" => {
            let n = arr("rules").len();
            if n == 1 { "Check 1 rule".into() } else { format!("Check {n} rules") }
        }
        "test" => {
            let (a, b) = (s(op, "a"), s(op, "b"));
            match s(op, "test") {
                "compare_means" => format!("Compare the average of {} between {}", or(a, "…"), or(b, "two groups")),
                "compare_groups" => format!("Compare the average of {} across {}", or(a, "…"), or(b, "groups")),
                "independence" => format!("Test whether {} and {} are related", or(a, "…"), or(b, "…")),
                _ => format!("Correlation of {} and {}", or(a, "…"), or(b, "…")),
            }
        }
        _ => "Step".into(),
    }
}

pub fn describe(kind: &StepKind) -> String {
    describe_json(&crate::persist::step_kind_to_json(kind))
}

/// The step's name, or what it does when it has none.
pub fn title(step: &Step) -> String {
    step.label.clone().filter(|l| !l.trim().is_empty()).unwrap_or_else(|| describe(&step.kind))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_shared_examples() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/desktop/src/lib/describe-examples.json");
        let ex = crate::json::Json::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        let ex = ex.as_arr().unwrap();
        assert!(ex.len() > 15);
        for e in ex {
            let want = e.get("words").and_then(crate::json::Json::as_str).unwrap();
            assert_eq!(super::describe_json(e.get("op").unwrap()), want);
        }
    }
}
