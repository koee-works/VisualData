//! Renaming a source: its id changes everywhere it is used.
//!
//! Steps refer to sources by id in their inputs, and custom code may name
//! them too: SQL as a table (`FROM regions`, `"regions"`, `regions.col`),
//! Python as `dfs["regions"]`. Renaming rewrites those references in the
//! steps that read the source, and only there.
//!
//! SQL is rewritten from DuckDB's own parse tree, which gives the position
//! of every table reference and qualified column, so aliases, columns,
//! strings and comments that merely spell the same word are left alone.
//! Only SQL that does not parse falls back to a token scan.

use crate::model::{Analysis, StepKind};
use crate::plan::valid_source_id;
use crate::Error;

/// What a rename changed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Renamed {
    /// Steps whose SQL or Python text was rewritten.
    pub code_changed: Vec<String>,
}

pub fn rename_source(a: &mut Analysis, old: &str, new: &str) -> Result<Renamed, Error> {
    if !a.is_source(old) {
        return Err(Error::Validation(format!("unknown source `{old}`")));
    }
    if old == new {
        return Ok(Renamed::default());
    }
    if !valid_source_id(new) {
        return Err(Error::Validation(format!(
            "`{new}` cannot be a source name: start with a letter and use only letters, digits and _"
        )));
    }
    if a.is_source(new) || a.step(new).is_some() {
        return Err(Error::Validation(format!(
            "`{new}` is already used in this analysis"
        )));
    }
    let mut out = Renamed::default();
    for src in &mut a.sources {
        if src.id == old {
            src.id = new.to_string();
        }
    }
    for step in &mut a.steps {
        let mut reads = false;
        for input in &mut step.inputs {
            if input == old {
                *input = new.to_string();
                reads = true;
            }
        }
        if !reads {
            continue;
        }
        let changed = match &mut step.kind {
            StepKind::Sql { sql } => replace(sql, rename_sql(sql, old, new)),
            StepKind::Python { code, .. } => replace(code, rename_python(code, old, new)),
            StepKind::R { code, .. } => replace(code, rename_r(code, old, new)),
            _ => false,
        };
        if changed {
            out.code_changed.push(step.id.clone());
        }
    }
    Ok(out)
}

fn replace(target: &mut String, new: String) -> bool {
    if *target == new {
        return false;
    }
    *target = new;
    true
}

/// Rewrite references to the table `old` in SQL text.
pub fn rename_sql(sql: &str, old: &str, new: &str) -> String {
    match parsed_references(sql, old) {
        Some(at) => {
            let mut out = sql.to_string();
            let mut at = at;
            at.sort_unstable();
            at.dedup();
            for pos in at.into_iter().rev() {
                if let Some(len) = identifier_len(&sql[pos..], old) {
                    out.replace_range(
                        pos..pos + len,
                        &quote_if_needed(new, sql.as_bytes()[pos] == b'"'),
                    );
                }
            }
            out
        }
        None => rename_sql_tokens(sql, old, new),
    }
}

/// Byte offsets of references to the table `old`, from DuckDB's parser.
/// `None` when the SQL does not parse (or the parser is unavailable).
fn parsed_references(sql: &str, old: &str) -> Option<Vec<usize>> {
    let conn = duckdb::Connection::open_in_memory().ok()?;
    let raw: String = conn
        .query_row("SELECT json_serialize_sql(?)", [sql], |r| r.get(0))
        .ok()?;
    let tree = crate::json::Json::parse(&raw).ok()?;
    if tree.get("error") == Some(&crate::json::Json::Bool(true)) {
        return None;
    }
    let mut ctes = Vec::new();
    let mut at = Vec::new();
    walk(&tree, old, &mut ctes, &mut at);
    // The query defines its own table of that name: its references mean that.
    if ctes.iter().any(|c: &String| c.eq_ignore_ascii_case(old)) {
        return Some(Vec::new());
    }
    Some(at)
}

