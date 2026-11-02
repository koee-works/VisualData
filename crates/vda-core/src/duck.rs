//! Embedded DuckDB engine (ADR-0001) and Parquet I/O for the Python boundary.
//!
//! Every query result is normalized to the value types the core models
//! (BOOLEAN, BIGINT, DOUBLE, VARCHAR, DATE, TIMESTAMP) with explicit casts, so
//! results from the reference engine and from DuckDB are directly comparable.
//! Other types (TIMESTAMPTZ, TIME, INTERVAL, LIST, …) are cast to VARCHAR;
//! see STATUS.md.

use crate::compile::{self, quote_ident, quote_literal, CSV_OPTIONS};
use crate::engine::Engine;
use crate::model::{Step, StepKind};
use crate::table::{DataType, Field, Schema, Table, Value};
use crate::Error;
use duckdb::appender_params_from_iter;
use duckdb::types::{TimeUnit, Value as DValue, ValueRef};
use duckdb::Connection;
use std::path::Path;


fn db_err(context: &str) -> impl Fn(duckdb::Error) -> Error + '_ {
    move |e| Error::Data(format!("{context}: {e}"))
}

/// Open an in-memory database configured for the desktop app: Parquet is
/// compiled in, and DuckDB never downloads extensions at run time (ADR-0001:
/// no network required).
fn open() -> Result<Connection, Error> {
    let conn = Connection::open_in_memory().map_err(db_err("cannot start DuckDB"))?;
    // One thread keeps floating-point aggregation order deterministic, so
    // replay hashes are stable. Revisit with a tolerance-based replay check
    // before raising it.
    conn.execute_batch("SET autoinstall_known_extensions = false; SET threads = 1;")
        .map_err(db_err("cannot configure DuckDB"))?;
    Ok(conn)
}

pub struct DuckDbEngine {
    conn: Connection,
    version: String,
}

impl DuckDbEngine {
    pub fn new() -> Result<Self, Error> {
        let conn = open()?;
        let version: String = conn
            .query_row("SELECT version()", [], |r| r.get(0))
            .map_err(db_err("cannot read DuckDB version"))?;
        Ok(DuckDbEngine { conn, version })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Run a query and return its result normalized to core types.
    pub fn query(&self, sql: &str) -> Result<Table, Error> {
        query_table(&self.conn, sql)
    }

    fn load_input(&self, input: &Table) -> Result<(), Error> {
        load_table(&self.conn, "input", input)
    }
}

impl Engine for DuckDbEngine {
    fn fingerprint(&self) -> String {
        format!("duckdb {}", self.version)
    }

    fn import_csv(&mut self, path: &Path) -> Result<Table, Error> {
        let p = quote_literal(&Value::Text(path.to_string_lossy().into_owned()));
        query_table(&self.conn, &format!("SELECT * FROM read_csv({p}, {CSV_OPTIONS})"))
            .map_err(|e| Error::Data(format!("cannot import {}: {e}", path.display())))
    }

