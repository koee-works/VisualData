//! Database sources: a table, or one read-only query, from a SQLite or
//! DuckDB database file.
//!
//! The rows are copied into the project's saved data when the source is
//! read, like any other source, so a run is reproducible: the source's
//! fingerprint is the database file's content hash plus the table or query,
//! and a changed database outdates exactly the steps that read it.
//!
//! Queries are held to the same rules as SQL steps (ADR-0004):
//! - **DuckDB files** are opened read-only on a separate connection that can
//!   read no other file and write only the result; the query must be a
//!   single SELECT.
//! - **SQLite files** are opened read-only, with attaching other databases
//!   and loading extensions turned off; the statement must be one read-only
//!   statement.

use crate::compile::{quote_ident, quote_literal};
use crate::model::SourceFormat;
use crate::table::{DataType, Field, Schema, Table, Value};
use crate::Error;
use std::path::Path;

/// Most rows read from a SQLite query or table (its rows are held in memory
/// while converting; DuckDB files stream).
pub const MAX_SQLITE_ROWS: usize = 20_000_000;

/// Which kind of database a file is, from its first bytes.
pub fn sniff(path: &Path) -> Option<SourceFormat> {
    use std::io::Read;
    let mut head = [0u8; 16];
    let mut f = std::fs::File::open(path).ok()?;
    let n = f.read(&mut head).ok()?;
    if n >= 16 && &head[..16] == b"SQLite format 3\0" {
        return Some(SourceFormat::Sqlite);
    }
    if n >= 12 && &head[8..12] == b"DUCK" {
        return Some(SourceFormat::DuckDb);
    }
    None
}

fn db_err(path: &Path) -> impl Fn(String) -> Error + '_ {
    move |m| Error::Data(format!("cannot read {}: {m}", path.display()))
}

/// What a database source reads: a table, or a query.
#[derive(Debug, Clone, Copy)]
pub enum Read<'a> {
    Table(Option<&'a str>),
    Query(&'a str),
}

// ------------------------------------------------------------------ DuckDB

fn duck_readonly(path: &Path, write_to: Option<&Path>) -> Result<duckdb::Connection, Error> {
    let config = duckdb::Config::default()
        .access_mode(duckdb::AccessMode::ReadOnly)
        .map_err(|e| db_err(path)(e.to_string()))?;
    let conn = duckdb::Connection::open_with_flags(path, config).map_err(|e| db_err(path)(e.to_string()))?;
    let mut sql = String::from("SET threads = 1; SET autoinstall_known_extensions = false; SET autoload_known_extensions = false;");
    if let Some(out) = write_to {
        sql.push_str(&format!(" SET allowed_paths = [{}];", quote_literal(&Value::Text(out.to_string_lossy().into_owned()))));
    }
    sql.push_str(" SET enable_external_access = false; SET lock_configuration = true;");
    conn.execute_batch(&sql).map_err(|e| db_err(path)(e.to_string()))?;
    Ok(conn)
}

fn duck_tables(conn: &duckdb::Connection, path: &Path) -> Result<Vec<String>, Error> {
    let mut st = conn
        .prepare("SELECT table_name FROM information_schema.tables WHERE table_catalog = current_database() AND table_schema = 'main' ORDER BY table_name")
        .map_err(|e| db_err(path)(e.to_string()))?;
    let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| db_err(path)(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| db_err(path)(e.to_string()))
}

// ------------------------------------------------------------------ SQLite

fn sqlite_readonly(path: &Path) -> Result<rusqlite::Connection, Error> {
    use rusqlite::OpenFlags;
    let conn = rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| db_err(path)(e.to_string()))?;
    // No other databases, and no extensions.
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED, 0).map_err(|e| db_err(path)(e.to_string()))?;
    Ok(conn)
}

fn sqlite_tables(conn: &rusqlite::Connection, path: &Path) -> Result<Vec<String>, Error> {
    let mut st = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .map_err(|e| db_err(path)(e.to_string()))?;
    let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| db_err(path)(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| db_err(path)(e.to_string()))
}

