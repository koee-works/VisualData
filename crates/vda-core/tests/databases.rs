//! SQLite and DuckDB database sources: tables and queries, types, the
//! sandbox around queries, and outdating when the database changes.

use std::fs;
use std::path::{Path, PathBuf};
use vda_core::database::{self, Read};
use vda_core::duck::DuckDbEngine;
use vda_core::engine::{Engine, ReferenceEngine};
use vda_core::model::*;
use vda_core::session::{Session, StepState};
use vda_core::table::{DataType, Value};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vda-db-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn sqlite(path: &Path) {
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch(
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, region TEXT, amount REAL, paid BOOLEAN, day DATE, at DATETIME, code TEXT);
         INSERT INTO orders VALUES (1, 'East', 10.5, 1, '2026-07-01', '2026-07-01 10:00:00', '007');
         INSERT INTO orders VALUES (2, 'West', 4, 0, '2026-07-02', '2026-07-02 11:30:00', 'A1');
         INSERT INTO orders VALUES (3, NULL, NULL, NULL, NULL, NULL, NULL);
         CREATE TABLE regions (region TEXT, manager TEXT);
         INSERT INTO regions VALUES ('East', 'Priya'), ('West', 'Diego');
         CREATE VIEW big AS SELECT * FROM orders WHERE amount > 5;",
    )
    .unwrap();
}

fn duckfile(path: &Path) {
    let c = duckdb::Connection::open(path).unwrap();
    c.execute_batch(
        "CREATE TABLE orders AS SELECT * FROM (VALUES (1, 'East', 10.5::DOUBLE, DATE '2026-07-01'), (2, 'West', 4.0, DATE '2026-07-02'), (3, NULL, NULL, NULL)) v(id, region, amount, day);
         CREATE TABLE regions AS SELECT * FROM (VALUES ('East', 'Priya'), ('West', 'Diego')) v(region, manager);",
    )
    .unwrap();
}

fn read(path: &Path, f: SourceFormat, what: Read) -> Result<vda_core::table::Table, String> {
    let out = path.with_extension(format!("{}.parquet", std::process::id()));
    database::copy_to_parquet(path, f, what, &out).map_err(|e| e.to_string())?;
    let t = vda_core::duck::read_parquet(&out).unwrap();
    let _ = fs::remove_file(&out);
    Ok(t)
}

#[test]
fn tables_queries_and_types() {
    let d = dir("read");
    let (s, k) = (d.join("shop.sqlite"), d.join("shop.duckdb"));
    sqlite(&s);
    duckfile(&k);
    assert_eq!(database::sniff(&s), Some(SourceFormat::Sqlite));
    assert_eq!(database::sniff(&k), Some(SourceFormat::DuckDb));
    assert_eq!(database::tables(&s, SourceFormat::Sqlite).unwrap(), ["big", "orders", "regions"]);
    assert_eq!(database::tables(&k, SourceFormat::DuckDb).unwrap(), ["orders", "regions"]);

    let t = read(&s, SourceFormat::Sqlite, Read::Table(Some("orders"))).unwrap();
    let types: Vec<DataType> = t.schema.fields.iter().map(|f| f.dtype).collect();
    use DataType::*;
    // Declared types and values decide: codes stay text, dates become dates.
    assert_eq!(types, [Int, Text, Float, Bool, Date, Timestamp, Text]);
    assert_eq!(t.rows[0][6], Value::Text("007".into()));
    assert_eq!(t.rows[1][2], Value::Float(4.0));
    assert!(t.rows[2].iter().skip(1).all(Value::is_null));

    let q = read(&s, SourceFormat::Sqlite, Read::Query("SELECT region, sum(amount) AS total FROM orders GROUP BY region ORDER BY region")).unwrap();
    assert_eq!(q.rows.len(), 3);
    let first = read(&k, SourceFormat::DuckDb, Read::Table(None)).unwrap();
    assert_eq!(first.schema.fields[0].name, "id", "the first table by name");
    let q = read(&k, SourceFormat::DuckDb, Read::Query("SELECT o.id, r.manager FROM orders o JOIN regions r USING (region) ORDER BY id")).unwrap();
    assert_eq!(q.rows, vec![vec![Value::Int(1), Value::Text("Priya".into())], vec![Value::Int(2), Value::Text("Diego".into())]]);
}

