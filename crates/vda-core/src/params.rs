//! Analysis parameters: named values that steps refer to, so one change
//! (a date range, a threshold, a region) updates every step that uses it.
//!
//! - Formulas write `@name`, SQL writes `$name`, a filter can compare with a
//!   parameter instead of a fixed value, and Python steps get `params`.
//! - Steps are *bound* before planning: references are replaced by the
//!   values as literals. Engines, the inspectable SQL and cache keys see
//!   only bound steps, so a new value outdates exactly the steps that use
//!   the parameter, and both engines agree by construction.
//! - Unknown references are left as written; the step then fails with a
//!   message naming the parameter (and the linter reports it first).

use crate::compile::quote_literal;
use crate::model::{Step, StepKind};
use crate::table::{DataType, Value};
use crate::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub value: Value,
}

/// The kinds of value a parameter can hold.
pub fn allowed(t: DataType) -> bool {
    matches!(t, DataType::Int | DataType::Float | DataType::Text | DataType::Bool | DataType::Date)
}

pub fn valid_name(name: &str) -> bool {
    let mut cs = name.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_') && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Check a set of parameters: valid, unique names and supported values.
pub fn validate(params: &[Parameter]) -> Result<(), Error> {
    for (i, p) in params.iter().enumerate() {
        if !valid_name(&p.name) {
            return Err(Error::Validation(format!(
                "parameter name `{}`: use letters, digits and _, starting with a letter",
                p.name
            )));
        }
        if params[..i].iter().any(|q| q.name == p.name) {
            return Err(Error::Validation(format!("there are two parameters called `{}`", p.name)));
        }
        match p.value.dtype() {
            Some(t) if allowed(t) => {}
            _ => {
                return Err(Error::Validation(format!(
                    "parameter `{}` needs a value: a number, text, true/false or a date",
                    p.name
                )))
            }
        }
        if let Value::Float(f) = p.value {
            if !f.is_finite() {
                return Err(Error::Validation(format!("parameter `{}` must be a finite number", p.name)));
            }
        }
    }
    Ok(())
}

fn find<'a>(params: &'a [Parameter], name: &str) -> Option<&'a Parameter> {
    params.iter().find(|p| p.name == name)
}

// ---------------------------------------------------------------- formulas

/// `@name` references in a formula, with their character positions (from 0).
fn formula_refs(src: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            q @ ('"' | '\'') => {
                i += 1;
                while i < chars.len() {
                    if chars[i] == q && chars.get(i + 1) == Some(&q) {
                        i += 2;
                    } else if chars[i] == q {
                        break;
                    } else {
                        i += 1;
                    }
                }
                i += 1;
            }
            '[' => {
                while i < chars.len() && chars[i] != ']' {
                    i += 1;
                }
                i += 1;
            }
            '@' => {
                let start = i;
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                if i > start + 1 {
                    out.push((start, i, chars[start + 1..i].iter().collect()));
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// The parameters a formula names.
pub fn formula_params(src: &str) -> Vec<String> {
    formula_refs(src).into_iter().map(|r| r.2).collect()
}

/// A parameter's value as formula source.
fn formula_literal(v: &Value) -> String {
    match v {
        Value::Int(i) if *i < 0 => format!("({i})"),
        Value::Float(f) if *f < 0.0 => format!("({f:?})"),
        Value::Float(f) => format!("{f:?}"),
        Value::Text(s) => format!("\"{}\"", s.replace('"', "\"\"")),
        Value::Date(_) => format!("date(\"{v}\")"),
        v => v.to_string(),
    }
}

pub fn bind_formula(src: &str, params: &[Parameter]) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    for (start, end, name) in formula_refs(src) {
        let Some(p) = find(params, &name) else { continue };
        out.extend(&chars[at..start]);
        out.push_str(&formula_literal(&p.value));
        at = end;
    }
    out.extend(&chars[at..]);
    out
}

// --------------------------------------------------------------------- SQL

/// `$name` references in SQL, skipping strings, quoted names, comments and
/// dollar-quoted text. `$1` (positional) is not a parameter.
fn sql_refs(sql: &str) -> Vec<(usize, usize, String)> {
    let c: Vec<char> = sql.chars().collect();
    let ident = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            q @ ('\'' | '"') => {
                i += 1;
                while i < c.len() {
                    if c[i] == q && c.get(i + 1) == Some(&q) {
                        i += 2;
                    } else if c[i] == q {
                        break;
                    } else {
                        i += 1;
                    }
                }
                i += 1;
            }
            '-' if c.get(i + 1) == Some(&'-') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if c.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            '$' => {
                let start = i;
                let mut j = i + 1;
                while j < c.len() && ident(c[j]) {
                    j += 1;
                }
                let name: String = c[start + 1..j].iter().collect();
                if c.get(j) == Some(&'$') {
                    // $tag$ … $tag$ is quoted text.
                    let tag: Vec<char> = c[start..=j].to_vec();
                    i = j + 1;
                    while i < c.len() && c[i..].iter().take(tag.len()).ne(tag.iter()) {
                        i += 1;
                    }
                    i += tag.len();
                } else {
                    let after_ident = start > 0 && ident(c[start - 1]);
                    if !name.is_empty() && !name.starts_with(|ch: char| ch.is_ascii_digit()) && !after_ident {
                        out.push((start, j, name));
                    }
                    i = j.max(i + 1);
                }
            }
            _ => i += 1,
        }
    }
    out
}