/// SQLite stores any value in any column; a column's type follows from its
/// declared type and the values it holds.
fn sqlite_type(declared: &str, values: &[&Value]) -> DataType {
    let d = declared.to_ascii_uppercase();
    let present: Vec<&&Value> = values.iter().filter(|v| !v.is_null()).collect();
    let all = |f: &dyn Fn(&Value) -> bool| present.iter().all(|v| f(v));
    let iso = |v: &Value, ts: bool| match v {
        Value::Text(s) => if ts { crate::table::parse_timestamp(s).is_some() } else { crate::table::parse_date(s).is_some() },
        _ => false,
    };
    if d.contains("BOOL") && all(&|v| matches!(v, Value::Int(0 | 1))) {
        DataType::Bool
    } else if d.contains("DATE") && !d.contains("TIME") && !present.is_empty() && all(&|v| iso(v, false)) {
        DataType::Date
    } else if (d.contains("TIME") || d.contains("DATE")) && !present.is_empty() && all(&|v| iso(v, true)) {
        DataType::Timestamp
    } else if all(&|v| matches!(v, Value::Int(_))) && !(present.is_empty() && (d.contains("CHAR") || d.contains("TEXT"))) {
        if present.is_empty() && (d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB")) { DataType::Float } else { DataType::Int }
    } else if all(&|v| matches!(v, Value::Int(_) | Value::Float(_))) {
        DataType::Float
    } else {
        DataType::Text
    }
}

fn sqlite_read(path: &Path, what: Read) -> Result<Table, Error> {
    let conn = sqlite_readonly(path)?;
    let sql = match what {
        Read::Table(t) => {
            let tables = sqlite_tables(&conn, path)?;
            let name = match t {
                Some(t) if tables.iter().any(|x| x == t) => t.to_string(),
                Some(t) => return Err(Error::Validation(format!("{} has no table `{t}` (tables: {})", path.display(), tables.join(", ")))),
                None => tables.first().cloned().ok_or_else(|| Error::Validation(format!("{} has no tables", path.display())))?,
            };
            format!("SELECT * FROM {}", quote_ident(&name))
        }
        Read::Query(q) => q.trim().trim_end_matches(';').to_string(),
    };
    let mut st = conn.prepare(&sql).map_err(|e| Error::Validation(format!("the query does not run: {e}")))?;
    if !st.readonly() {
        return Err(Error::Validation("the query must only read (SELECT); it cannot change the database".into()));
    }
    let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
    let declared: Vec<String> = st.columns().iter().map(|c| c.decl_type().unwrap_or("").to_string()).collect();
    let n = names.len();
    let mut raw: Vec<Vec<Value>> = Vec::new();
    let explain = |e: rusqlite::Error| {
        let m = e.to_string();
        if m.contains("too many attached databases") {
            Error::Validation("the query can only read this database; it cannot attach others".into())
        } else {
            db_err(path)(m)
        }
    };
    let mut rows = st.query([]).map_err(explain)?;
    while let Some(r) = rows.next().map_err(explain)? {
        if raw.len() >= MAX_SQLITE_ROWS {
            return Err(Error::Data(format!("more than {MAX_SQLITE_ROWS} rows; filter in the query, or convert the database to DuckDB or Parquet")));
        }
        let mut row = Vec::with_capacity(n);
        for i in 0..n {
            use rusqlite::types::ValueRef;
            row.push(match r.get_ref(i).map_err(|e| db_err(path)(e.to_string()))? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(x) => Value::Int(x),
                ValueRef::Real(x) => if x.is_finite() { Value::Float(x) } else { Value::Null },
                ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
                ValueRef::Blob(b) => Value::Text(format!("<{} bytes>", b.len())),
            });
        }
        raw.push(row);
    }
    let mut fields = Vec::with_capacity(n);
    for (i, name) in names.iter().enumerate() {
        let col: Vec<&Value> = raw.iter().map(|r| &r[i]).collect();
        fields.push(Field { name: name.clone(), dtype: sqlite_type(&declared[i], &col) });
    }
    // Convert each value to its column's type.
    for row in &mut raw {
        for (v, f) in row.iter_mut().zip(&fields) {
            *v = match (f.dtype, std::mem::replace(v, Value::Null)) {
                (_, Value::Null) => Value::Null,
                (DataType::Bool, Value::Int(x)) => Value::Bool(x != 0),
                (DataType::Float, Value::Int(x)) => Value::Float(x as f64),
                (DataType::Date, Value::Text(s)) => crate::table::parse_date(&s).map(Value::Date).unwrap_or(Value::Null),
                (DataType::Timestamp, Value::Text(s)) => crate::table::parse_timestamp(&s).map(Value::Timestamp).unwrap_or(Value::Null),
                (DataType::Text, x @ (Value::Int(_) | Value::Float(_))) => Value::Text(x.to_string()),
                (_, x) => x,
            };
        }
    }
    let mut seen = std::collections::HashSet::new();
    for f in &fields {
        if !seen.insert(f.name.clone()) {
            return Err(Error::Validation(format!("the query gives two columns named `{}`; rename one with AS", f.name)));
        }
    }
    Ok(Table { schema: Schema { fields }, rows: raw })
}

// -------------------------------------------------------------- the module

/// The tables (and views) a database file holds, by name.
pub fn tables(path: &Path, format: SourceFormat) -> Result<Vec<String>, Error> {
    match format {
        SourceFormat::Sqlite => sqlite_tables(&sqlite_readonly(path)?, path),
        SourceFormat::DuckDb => duck_tables(&duck_readonly(path, None)?, path),
        _ => Ok(vec![]),
    }
}

/// Read a database source into a Parquet file at `raw` (to be adopted by the
/// engine).
pub fn copy_to_parquet(path: &Path, format: SourceFormat, what: Read, raw: &Path) -> Result<(), Error> {
    match format {
        SourceFormat::Sqlite => crate::duck::write_parquet(&sqlite_read(path, what)?, raw),
        SourceFormat::DuckDb => {
            let conn = duck_readonly(path, Some(raw))?;
            let sql = match what {
                Read::Table(t) => {
                    let tables = duck_tables(&conn, path)?;
                    let name = match t {
                        Some(t) if tables.iter().any(|x| x == t) => t.to_string(),
                        Some(t) => return Err(Error::Validation(format!("{} has no table `{t}` (tables: {})", path.display(), tables.join(", ")))),
                        None => tables.first().cloned().ok_or_else(|| Error::Validation(format!("{} has no tables", path.display())))?,
                    };
                    format!("SELECT * FROM {}", quote_ident(&name))
                }
                Read::Query(q) => {
                    let q = q.trim().trim_end_matches(';');
                    crate::duck::check_single_select(&conn, q).map_err(|m| Error::Validation(format!("the query: {m}")))?;
                    q.to_string()
                }
            };
            let lit = quote_literal(&Value::Text(raw.to_string_lossy().into_owned()));
            conn.execute_batch(&format!("COPY ({sql}) TO {lit} (FORMAT parquet)"))
                .map_err(|e| Error::Validation(format!("the query does not run: {e}")))
        }
        f => Err(Error::Unsupported(format!("{} is not a database format", f.name()))),
    }
}