fn walk(node: &crate::json::Json, old: &str, ctes: &mut Vec<String>, at: &mut Vec<usize>) {
    use crate::json::Json;
    match node {
        Json::Obj(m) => {
            let loc = m
                .get("query_location")
                .and_then(Json::as_f64)
                .map(|f| f as usize);
            let named = |v: Option<&Json>| {
                v.and_then(Json::as_str)
                    .is_some_and(|t| t.eq_ignore_ascii_case(old))
            };
            match m.get("type").and_then(Json::as_str) {
                Some("BASE_TABLE")
                    if named(m.get("table_name"))
                        && m.get("schema_name")
                            .and_then(Json::as_str)
                            .unwrap_or("")
                            .is_empty() =>
                {
                    at.extend(loc)
                }
                Some("COLUMN_REF") => {
                    if let Some(Json::Arr(parts)) = m.get("column_names") {
                        if parts.len() >= 2 && named(parts.first()) {
                            at.extend(loc);
                        }
                    }
                }
                _ => {}
            }
            if let Some(Json::Arr(entries)) = m.get("cte_map").and_then(|c| c.get("map")) {
                for e in entries {
                    if let Some(k) = e.get("key").and_then(Json::as_str) {
                        ctes.push(k.to_string());
                    }
                }
            }
            for v in m.values() {
                walk(v, old, ctes, at);
            }
        }
        Json::Arr(items) => items.iter().for_each(|v| walk(v, old, ctes, at)),
        _ => {}
    }
}

/// Length of the identifier `old` written at the start of `text`, bare (any
/// case) or double-quoted (exact), if that is what is there.
fn identifier_len(text: &str, old: &str) -> Option<usize> {
    if let Some(rest) = text.strip_prefix('"') {
        let quoted = old.replace('"', "\"\"");
        return rest
            .starts_with(&format!("{quoted}\""))
            .then_some(quoted.len() + 2);
    }
    let word: usize = text
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
        .count();
    text[..word].eq_ignore_ascii_case(old).then_some(word)
}

fn quote_if_needed(new: &str, was_quoted: bool) -> String {
    if was_quoted {
        format!("\"{}\"", new.replace('"', "\"\""))
    } else {
        new.to_string()
    }
}