pub fn sql_params(sql: &str) -> Vec<String> {
    sql_refs(sql).into_iter().map(|r| r.2).collect()
}

pub fn bind_sql(sql: &str, params: &[Parameter]) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    for (start, end, name) in sql_refs(sql) {
        let Some(p) = find(params, &name) else { continue };
        out.extend(&chars[at..start]);
        // Parenthesized so `x - $n` with a negative value cannot become a comment.
        out.push_str(&format!("({} /* ${name} */)", quote_literal(&p.value)));
        at = end;
    }
    out.extend(&chars[at..]);
    out
}

// ------------------------------------------------------------------- steps

/// The parameters a step refers to (Python steps can read any of them).
pub fn used_by(step: &Step) -> Vec<String> {
    let mut names = match &step.kind {
        StepKind::Filter { predicates } => predicates.iter().filter_map(|p| p.param.clone()).collect(),
        StepKind::Derive { columns } => columns.iter().flat_map(|c| formula_params(&c.formula)).collect(),
        StepKind::Sql { sql } => sql_params(sql),
        StepKind::Check { rules, .. } => rules.iter().flat_map(|r| r.formula().map(formula_params).unwrap_or_default()).collect(),
        _ => Vec::new(),
    };
    names.dedup();
    names
}

/// The step with parameter references replaced by their values.
pub fn bind_step(step: &Step, params: &[Parameter]) -> Step {
    if params.is_empty() {
        return step.clone();
    }
    let mut s = step.clone();
    match &mut s.kind {
        StepKind::Filter { predicates } => {
            for p in predicates {
                if let Some(v) = p.param.as_deref().and_then(|n| find(params, n)) {
                    // Bound to its value alone: the same comparison with a
                    // fixed value, or a renamed parameter, has the same key.
                    p.value = v.value.clone();
                    p.param = None;
                }
            }
        }
        StepKind::Derive { columns } => {
            for c in columns {
                c.formula = bind_formula(&c.formula, params);
            }
        }
        StepKind::Sql { sql } => *sql = bind_sql(sql, params),
        StepKind::Clean { columns, .. } => {
            for c in columns {
                if let Some(f) = &mut c.missing_when {
                    *f = bind_formula(f, params);
                }
            }
        }
        StepKind::Check { rules, .. } => {
            for r in rules {
                if let Some(f) = r.formula_mut() {
                    *f = bind_formula(f, params);
                }
            }
        }
        _ => {}
    }
    s
}