#[test]
fn queries_can_only_read_this_database() {
    let d = dir("sandbox");
    let (s, k) = (d.join("shop.sqlite"), d.join("shop.duckdb"));
    sqlite(&s);
    duckfile(&k);
    fs::write(d.join("secret.csv"), "x\n1\n").unwrap();
    let secret = d.join("secret.csv").to_string_lossy().into_owned();
    let other = d.join("other.sqlite").to_string_lossy().into_owned();
    for (f, path, q, want) in [
        (SourceFormat::Sqlite, &s, "DELETE FROM orders".to_string(), "must only read"),
        (SourceFormat::Sqlite, &s, format!("ATTACH '{other}' AS o"), "cannot attach others"),
        (SourceFormat::DuckDb, &k, format!("SELECT * FROM read_csv('{secret}')"), "does not run"),
        (SourceFormat::DuckDb, &k, "DROP TABLE orders".to_string(), "the query"),
        (SourceFormat::DuckDb, &k, format!("COPY orders TO '{secret}'"), "the query"),
        (SourceFormat::Sqlite, &s, "SELECT nope FROM orders".to_string(), "does not run"),
        (SourceFormat::Sqlite, &s, "SELECT 1 AS a, 2 AS a".to_string(), "two columns named `a`"),
    ] {
        let e = read(path, f, Read::Query(&q)).unwrap_err();
        assert!(e.contains(want), "{q}: {e}");
    }
    assert_eq!(fs::read_to_string(d.join("secret.csv")).unwrap(), "x\n1\n");
    assert_eq!(read(&s, SourceFormat::Sqlite, Read::Table(Some("orders"))).unwrap().rows.len(), 3, "nothing was deleted");
    let e = read(&s, SourceFormat::Sqlite, Read::Table(Some("nope"))).unwrap_err();
    assert!(e.contains("has no table `nope` (tables: big, orders, regions)"), "{e}");
}

fn session(d: &Path, engine: Box<dyn Engine>) -> Session {
    let mut sales = Source::new("sales", "shop.sqlite");
    sales.sheet = Some("orders".into());
    let mut managers = Source::new("managers", "shop.duckdb");
    managers.query = Some("SELECT region, manager FROM regions".into());
    let a = Analysis {
        name: "db".into(),
        sources: vec![sales, managers],
        steps: vec![
            Step { id: "joined".into(), inputs: vec!["sales".into(), "managers".into()], label: None, kind: StepKind::Join { how: JoinKind::Left, on: vec![JoinKey::equal("region", "region")] } },
            Step {
                id: "by_region".into(),
                inputs: vec!["sales".into()],
                label: None,
                kind: StepKind::Aggregate { group_by: vec!["region".into()], measures: vec![Measure { func: AggFunc::Sum, column: Some("amount".into()), alias: "total".into() }] },
            },
        ],
        cells: vec![],
        parameters: vec![],
    };
    Session::new(a, d, engine, None).unwrap()
}

#[test]
fn database_sources_run_in_both_engines_and_outdate_when_the_file_changes() {
    for (name, engine) in [("duck", Box::new(DuckDbEngine::new().unwrap()) as Box<dyn Engine>), ("ref", Box::new(ReferenceEngine::new()))] {
        let d = dir(&format!("session-{name}"));
        sqlite(&d.join("shop.sqlite"));
        duckfile(&d.join("shop.duckdb"));
        let mut s = session(&d, engine);
        assert!(s.run_all().unwrap().succeeded());
        let t = s.preview("joined", 0, 10).unwrap();
        assert_eq!(t.rows[0].last(), Some(&Value::Text("Priya".into())), "{name}");
        // The inspectable SQL says the data was copied out of the database.
        assert!(s.pipeline_sql("by_region").unwrap().contains("table orders of sqlite shop.sqlite, copied on import"));

        // A new row in the SQLite file outdates what reads it, not the rest.
        let c = rusqlite::Connection::open(d.join("shop.sqlite")).unwrap();
        c.execute("INSERT INTO orders (id, region, amount) VALUES (4, 'East', 1)", []).unwrap();
        drop(c);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(s.state("by_region").unwrap(), StepState::Stale, "{name}");
        assert!(s.run_all().unwrap().succeeded());
        assert_eq!(s.preview("by_region", 0, 1).unwrap().rows[0][1], Value::Float(11.5), "{name}");
    }
}

#[test]
fn changing_the_query_reads_the_database_again() {
    let d = dir("requery");
    sqlite(&d.join("shop.sqlite"));
    duckfile(&d.join("shop.duckdb"));
    let mut s = session(&d, Box::new(DuckDbEngine::new().unwrap()));
    assert_eq!(s.source_dataset("sales").unwrap().rows, 3);
    s.analysis.sources[0].query = Some("SELECT * FROM orders WHERE amount > 5".into());
    assert_eq!(s.source_dataset("sales").unwrap().rows, 1, "not the copy of the whole table");
    assert_eq!(s.state("by_region").unwrap(), StepState::NotRun);
}