/// Fallback for SQL that does not parse: bare identifiers (any case, as SQL
/// folds them) and double-quoted ones (exact); strings and comments kept.
fn rename_sql_tokens(sql: &str, old: &str, new: &str) -> String {
    let b = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let rest = &sql[i..];
        if c == b'\'' {
            let end = scan_quoted(b, i, b'\'');
            out.push_str(&sql[i..end]);
            i = end;
        } else if c == b'"' {
            let end = scan_quoted(b, i, b'"');
            let inner = sql[i + 1..end.saturating_sub(1).max(i + 1)].replace("\"\"", "\"");
            if end - i >= 2 && b[end - 1] == b'"' && inner == old {
                out.push('"');
                out.push_str(&new.replace('"', "\"\""));
                out.push('"');
            } else {
                out.push_str(&sql[i..end]);
            }
            i = end;
        } else if rest.starts_with("--") {
            let end = rest.find('\n').map(|n| i + n).unwrap_or(b.len());
            out.push_str(&sql[i..end]);
            i = end;
        } else if let Some(body) = rest.strip_prefix("/*") {
            let end = body.find("*/").map(|n| i + 2 + n + 2).unwrap_or(b.len());
            out.push_str(&sql[i..end]);
            i = end;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let mut end = i;
            while end < b.len()
                && (b[end].is_ascii_alphanumeric() || b[end] == b'_' || b[end] == b'$')
            {
                end += 1;
            }
            let word = &sql[i..end];
            out.push_str(if word.eq_ignore_ascii_case(old) {
                new
            } else {
                word
            });
            i = end;
        } else {
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// End (exclusive) of a quoted run starting at `start`, with doubled quotes
/// as escapes. An unterminated run ends at the end of the text.
fn scan_quoted(b: &[u8], start: usize, q: u8) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        if b[i] == q {
            if i + 1 < b.len() && b[i + 1] == q {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    b.len()
}

/// Rewrite `dfs["old"]`, `dfs['old']` and `dfs.get("old"…)` in Python code.
pub fn rename_python(code: &str, old: &str, new: &str) -> String {
    let mut out = code.to_string();
    for q in ['"', '\''] {
        for (from, to) in [
            (format!("dfs[{q}{old}{q}]"), format!("dfs[{q}{new}{q}]")),
            (
                format!("dfs.get({q}{old}{q}"),
                format!("dfs.get({q}{new}{q}"),
            ),
        ] {
            out = out.replace(&from, &to);
        }
    }
    out
}

/// Rewrite `dfs$old`, `` dfs$`old` ``, `dfs[["old"]]` and `dfs[['old']]` in R code.
pub fn rename_r(code: &str, old: &str, new: &str) -> String {
    let mut out = code.to_string();
    for q in ['"', '\''] {
        out = out.replace(&format!("dfs[[{q}{old}{q}]]"), &format!("dfs[[{q}{new}{q}]]"));
    }
    out = out.replace(&format!("dfs$`{old}`"), &format!("dfs$`{new}`"));
    // dfs$old only as a whole name: not dfs$older.
    let from = format!("dfs${old}");
    let mut result = String::new();
    let mut rest = out.as_str();
    while let Some(i) = rest.find(&from) {
        let after = rest[i + from.len()..].chars().next();
        result.push_str(&rest[..i]);
        if after.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
            result.push_str(&from);
        } else {
            result.push_str(&format!("dfs${new}"));
        }
        rest = &rest[i + from.len()..];
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r_lookups_are_renamed() {
        assert_eq!(
            rename_r("a <- dfs$regions; b <- dfs[[\"regions\"]]; c <- dfs$regions_old; d <- dfs$`regions`", "regions", "targets"),
            "a <- dfs$targets; b <- dfs[[\"targets\"]]; c <- dfs$regions_old; d <- dfs$`targets`"
        );
    }

    #[test]
    fn only_real_table_references_are_renamed() {
        let sql = "SELECT regions.region AS regions, (SELECT count(*) FROM regions) AS n, 'regions' AS s \
                   FROM input JOIN \"regions\" r ON TRUE -- regions\nWHERE Regions.x > 0";
        assert_eq!(
            rename_sql(sql, "regions", "targets"),
            "SELECT targets.region AS regions, (SELECT count(*) FROM targets) AS n, 'regions' AS s \
             FROM input JOIN \"targets\" r ON TRUE -- regions\nWHERE targets.x > 0"
        );
        // A column of that name, and a CTE of that name, are not the source.
        assert_eq!(
            rename_sql("SELECT regions FROM input", "regions", "t"),
            "SELECT regions FROM input"
        );
        let cte = "WITH regions AS (SELECT 1 AS x) SELECT * FROM regions";
        assert_eq!(rename_sql(cte, "regions", "t"), cte);
    }

    #[test]
    fn sql_that_does_not_parse_falls_back_to_tokens() {
        let sql = "SELECT r.region, 'regions' AS label -- regions here stay\n\
                   FROM input JOIN regions AS r USING (region) JOIN \"regions\" q ON TRUE \
                   /* regions */ WHERE REGIONS.x > 0 AND regions_old = 1";
        assert_eq!(
            rename_sql_tokens(sql, "regions", "targets"),
            "SELECT r.region, 'regions' AS label -- regions here stay\n\
             FROM input JOIN targets AS r USING (region) JOIN \"targets\" q ON TRUE \
             /* regions */ WHERE targets.x > 0 AND regions_old = 1"
        );
        // Escaped quotes inside strings do not end them.
        assert_eq!(
            rename_sql_tokens("SELECT 'it''s regions' FROM regions", "regions", "t"),
            "SELECT 'it''s regions' FROM t"
        );
        assert_eq!(
            rename_sql("SELECT 'unterminated regions", "regions", "t"),
            "SELECT 'unterminated regions"
        );
    }

    #[test]
    fn python_lookups_are_renamed() {
        let code = "a = dfs['regions']\nb = dfs.get(\"regions\", None)\nc = 'regions'";
        assert_eq!(
            rename_python(code, "regions", "targets"),
            "a = dfs['targets']\nb = dfs.get(\"targets\", None)\nc = 'regions'"
        );
    }
}