/// Rename a parameter in every formula, SQL query and filter that uses it.
/// Python code is left as written (it reads `params["…"]`, and editing it
/// would ask for a new review); the result says which Python steps to edit.
pub fn rename(a: &mut crate::model::Analysis, old: &str, new: &str) -> Result<Vec<String>, Error> {
    if !valid_name(new) {
        return Err(Error::Validation(format!("parameter name `{new}`: use letters, digits and _, starting with a letter")));
    }
    if a.parameters.iter().any(|p| p.name == new) {
        return Err(Error::Validation(format!("there is already a parameter called `{new}`")));
    }
    let p = a.parameters.iter_mut().find(|p| p.name == old).ok_or_else(|| Error::Validation(format!("unknown parameter `{old}`")))?;
    p.name = new.to_string();
    let swap = |src: &str, refs: Vec<(usize, usize, String)>, sigil: char| -> String {
        let chars: Vec<char> = src.chars().collect();
        let mut out = String::new();
        let mut at = 0;
        for (start, end, name) in refs {
            if name == old {
                out.extend(&chars[at..start]);
                out.push(sigil);
                out.push_str(new);
                at = end;
            }
        }
        out.extend(&chars[at..]);
        out
    };
    let mut python = Vec::new();
    for s in &mut a.steps {
        match &mut s.kind {
            StepKind::Filter { predicates } => {
                for p in predicates {
                    if p.param.as_deref() == Some(old) {
                        p.param = Some(new.to_string());
                    }
                }
            }
            StepKind::Derive { columns } => {
                for c in columns {
                    c.formula = swap(&c.formula, formula_refs(&c.formula), '@');
                }
            }
            StepKind::Check { rules, .. } => {
                for r in rules {
                    if let Some(f) = r.formula_mut() {
                        *f = swap(f, formula_refs(f), '@');
                    }
                }
            }
            StepKind::Sql { sql } => *sql = swap(sql, sql_refs(sql), '$'),
            StepKind::Python { code, .. } | StepKind::R { code, .. } if code.contains(old) => python.push(s.id.clone()),
            _ => {}
        }
    }
    Ok(python)
}

/// Parameters as JSON for Python: `[[name, type, value], …]`.
pub fn python_json(params: &[Parameter]) -> String {
    let items: Vec<crate::json::Json> = params
        .iter()
        .map(|p| {
            use crate::json::Json;
            let (t, v) = match &p.value {
                Value::Int(i) => ("int", Json::Str(i.to_string())),
                Value::Float(f) => ("float", Json::Num(*f)),
                Value::Bool(b) => ("bool", Json::Bool(*b)),
                Value::Date(_) => ("date", Json::Str(p.value.to_string())),
                v => ("text", Json::Str(v.to_string())),
            };
            Json::Arr(vec![Json::str(p.name.clone()), Json::str(t), v])
        })
        .collect();
    crate::json::Json::Arr(items).to_string_compact()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ps() -> Vec<Parameter> {
        vec![
            Parameter { name: "n".into(), value: Value::Int(-5) },
            Parameter { name: "region".into(), value: Value::Text("O\"Hare's".into()) },
            Parameter { name: "x".into(), value: Value::Float(2.5) },
        ]
    }

    #[test]
    fn formulas_bind_outside_text_and_names() {
        let f = r#"units * @x + @n & "@n" & [@n] & @unknown"#;
        assert_eq!(bind_formula(f, &ps()), r#"units * 2.5 + (-5) & "@n" & [@n] & @unknown"#);
        assert_eq!(bind_formula("region = @region", &ps()), r#"region = "O""Hare's""#);
        assert_eq!(formula_params(f), ["x", "n", "unknown"]);
    }

    #[test]
    fn sql_binds_outside_strings_comments_and_dollar_quotes() {
        let sql = "SELECT x - $n, '$n', \"$n\", $1, $$ $n $$, $t$ $n $t$ -- $n\n/* $n */ FROM t WHERE r = $region AND a$n = 1";
        assert_eq!(
            bind_sql(sql, &ps()),
            "SELECT x - (-5 /* $n */), '$n', \"$n\", $1, $$ $n $$, $t$ $n $t$ -- $n\n/* $n */ FROM t WHERE r = ('O\"Hare''s' /* $region */) AND a$n = 1"
        );
        assert_eq!(sql_params(sql), ["n", "region"]);
    }

    #[test]
    fn names_and_values_are_checked() {
        assert!(validate(&ps()).is_ok());
        let bad = |name: &str, value: Value| validate(&[Parameter { name: name.into(), value }]).unwrap_err().to_string();
        assert!(bad("2x", Value::Int(1)).contains("starting with a letter"));
        assert!(bad("t", Value::Null).contains("needs a value"));
        let twice = vec![ps()[0].clone(), ps()[0].clone()];
        assert!(validate(&twice).unwrap_err().to_string().contains("two parameters called `n`"));
    }
}
