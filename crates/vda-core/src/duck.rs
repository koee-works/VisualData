//! Embedded DuckDB engine (ADR-0001) and Parquet I/O.
//!
//! Results never pass through memory as whole tables: every step reads its
//! input Parquet file with `read_parquet` and writes its output with `COPY …
//! TO`. Only bounded previews, profiles and chart data are materialized.
//!
//! Every result is normalized to the value types the core models (BOOLEAN,
//! BIGINT, DOUBLE, VARCHAR, DATE, TIMESTAMP), so results from the reference
//! engine and from DuckDB are directly comparable. Other types (TIMESTAMPTZ,
//! TIME, INTERVAL, LIST, …) are cast to VARCHAR; see STATUS.md.

use crate::compile::{self, quote_ident, quote_literal, CSV_OPTIONS};
use crate::dataset::Dataset;
use crate::engine::Engine;
use crate::hash::sha256_file;
use crate::model::{Step, StepKind};
use crate::table::{ColumnProfile, DataType, Field, Schema, Table, Value};
use crate::Error;
use duckdb::appender_params_from_iter;
use duckdb::types::{TimeUnit, Value as DValue, ValueRef};
use duckdb::Connection;
use std::fs;
use std::path::{Path, PathBuf};

fn db_err(context: &str) -> impl Fn(duckdb::Error) -> Error + '_ {
    move |e| Error::Data(format!("{context}: {e}"))
}

fn lit(path: &Path) -> String {
    quote_literal(&Value::Text(path.to_string_lossy().into_owned()))
}

/// Resource settings for the embedded database.
#[derive(Debug, Clone, Default)]
pub struct EngineOptions {
    /// Memory DuckDB may use before spilling to `temp_dir`. `None` keeps
    /// DuckDB's default (80% of physical memory).
    pub memory_limit_mb: Option<u64>,
    /// Where intermediate data spills. Defaults to `<system temp>/vda-duckdb`.
    pub temp_dir: Option<PathBuf>,
    /// When set, DuckDB may only read and write files under these paths
    /// (directories, or single files such as a source CSV kept elsewhere),
    /// plus the spill directory. The configuration is then locked, so no
    /// query can lift the restriction (ADR-0004).
    pub allowed_paths: Option<Vec<PathBuf>>,
}

