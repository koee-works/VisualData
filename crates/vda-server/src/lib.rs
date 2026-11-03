//! Exploration: reading a table or a query from a PostgreSQL server into the
//! Parquet copy a source is held as.
//!
//! How a read works:
//! 1. `prepare` the query (one statement, nothing runs) to learn its columns
//!    and their server types.
//! 2. Inside a READ ONLY transaction, with UTC, ISO dates and exact floats,
//!    stream `COPY (SELECT <conversions> FROM (<query>) q) TO STDOUT (CSV)`
//!    to a temporary file. The server converts its types to the app's six
//!    (money to numeric, timestamptz to UTC, the rest to text), so memory
//!    stays flat whatever the size.
//! 3. DuckDB reads the CSV with exact column types and writes Parquet.

use postgres::types::{Kind, Type};
use postgres::{Client, Config, NoTls};
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct PgSource {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    /// Server-side limit for one read.
    pub timeout: Duration,
}

#[derive(Debug, Clone, Copy)]
pub enum Read<'a> {
    Table(&'a str),
    Query(&'a str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    /// The server's type, e.g. `numeric`, `timestamptz`.
    pub server_type: String,
    /// BOOLEAN, BIGINT, DOUBLE, VARCHAR, DATE or TIMESTAMP.
    pub app_type: &'static str,
    /// How the value was changed on the way, if it was.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FetchReport {
    pub rows: u64,
    /// SHA-256 of the rows as they streamed: the snapshot's fingerprint.
    /// Equal reads give equal hashes, so an unchanged table outdates nothing.
    pub sha256: String,
    /// The order the rows came in: by primary key for a table, else none.
    pub ordered_by: Option<Vec<String>>,
    pub columns: Vec<Column>,
    pub bytes_streamed: u64,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub enum Error {
    /// Cannot reach or sign in to the server.
    Connect(String),
    /// The query is not acceptable or failed on the server.
    Query(String),
    Cancelled,
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Connect(m) => write!(f, "cannot connect: {m}"),
            Error::Query(m) => write!(f, "{m}"),
            Error::Cancelled => write!(f, "cancelled"),
            Error::Io(m) => write!(f, "{m}"),
        }
    }
}

fn pg_message(e: &postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => match db.hint() {
            Some(h) => format!("{} ({h})", db.message()),
            None => db.message().to_string(),
        },
        None => {
            let s = e.to_string();
            let s = s.strip_prefix("db error: ").unwrap_or(&s).to_string();
            s
        }
    }
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn connect(src: &PgSource, password: &str) -> Result<Client, Error> {
    let mut c = Config::new();
    c.host(&src.host)
        .port(src.port)
        .dbname(&src.database)
        .user(&src.user)
        .password(password)
        .application_name("vda")
        .connect_timeout(Duration::from_secs(10))
        // Every transaction this connection starts is read-only.
        .options(&format!(
            "-c default_transaction_read_only=on -c statement_timeout={} -c TimeZone=UTC -c DateStyle=ISO -c IntervalStyle=iso_8601 -c extra_float_digits=3",
            src.timeout.as_millis()
        ));
    c.connect(NoTls).map_err(|e| Error::Connect(pg_message(&e)))
}

/// Tables and views the user can read, as `schema.name` (just `name` in `public`).
pub fn tables(client: &mut Client) -> Result<Vec<String>, Error> {
    let rows = client
        .query(
            "SELECT table_schema, table_name FROM information_schema.tables
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
               AND has_table_privilege(quote_ident(table_schema) || '.' || quote_ident(table_name), 'SELECT')
             ORDER BY table_schema <> 'public', table_schema, table_name",
            &[],
        )
        .map_err(|e| Error::Query(pg_message(&e)))?;
    Ok(rows
        .iter()
        .map(|r| {
            let (s, t): (String, String) = (r.get(0), r.get(1));
            if s == "public" { t } else { format!("{s}.{t}") }
        })
        .collect())
}

