//! Peak memory and time for a large query: cargo run -p vda-server --example scale -- <rows>
use std::sync::{atomic::AtomicBool, Arc};
fn main() {
    let n: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(5_000_000);
    let src = vda_server::PgSource { host: "127.0.0.1".into(), port: 54329, database: "shop".into(), user: "reader".into(), timeout: std::time::Duration::from_secs(600) };
    let mut c = vda_server::connect(&src, "reader-pw").unwrap();
    let work = std::env::temp_dir();
    let out = work.join("vda-scale.parquet");
    let q = format!("SELECT i AS id, md5(i::text) AS code, (i % 1000) * 1.5 AS amount, DATE '2026-01-01' + (i % 365) AS day FROM generate_series(1, {n}) i");
    let r = vda_server::fetch(&mut c, vda_server::Read::Query(&q), &out, &work, &Arc::new(AtomicBool::new(false))).unwrap();
    let hwm = std::fs::read_to_string("/proc/self/status").unwrap().lines().find(|l| l.starts_with("VmHWM")).unwrap().to_string();
    println!("{} rows in {:.1?}, {} MB streamed, Parquet {} MB, {hwm}", r.rows, r.elapsed, r.bytes_streamed / 1_000_000, std::fs::metadata(&out).unwrap().len() / 1_000_000);
    let _ = std::fs::remove_file(out);
}