/// Open an in-memory database configured for the desktop app: Parquet is
/// compiled in, DuckDB never downloads extensions at run time (ADR-0001: no
/// network required), and large operations spill to disk.
fn open(opts: &EngineOptions) -> Result<Connection, Error> {
    let conn = Connection::open_in_memory().map_err(db_err("cannot start DuckDB"))?;
    let temp = opts.temp_dir.clone().unwrap_or_else(|| std::env::temp_dir().join("vda-duckdb"));
    fs::create_dir_all(&temp)?;
    // One thread keeps output files byte-identical across runs (replay
    // compares file hashes) and floating-point aggregation order stable.
    let mut sql = format!(
        "SET autoinstall_known_extensions = false; SET threads = 1; \
         SET preserve_insertion_order = true; SET temp_directory = {};",
        lit(&temp)
    );
    if let Some(mb) = opts.memory_limit_mb {
        sql.push_str(&format!(" SET memory_limit = '{mb}MB';"));
    }
    if let Some(paths) = &opts.allowed_paths {
        let mut dirs = vec![temp.clone()];
        let mut files = Vec::new();
        for p in paths {
            let p = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            if p.is_dir() {
                dirs.push(p);
            } else {
                files.push(p);
            }
        }
        // A trailing separator matches the directory's contents, not
        // siblings that merely share its name as a prefix.
        let list = |ps: &[PathBuf], dir: bool| {
            ps.iter()
                .map(|p| {
                    let mut s = fs::canonicalize(p).unwrap_or_else(|_| p.clone()).to_string_lossy().into_owned();
                    if dir && !s.ends_with(std::path::MAIN_SEPARATOR) {
                        s.push(std::path::MAIN_SEPARATOR);
                    }
                    quote_literal(&Value::Text(s))
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        sql.push_str(&format!(" SET allowed_directories = [{}];", list(&dirs, true)));
        if !files.is_empty() {
            sql.push_str(&format!(" SET allowed_paths = [{}];", list(&files, false)));
        }
        sql.push_str(" SET enable_external_access = false; SET lock_configuration = true;");
    }
    conn.execute_batch(&sql).map_err(db_err("cannot configure DuckDB"))?;
    Ok(conn)
}

pub struct DuckDbEngine {
    conn: Connection,
    version: String,
}

impl DuckDbEngine {
    pub fn new() -> Result<Self, Error> {
        Self::with_options(EngineOptions::default())
    }

    pub fn with_options(opts: EngineOptions) -> Result<Self, Error> {
        let conn = open(&opts)?;
        let version: String = conn
            .query_row("SELECT version()", [], |r| r.get(0))
            .map_err(db_err("cannot read DuckDB version"))?;
        Ok(DuckDbEngine { conn, version })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Run a query and return its result normalized to core types. Only for
    /// results known to be small.
    pub fn query(&self, sql: &str) -> Result<Table, Error> {
        query_table(&self.conn, sql)
    }

    /// Write the result of `sql` to `out` as normalized Parquet.
    fn copy_to(&self, sql: &str, out: &Path) -> Result<Dataset, Error> {
        let sql = sql.trim().trim_end_matches(';');
        // Parquet has no 128-bit integers: DuckDB would write HUGEINT (what
        // SUM over BIGINT returns) as DOUBLE, silently turning integer sums
        // into floats and hiding overflow. Such results are cast before
        // writing instead, via a temp table so row order is kept; the cast
        // fails loudly on overflow.
        if let Ok(cols) = describe(&self.conn, sql) {
            if cols.iter().any(|(_, ty)| matches!(ty.as_str(), "HUGEINT" | "UHUGEINT")) {
                self.conn
                    .execute_batch(&format!("CREATE OR REPLACE TEMP TABLE __vda_wide AS {sql};"))
                    .map_err(db_err("query failed"))?;
                let projection: Vec<String> = cols
                    .iter()
                    .map(|(n, ty)| format!("CAST({} AS {}) AS {}", quote_ident(n), normalize(ty).1, quote_ident(n)))
                    .collect();
                let copied = self
                    .conn
                    .execute_batch(&format!(
                        "COPY (SELECT {} FROM __vda_wide) TO {} (FORMAT parquet);",
                        projection.join(", "),
                        lit(out)
                    ))
                    .map_err(db_err("query failed"));
                let _ = self.conn.execute_batch("DROP TABLE IF EXISTS __vda_wide;");
                copied?;
                return self.dataset_info(out);
            }
        }
        let raw = out.with_extension("raw.parquet");
        // COPY keeps the query's row order, including a final ORDER BY.
        self.conn
            .execute_batch(&format!("COPY ({sql}) TO {} (FORMAT parquet);", lit(&raw)))
            .map_err(db_err("query failed"))?;
        let result = self.normalize_file(&raw, out);
        let _ = fs::remove_file(&raw);
        result
    }

    /// Rewrite a Parquet file with core types (a rename when it already has them).
    fn normalize_file(&self, src: &Path, out: &Path) -> Result<Dataset, Error> {
        let cols = describe(&self.conn, &format!("SELECT * FROM read_parquet({})", lit(src)))?;
        if cols.iter().all(|(_, ty)| normalize(ty).1 == ty.as_str()) {
            fs::rename(src, out)?;
        } else {
            let projection: Vec<String> = cols
                .iter()
                .map(|(n, ty)| format!("CAST({} AS {}) AS {}", quote_ident(n), normalize(ty).1, quote_ident(n)))
                .collect();
            // A single-threaded scan of one file reads rows in file order.
            self.conn
                .execute_batch(&format!(
                    "COPY (SELECT {} FROM read_parquet({})) TO {} (FORMAT parquet);",
                    projection.join(", "),
                    lit(src),
                    lit(out)
                ))
                .map_err(db_err("cannot normalize result"))?;
        }
        self.dataset_info(out)
    }

    /// Schema, row count, size and hash of a normalized Parquet file.
    pub fn dataset_info(&self, path: &Path) -> Result<Dataset, Error> {
        let src = format!("read_parquet({})", lit(path));
        let fields = describe(&self.conn, &format!("SELECT * FROM {src}"))?
            .into_iter()
            .map(|(name, ty)| Field { name, dtype: normalize(&ty).0 })
            .collect();
        // Answered from Parquet metadata; no data pages are read.
        let rows: i64 = self
            .conn
            .query_row(&format!("SELECT count(*) FROM {src}"), [], |r| r.get(0))
            .map_err(db_err("cannot count rows"))?;
        Ok(Dataset {
            path: path.to_path_buf(),
            schema: Schema { fields },
            rows: rows as u64,
            bytes: fs::metadata(path)?.len(),
            content_hash: sha256_file(path)?,
        })
    }
}

impl Engine for DuckDbEngine {
    fn fingerprint(&self) -> String {
        format!("duckdb {}", self.version)
    }

    fn import_csv(&mut self, src: &Path, out: &Path) -> Result<Dataset, Error> {
        self.copy_to(&format!("SELECT * FROM read_csv({}, {CSV_OPTIONS})", lit(src)), out)
            .map_err(|e| Error::Data(format!("cannot import {}: {e}", src.display())))
    }

    fn run_step(&mut self, step: &Step, inputs: &[Dataset], out: &Path) -> Result<Dataset, Error> {
        if inputs.len() != step.inputs.len() || inputs.is_empty() {
            return Err(Error::Validation(format!("step `{}`: inputs do not match its definition", step.id)));
        }
        // Visual steps are validated first so users get the same actionable
        // message as with the reference engine, not a raw SQL error.
        let schemas: Vec<_> = inputs.iter().map(|d| d.schema.clone()).collect();
        compile::output_schema(step, &schemas)?;
        let sql = compile::step_sql_for(step, &schemas)?
            .ok_or_else(|| Error::Unsupported(format!("step `{}` does not execute as SQL", step.id)))?;
        if let StepKind::Sql { .. } = step.kind {
            check_single_select(&self.conn, &sql).map_err(|m| Error::Validation(format!("step `{}`: {m}", step.id)))?;
        }
        // The first input is `input`; every input is also visible by its id.
        let mut views = vec![("input".to_string(), &inputs[0])];
        views.extend(step.inputs.iter().map(|id| quote_ident(id)).zip(inputs));
        let create: String = views
            .iter()
            .map(|(name, ds)| format!("CREATE OR REPLACE TEMP VIEW {name} AS SELECT * FROM read_parquet({});", lit(&ds.path)))
            .collect();
        let drop: String = views.iter().map(|(name, _)| format!("DROP VIEW IF EXISTS {name};")).collect();
        if let Err(e) = self.conn.execute_batch(&create) {
            let _ = self.conn.execute_batch(&drop);
            return Err(db_err("cannot read input")(e));
        }
        let result = self.copy_to(&sql, out);
        let _ = self.conn.execute_batch(&drop);
        result.map_err(|e| match (&step.kind, e) {
            (StepKind::Sql { .. }, Error::Data(m)) => Error::Validation(format!("step `{}`: {m}", step.id)),
            (_, e) => e,
        })
    }

    fn adopt_parquet(&mut self, src: &Path, out: &Path) -> Result<Dataset, Error> {
        self.normalize_file(src, out)
    }

    fn preview(&mut self, ds: &Dataset, offset: u64, limit: u64) -> Result<Table, Error> {
        query_table(
            &self.conn,
            &format!("SELECT * FROM read_parquet({}) LIMIT {limit} OFFSET {offset}", lit(&ds.path)),
        )
    }

    fn columns(&mut self, ds: &Dataset, columns: &[&str], limit: u64) -> Result<Table, Error> {
        let cols: Vec<String> = columns.iter().map(|c| quote_ident(c)).collect();
        query_table(
            &self.conn,
            &format!("SELECT {} FROM read_parquet({}) LIMIT {limit}", cols.join(", "), lit(&ds.path)),
        )
    }

    fn profile(&mut self, ds: &Dataset) -> Result<Vec<ColumnProfile>, Error> {
        if ds.schema.fields.is_empty() {
            return Ok(Vec::new());
        }
        // One pass over the data for all columns.
        let mut exprs = Vec::new();
        for f in &ds.schema.fields {
            let c = quote_ident(&f.name);
            exprs.push(format!("count(*) - count({c})"));
            exprs.push(format!("count(DISTINCT {c})"));
            exprs.push(format!("min({c})"));
            exprs.push(format!("max({c})"));
        }
        let t = query_table(&self.conn, &format!("SELECT {} FROM read_parquet({})", exprs.join(", "), lit(&ds.path)))?;
        let row = &t.rows[0];
        Ok(ds
            .schema
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let int = |v: &Value| if let Value::Int(n) = v { *n as usize } else { 0 };
                let opt = |v: &Value| (!v.is_null()).then(|| v.clone());
                ColumnProfile {
                    name: f.name.clone(),
                    dtype: f.dtype,
                    rows: ds.rows as usize,
                    nulls: int(&row[4 * i]),
                    distinct: int(&row[4 * i + 1]),
                    min: opt(&row[4 * i + 2]),
                    max: opt(&row[4 * i + 3]),
                }
            })
            .collect())
    }
}

/// Custom SQL must be exactly one read-only query. DuckDB's own parser
/// decides: `json_serialize_sql` accepts only SELECT statements (including
/// WITH and FROM-first forms), so COPY, ATTACH, SET, INSTALL, PRAGMA and
/// similar are refused before anything runs, and a second statement cannot
/// be smuggled in after the first.
pub fn check_single_select(conn: &Connection, sql: &str) -> Result<(), String> {
    let sql = sql.trim().trim_end_matches(';');
    let raw: String = conn
        .query_row("SELECT json_serialize_sql(?)", [sql], |r| r.get(0))
        .map_err(|e| format!("could not check the query: {e}"))?;
    let parsed = crate::json::Json::parse(&raw).map_err(|e| e.to_string())?;
    if parsed.get("error") == Some(&crate::json::Json::Bool(true)) {
        let message = parsed.get("error_message").and_then(|m| m.as_str()).unwrap_or("");
        return Err(if parsed.get("error_type").and_then(|t| t.as_str()) == Some("parser") {
            format!("the query does not parse: {message}")
        } else {
            "custom SQL must be one query that reads data (SELECT, WITH or FROM). \
             Writing files, changing settings and attaching databases are not allowed."
                .into()
        });
    }
    match parsed.get("statements").and_then(|s| s.as_arr()).map(Vec::len) {
        Some(1) => Ok(()),
        _ => Err("custom SQL must be a single query; remove the extra statements".into()),
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

fn describe(conn: &Connection, sql: &str) -> Result<Vec<(String, String)>, Error> {
    let mut stmt = conn.prepare(&format!("DESCRIBE {sql}")).map_err(db_err("query failed"))?;
    let cols = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(db_err("query failed"))?
        .collect::<Result<_, _>>()
        .map_err(db_err("query failed"))?;
    Ok(cols)
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
    let cols = describe(conn, "__vda_result")?;
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

fn load_table(conn: &Connection, name: &str, t: &Table) -> Result<(), Error> {
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

/// Write an in-memory table as Parquet. For small tables (tests, reference
/// engine); large results never exist as in-memory tables.
pub fn write_parquet(t: &Table, path: &Path) -> Result<(), Error> {
    let conn = open(&EngineOptions::default())?;
    load_table(&conn, "t", t)?;
    conn.execute_batch(&format!("COPY t TO {} (FORMAT parquet);", lit(path)))
        .map_err(db_err("cannot write Parquet"))
}

/// Read a whole Parquet file into memory. For small files only.
pub fn read_parquet(path: &Path) -> Result<Table, Error> {
    let conn = open(&EngineOptions::default())?;
    query_table(&conn, &format!("SELECT * FROM read_parquet({})", lit(path)))
}