/// How a server type reaches the app: the expression applied to the column
/// on the server, the app type, and a note when something is lost.
fn conversion(col: &str, t: &Type) -> (String, &'static str, Option<String>) {
    let c = quote_ident(col);
    let base = match t.kind() {
        Kind::Domain(inner) => inner.clone(),
        _ => t.clone(),
    };
    match base {
        Type::BOOL => (c, "BOOLEAN", None),
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => (c, "BIGINT", None),
        Type::FLOAT4 | Type::FLOAT8 => (
            format!("CASE WHEN {c} IN ('NaN', 'Infinity', '-Infinity') THEN NULL ELSE {c} END"),
            "DOUBLE",
            Some("NaN and infinite values become missing".into()),
        ),
        Type::NUMERIC => (
            format!("CASE WHEN {c} IN ('NaN', 'Infinity', '-Infinity') THEN NULL ELSE {c} END"),
            "DOUBLE",
            Some("exact numbers become decimals: about 15 significant digits are kept".into()),
        ),
        Type::MONEY => (format!("{c}::numeric"), "DOUBLE", Some("money becomes a decimal, without the currency".into())),
        Type::DATE => (format!("CASE WHEN isfinite({c}) THEN {c} END"), "DATE", None),
        Type::TIMESTAMP => (format!("CASE WHEN isfinite({c}) THEN {c} END"), "TIMESTAMP", None),
        Type::TIMESTAMPTZ => (
            format!("CASE WHEN isfinite({c}) THEN {c} AT TIME ZONE 'UTC' END"),
            "TIMESTAMP",
            Some("times with a time zone are converted to UTC".into()),
        ),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::CHAR => (c, "VARCHAR", None),
        Type::BYTEA => (format!("'<' || length({c}) || ' bytes>'"), "VARCHAR", Some("binary values are shown by their size".into())),
        _ => {
            let note = match base.kind() {
                Kind::Enum(_) => None,
                _ => Some(format!("{} values become text", base.name())),
            };
            (format!("{c}::text"), "VARCHAR", note)
        }
    }
}