    fn run_step(&mut self, step: &Step, input: &Table) -> Result<Table, Error> {
        // Visual steps are validated first so users get the same actionable
        // message as with the reference engine, not a raw SQL error.
        compile::output_schema(step, &input.schema)?;
        let sql = compile::step_sql(&step.kind)
            .ok_or_else(|| Error::Unsupported(format!("step `{}` does not execute as SQL", step.id)))?;
        self.load_input(input)?;
        let result = query_table(&self.conn, &sql);
        let _ = self.conn.execute_batch("DROP TABLE IF EXISTS input;");
        result.map_err(|e| match (&step.kind, e) {
            (StepKind::Sql { .. }, Error::Data(m)) => Error::Validation(format!("step `{}`: {m}", step.id)),
            (_, e) => e,
        })
    }
}

/// DuckDB type name → core type and the cast that normalizes it.
fn normalize(duck_type: &str) -> (DataType, &'static str) {
    let t = duck_type.to_ascii_uppercase();
    match t.as_str() {
        "BOOLEAN" => (DataType::Bool, "BOOLEAN"),
        "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "UTINYINT" | "USMALLINT" | "UINTEGER" | "UBIGINT"
        | "HUGEINT" | "UHUGEINT" => (DataType::Int, "BIGINT"),
        "FLOAT" | "DOUBLE" => (DataType::Float, "DOUBLE"),
        _ if t.starts_with("DECIMAL") => (DataType::Float, "DOUBLE"),
        "DATE" => (DataType::Date, "DATE"),
        // Sub-microsecond precision is truncated; zone-aware timestamps are
        // not modelled yet and fall through to VARCHAR.
        "TIMESTAMP" | "DATETIME" | "TIMESTAMP_S" | "TIMESTAMP_MS" | "TIMESTAMP_NS" => (DataType::Timestamp, "TIMESTAMP"),
        _ => (DataType::Text, "VARCHAR"),
    }
}

pub(crate) fn query_table(conn: &Connection, sql: &str) -> Result<Table, Error> {
    let sql = sql.trim().trim_end_matches(';');
    // Materialize first: a table keeps the query's row order (including any
    // ORDER BY), which a wrapping subquery would not guarantee.
    conn.execute_batch(&format!("CREATE OR REPLACE TEMP TABLE __vda_result AS {sql};"))
        .map_err(db_err("query failed"))?;
    let result = read_result(conn);
    let _ = conn.execute_batch("DROP TABLE IF EXISTS __vda_result;");
    result
}

fn read_result(conn: &Connection) -> Result<Table, Error> {
    let mut describe = conn.prepare("DESCRIBE __vda_result").map_err(db_err("query failed"))?;
    let cols: Vec<(String, String)> = describe
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(db_err("query failed"))?
        .collect::<Result<_, _>>()
        .map_err(db_err("query failed"))?;

    let mut fields = Vec::new();
    let mut projection = Vec::new();
    for (name, ty) in &cols {
        let (dtype, cast) = normalize(ty);
        fields.push(Field { name: name.clone(), dtype });
        projection.push(format!("CAST({} AS {cast})", quote_ident(name)));
    }
    if fields.is_empty() {
        return Ok(Table::default());
    }
    let normalized = format!("SELECT {} FROM __vda_result", projection.join(", "));
    let mut stmt = conn.prepare(&normalized).map_err(db_err("query failed"))?;
    let mut rows = stmt.query([]).map_err(db_err("query failed"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(db_err("query failed"))? {
        let mut vals = Vec::with_capacity(fields.len());
        for i in 0..fields.len() {
            let v = row.get_ref(i).map_err(db_err("cannot read value"))?;
            vals.push(match v {
                ValueRef::Null => Value::Null,
                ValueRef::Boolean(b) => Value::Bool(b),
                ValueRef::BigInt(n) => Value::Int(n),
                ValueRef::Double(f) => Value::Float(f),
                ValueRef::Text(bytes) => Value::Text(String::from_utf8_lossy(bytes).into_owned()),
                ValueRef::Date32(d) => Value::Date(d),
                ValueRef::Timestamp(unit, t) => Value::Timestamp(match unit {
                    TimeUnit::Second => t * 1_000_000,
                    TimeUnit::Millisecond => t * 1_000,
                    TimeUnit::Microsecond => t,
                    TimeUnit::Nanosecond => t / 1_000,
                }),
                other => return Err(Error::Data(format!("unexpected value after normalization: {other:?}"))),
            });
        }
        out.push(vals);
    }
    Ok(Table { schema: Schema { fields }, rows: out })
}

pub(crate) fn load_table(conn: &Connection, name: &str, t: &Table) -> Result<(), Error> {
    let cols: Vec<String> =
        t.schema.fields.iter().map(|f| format!("{} {}", quote_ident(&f.name), f.dtype.name())).collect();
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE {} ({});",
        quote_ident(name),
        cols.join(", ")
    ))
    .map_err(db_err("cannot create input table"))?;
    let mut app = conn.appender(name).map_err(db_err("cannot load input"))?;
    for row in &t.rows {
        let vals = row.iter().map(|v| match v {
            Value::Null => DValue::Null,
            Value::Bool(b) => DValue::Boolean(*b),
            Value::Int(i) => DValue::BigInt(*i),
            Value::Float(f) => DValue::Double(*f),
            Value::Text(s) => DValue::Text(s.clone()),
            Value::Date(d) => DValue::Date32(*d),
            Value::Timestamp(t) => DValue::Timestamp(TimeUnit::Microsecond, *t),
        });
        app.append_row(appender_params_from_iter(vals)).map_err(db_err("cannot load input"))?;
    }
    app.flush().map_err(db_err("cannot load input"))?;
    Ok(())
}

/// Write a table as Parquet (the Python boundary format).
pub fn write_parquet(t: &Table, path: &Path) -> Result<(), Error> {
    let conn = open()?;
    load_table(&conn, "t", t)?;
    let p = quote_literal(&Value::Text(path.to_string_lossy().into_owned()));
    conn.execute_batch(&format!("COPY t TO {p} (FORMAT parquet);"))
        .map_err(db_err("cannot write Parquet"))
}

pub fn read_parquet(path: &Path) -> Result<Table, Error> {
    let conn = open()?;
    let p = quote_literal(&Value::Text(path.to_string_lossy().into_owned()));
    query_table(&conn, &format!("SELECT * FROM read_parquet({p})"))
}
