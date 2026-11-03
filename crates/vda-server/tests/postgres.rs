//! Against a real PostgreSQL server: set VDA_TEST_PG=host:port (users
//! `analyst`/`secret-pw`, a superuser, and `reader`/`reader-pw`, SELECT only,
//! on a database `shop` seeded as in the exploration notes).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use vda_server::*;

fn server() -> Option<(String, u16)> {
    let v = std::env::var("VDA_TEST_PG").ok()?;
    let (h, p) = v.split_once(':')?;
    Some((h.to_string(), p.parse().ok()?))
}

fn src(user: &str) -> Option<PgSource> {
    let (host, port) = server()?;
    Some(PgSource { host, port, database: "shop".into(), user: user.into(), timeout: Duration::from_secs(60) })
}

fn work() -> PathBuf {
    let d = std::env::temp_dir().join(format!("vda-pg-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn no() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn duck(out: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    let db = duckdb::Connection::open_in_memory().unwrap();
    let q = format!("SELECT COLUMNS(*)::VARCHAR FROM ({})", sql.replace("{t}", &format!("read_parquet('{}')", out.display())));
    let mut st = db.prepare(&q).unwrap();
    let n = st.column_count();
    let _ = n;
    let rows = st
        .query_map([], |r| {
            let n = r.as_ref().column_count();
            Ok((0..n).map(|i| r.get::<_, Option<String>>(i).unwrap_or(None).unwrap_or_else(|| "NULL".into())).collect::<Vec<_>>())
        })
        .unwrap();
    rows.map(|r| r.unwrap()).collect()
}

fn peak_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM")).map(|l| l.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap() / 1024))
        .unwrap_or(0)
}

#[test]
fn a_table_streams_to_parquet_with_flat_memory() {
    let Some(s) = src("reader") else { return eprintln!("skipped: set VDA_TEST_PG") };
    let mut c = connect(&s, "reader-pw").unwrap();
    let t = tables(&mut c).unwrap();
    assert!(t.contains(&"orders".to_string()) && t.contains(&"east".to_string()), "{t:?}");
    let out = work().join("orders.parquet");
    let before = peak_rss_mb();
    let started = Instant::now();
    let r = fetch(&mut c, Read::Table("orders"), &out, &work(), &no()).unwrap();
    eprintln!(
        "500k rows: {:?} total, {} MB streamed, peak RSS {} MB (before {} MB), Parquet {} KB",
        started.elapsed(),
        r.bytes_streamed / 1_000_000,
        peak_rss_mb(),
        before,
        std::fs::metadata(&out).unwrap().len() / 1024
    );
    assert_eq!(r.rows, 500_000);
    let types: Vec<&str> = r.columns.iter().map(|c| c.app_type).collect();
    assert_eq!(types, ["BIGINT", "DATE", "VARCHAR", "VARCHAR", "BIGINT", "DOUBLE", "DOUBLE"]);
    // The same totals as the server's own.
    let server: (i64, i64) = {
        let row = c.query_one("SELECT count(*) - count(revenue), sum(units)::bigint FROM orders", &[]).unwrap();
        (row.get(0), row.get(1))
    };
    assert_eq!(duck(&out, "SELECT count(*) - count(revenue), sum(units) FROM {t}"), vec![vec![server.0.to_string(), server.1.to_string()]]);
}

#[test]
fn server_types_become_the_apps_six() {
    let Some(s) = src("reader") else { return eprintln!("skipped: set VDA_TEST_PG") };
    let mut c = connect(&s, "reader-pw").unwrap();
    let out = work().join("odd.parquet");
    let r = fetch(&mut c, Read::Query("SELECT * FROM odd_types ORDER BY id"), &out, &work(), &no()).unwrap();
    let got: Vec<(String, &str)> = r.columns.iter().map(|c| (c.name.clone(), c.app_type)).collect();
    eprintln!("{:#?}", r.columns);
    let want = [
        ("id", "BIGINT"), ("flag", "BOOLEAN"), ("small", "BIGINT"), ("big", "BIGINT"), ("real_n", "DOUBLE"), ("dbl", "DOUBLE"),
        ("exact", "DOUBLE"), ("money_amt", "DOUBLE"), ("label", "VARCHAR"), ("note", "VARCHAR"), ("day", "DATE"),
        ("at_local", "TIMESTAMP"), ("at_tz", "TIMESTAMP"), ("clock", "VARCHAR"), ("span", "VARCHAR"), ("uid", "VARCHAR"),
        ("doc", "VARCHAR"), ("tags", "VARCHAR"), ("raw", "VARCHAR"), ("ip", "VARCHAR"),
    ];
    assert_eq!(got, want.iter().map(|(a, b)| (a.to_string(), *b)).collect::<Vec<_>>());
    let rows = duck(&out, "SELECT * FROM {t} ORDER BY id");
    eprintln!("{rows:#?}");
    let r0 = &rows[0];
    assert_eq!(r0[3], "9223372036854775807", "bigint exact");
    assert_eq!(r0[7], "19.99", "money");
    assert_eq!(r0[9], "O'Hare");
    assert_eq!(r0[12], "2026-07-01 08:30:00", "timestamptz in UTC");
    assert_eq!(r0[14], "P1DT2H", "interval in ISO 8601");
    assert_eq!(r0[16], r#"{"k": [1, 2]}"#);
    assert_eq!(r0[17], "{x,y}");
    assert_eq!(r0[18], "<2 bytes>");
    let r1 = &rows[1];
    assert_eq!((r1[1].as_str(), r1[4].as_str(), r1[5].as_str()), ("NULL", "NULL", "NULL"), "NaN and Infinity become missing");
    assert_eq!(r1[10], "1969-12-31");
    assert_eq!(r1[12], "1969-12-31 23:59:59.5");
    assert_eq!(r1[16], "null", "JSON null is the text null, not missing");
    assert_eq!(r1[9], "NULL");
}

#[test]
fn nothing_can_change_the_database_even_as_a_superuser() {
    let Some(s) = src("analyst") else { return eprintln!("skipped: set VDA_TEST_PG") };
    let mut c = connect(&s, "secret-pw").unwrap();
    let out = work().join("x.parquet");
    for (q, want) in [
        ("DELETE FROM orders WHERE order_id = 1 RETURNING *", "must be a SELECT"),
        ("WITH d AS (DELETE FROM orders WHERE order_id = 1 RETURNING *) SELECT * FROM d", "must be a SELECT"),
        ("SELECT * FROM orders FOR UPDATE", "read-only transaction"),
        ("SELECT 1; DROP TABLE orders", "cannot insert multiple commands"),
        ("CREATE TABLE t AS SELECT 1 AS x", "must return rows"),
        ("SELECT lo_import('/etc/hostname')", "read-only transaction"),
        ("SELECT nextval('odd_types_id_seq')", "read-only transaction"),
        ("SELECT * FROM nope", "does not exist"),
    ] {
        let e = fetch(&mut c, Read::Query(q), &out, &work(), &no()).unwrap_err().to_string();
        assert!(e.contains(want), "{q}\n  -> {e}");
    }
    let n: i64 = c.query_one("SELECT count(*) FROM orders", &[]).unwrap().get(0);
    assert_eq!(n, 500_000, "nothing was deleted");
}

#[test]
fn permissions_passwords_timeouts_and_cancel() {
    let Some(s) = src("reader") else { return eprintln!("skipped: set VDA_TEST_PG") };
    let e = connect(&s, "wrong").err().unwrap().to_string();
    eprintln!("wrong password: {e}");
    assert!(e.contains("password authentication failed for user \"reader\""), "{e}");
    let mut c = connect(&s, "reader-pw").unwrap();
    let out = work().join("y.parquet");

    // The server's own permissions decide what a user can read.
    // Set up as an administrator, outside the app's read-only connection.
    let mut admin = postgres::Client::connect(&format!("host={} port={} user=analyst password=secret-pw dbname=shop", s.host, s.port), postgres::NoTls).unwrap();
    admin.batch_execute("DROP TABLE IF EXISTS salaries; CREATE TABLE salaries (who text, pay int)").unwrap();
    let e = fetch(&mut c, Read::Table("salaries"), &out, &work(), &no()).unwrap_err().to_string();
    assert!(e.contains("permission denied for table salaries"), "{e}");

    // A server-side time limit.
    let quick = PgSource { timeout: Duration::from_millis(500), ..s.clone() };
    let mut q = connect(&quick, "reader-pw").unwrap();
    let e = fetch(&mut q, Read::Query("SELECT pg_sleep(5)"), &out, &work(), &no()).unwrap_err().to_string();
    assert!(e.contains("statement timeout"), "{e}");

    // Cancel from another thread stops the server's work promptly.
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        f2.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let t = Instant::now();
    let e = fetch(&mut c, Read::Query("SELECT pg_sleep(20)"), &out, &work(), &flag).unwrap_err();
    assert!(matches!(e, Error::Cancelled), "{e}");
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    // The connection is usable after a cancel.
    assert_eq!(fetch(&mut c, Read::Query("SELECT 1 AS one"), &out, &work(), &no()).unwrap().rows, 1);
}

#[test]
fn an_unchanged_table_has_the_same_fingerprint_and_a_changed_row_does_not() {
    let Some(s) = src("reader") else { return eprintln!("skipped: set VDA_TEST_PG") };
    let mut c = connect(&s, "reader-pw").unwrap();
    let out = work().join("fp.parquet");
    let a = fetch(&mut c, Read::Table("orders"), &out, &work(), &no()).unwrap();
    let b = fetch(&mut c, Read::Table("orders"), &out, &work(), &no()).unwrap();
    assert_eq!(a.ordered_by.as_deref(), Some(&["order_id".to_string()][..]));
    assert_eq!(a.sha256, b.sha256, "same rows, same fingerprint");

    let mut admin = postgres::Client::connect(&format!("host={} port={} user=analyst password=secret-pw dbname=shop", s.host, s.port), postgres::NoTls).unwrap();
    admin.execute("UPDATE orders SET units = units + 1 WHERE order_id = 123456", &[]).unwrap();
    let changed = fetch(&mut c, Read::Table("orders"), &out, &work(), &no()).unwrap();
    admin.execute("UPDATE orders SET units = units - 1 WHERE order_id = 123456", &[]).unwrap();
    assert_ne!(changed.sha256, a.sha256, "one changed value changes the fingerprint");
    // Moved rows (an update rewrites a row elsewhere on disk) still stream in key order.
    let back = fetch(&mut c, Read::Table("orders"), &out, &work(), &no()).unwrap();
    assert_eq!(back.sha256, a.sha256, "back as it was");

    // A table without a key keeps the server's order, which can change.
    admin.batch_execute("DROP TABLE IF EXISTS loose; CREATE TABLE loose AS SELECT * FROM orders LIMIT 1000; GRANT SELECT ON loose TO reader").unwrap();
    let l = fetch(&mut c, Read::Table("loose"), &out, &work(), &no()).unwrap();
    assert_eq!(l.ordered_by, None);
}