/// Read a table or one query into `out` (Parquet), streaming through a
/// temporary CSV file in `work`.
pub fn fetch(client: &mut Client, what: Read, out: &Path, work: &Path, cancel: &Arc<AtomicBool>) -> Result<FetchReport, Error> {
    let started = Instant::now();
    let mut ordered_by = None;
    let query = match what {
        Read::Table(t) => {
            let qualified = match t.split_once('.') {
                Some((s, n)) => format!("{}.{}", quote_ident(s), quote_ident(n)),
                None => quote_ident(t),
            };
            // By primary key, so the same rows always stream in the same
            // order and give the same fingerprint.
            let key: Vec<String> = client
                .query(
                    "SELECT a.attname FROM pg_index i JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
                     WHERE i.indrelid = $1::text::regclass AND i.indisprimary ORDER BY array_position(i.indkey, a.attnum)",
                    &[&qualified],
                )
                .map(|rows| rows.iter().map(|r| r.get::<_, String>(0)).collect())
                .unwrap_or_default();
            if key.is_empty() {
                format!("SELECT * FROM {qualified}")
            } else {
                let order = key.iter().map(|k| quote_ident(k)).collect::<Vec<_>>().join(", ");
                ordered_by = Some(key);
                format!("SELECT * FROM {qualified} ORDER BY {order}")
            }
        }
        Read::Query(q) => q.trim().trim_end_matches(';').to_string(),
    };
    // One statement only: Parse in the extended protocol rejects more, and
    // nothing runs yet.
    let stmt = client.prepare(&query).map_err(|e| Error::Query(format!("the query does not run: {}", pg_message(&e))))?;
    if stmt.columns().is_empty() {
        return Err(Error::Query("the query must return rows (a SELECT)".into()));
    }
    let mut columns = Vec::new();
    let mut select = Vec::new();
    for (i, col) in stmt.columns().iter().enumerate() {
        if stmt.columns()[..i].iter().any(|c| c.name() == col.name()) {
            return Err(Error::Query(format!("the query gives two columns named `{}`; rename one with AS", col.name())));
        }
        let (expr, app_type, note) = conversion(col.name(), col.type_());
        select.push(format!("{expr} AS {}", quote_ident(col.name())));
        columns.push(Column { name: col.name().to_string(), server_type: col.type_().name().to_string(), app_type, note });
    }

    // Cancel from another thread with the server's cancel request.
    let token = client.cancel_token();
    let flag = cancel.clone();
    let done = Arc::new(AtomicBool::new(false));
    let done2 = done.clone();
    let watcher = std::thread::spawn(move || {
        while !done2.load(Ordering::SeqCst) {
            if flag.load(Ordering::SeqCst) {
                let _ = token.cancel_query(NoTls);
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });

    let csv = work.join(format!("pg-{}-{}.csv", std::process::id(), started.elapsed().as_nanos()));
    let mut hash = vda_core::hash::Sha256::new();
    let result = (|| -> Result<u64, Error> {
        let mut tx = client.build_transaction().read_only(true).start().map_err(|e| Error::Query(pg_message(&e)))?;
        let copy = format!("COPY (SELECT {} FROM ({query}) AS q) TO STDOUT WITH (FORMAT csv)", select.join(", "));
        let mut reader = tx.copy_out(copy.as_str()).map_err(|e| {
            if cancel.load(Ordering::SeqCst) {
                return Error::Cancelled;
            }
            // The query is read as a subquery, so anything but a query
            // (INSERT, DELETE, …) cannot even be parsed there.
            let code = e.code().map(|c| c.code().to_string()).unwrap_or_default();
            if code == "42601" || code == "0A000" {
                Error::Query("the query must be a SELECT (or WITH … SELECT); it cannot change the database".into())
            } else {
                Error::Query(pg_message(&e))
            }
        })?;
        let mut file = std::io::BufWriter::new(std::fs::File::create(&csv).map_err(|e| Error::Io(e.to_string()))?);
        let mut buf = vec![0u8; 1 << 16];
        let mut bytes = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| {
                if cancel.load(Ordering::SeqCst) {
                    return Error::Cancelled;
                }
                // Errors while rows stream arrive wrapped in an I/O error.
                match e.get_ref().and_then(|inner| inner.downcast_ref::<postgres::Error>()) {
                    Some(pg) => Error::Query(pg_message(pg)),
                    None => Error::Query(e.to_string()),
                }
            })?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            hash.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| Error::Io(e.to_string()))?;
        }
        file.flush().map_err(|e| Error::Io(e.to_string()))?;
        drop(reader);
        tx.commit().map_err(|e| Error::Query(pg_message(&e)))?;
        Ok(bytes)
    })();
    done.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    let bytes = match result {
        Ok(b) => b,
        Err(e) => {
            let _ = std::fs::remove_file(&csv);
            return Err(e);
        }
    };

    // CSV to Parquet with the exact types. Unquoted empty fields are
    // missing; quoted ones ("") are empty text.
    let conv = (|| -> Result<u64, String> {
        let db = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        // A cap keeps memory flat: DuckDB otherwise uses what it is allowed.
        db.execute_batch("SET threads = 2; SET memory_limit = '256MB'; SET preserve_insertion_order = true; SET autoinstall_known_extensions = false; SET autoload_known_extensions = false;").map_err(|e| e.to_string())?;
        let lit = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "''"));
        let cols: Vec<String> = columns.iter().map(|c| format!("'{}': '{}'", c.name.replace('\'', "''"), c.app_type)).collect();
        let sql = format!(
            "COPY (SELECT * FROM read_csv({}, columns = {{{}}}, header = false, quote = '\"', escape = '\"', allow_quoted_nulls = false, auto_detect = false)) TO {} (FORMAT parquet)",
            lit(&csv),
            cols.join(", "),
            lit(out)
        );
        db.execute_batch(&sql).map_err(|e| e.to_string())?;
        db.query_row(&format!("SELECT count(*) FROM read_parquet({})", lit(out)), [], |r| r.get::<_, i64>(0)).map(|n| n as u64).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(&csv);
    let rows = conv.map_err(Error::Io)?;
    // Column names and types are part of what was read.
    for c in &columns {
        hash.update(format!("\n{}:{}", c.name, c.app_type).as_bytes());
    }
    Ok(FetchReport { rows, sha256: hash.hex_digest(), ordered_by, columns, bytes_streamed: bytes, elapsed: started.elapsed() })
}
