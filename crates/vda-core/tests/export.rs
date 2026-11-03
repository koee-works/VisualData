//! Exported files read back to the same table.

use std::path::{Path, PathBuf};
use vda_core::duck::DuckDbEngine;
use vda_core::engine::Engine;
use vda_core::export::{export, ExportFormat};
use vda_core::model::SourceFormat;
use vda_core::table::Value;

const DATA: &str = "\
id,name,amount,when,day,ok
1,Ada,2.5,2026-07-01 09:30:00,2026-07-01,true
2,\"Bo, Jr.\",,2026-07-02 00:00:00,,false
3,Cy \"C\",-1e-3,,1969-12-31,
";

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vda-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn dataset(d: &Path) -> (DuckDbEngine, vda_core::dataset::Dataset) {
    let src = d.join("data.csv");
    std::fs::write(&src, DATA).unwrap();
    let mut e = DuckDbEngine::new().unwrap();
    let ds = e.import_csv(&src, &d.join("data.parquet")).unwrap();
    (e, ds)
}

#[test]
fn csv_parquet_and_excel_round_trip() {
    let d = dir("round");
    let (mut e, ds) = dataset(&d);
    let original = vda_core::duck::read_parquet(&ds.path).unwrap();
    for (file, fmt, src) in [
        ("out.csv", ExportFormat::Csv, SourceFormat::Csv),
        ("out.parquet", ExportFormat::Parquet, SourceFormat::Parquet),
        ("out.xlsx", ExportFormat::Excel, SourceFormat::Excel),
    ] {
        let path = d.join(file);
        assert_eq!(ExportFormat::from_path(&path), Some(fmt));
        assert_eq!(export(&ds, &path, fmt).unwrap(), 3);
        let back = e.import_source(&path, src, None, &d.join(format!("{file}.back.parquet"))).unwrap();
        let back = vda_core::duck::read_parquet(&back.path).unwrap();
        assert_eq!(back.schema, original.schema, "{file}");
        for (a, b) in back.rows.iter().zip(&original.rows) {
            for (x, y) in a.iter().zip(b) {
                let same = match (x, y) {
                    (Value::Float(p), Value::Float(q)) => (p - q).abs() < 1e-9,
                    _ => x == y,
                };
                assert!(same, "{file}: {x:?} vs {y:?}");
            }
        }
        assert!(!d.join(format!(".{file}.vda-export")).exists(), "no temporary file is left");
    }
}

#[test]
fn excel_limits_are_explained() {
    let d = dir("limits");
    let (_, mut ds) = dataset(&d);
    ds.rows = vda_core::export::EXCEL_MAX_ROWS + 1;
    let err = export(&ds, &d.join("big.xlsx"), ExportFormat::Excel).unwrap_err().to_string();
    assert!(err.contains("Export it as CSV or Parquet"), "{err}");
    assert!(!d.join("big.xlsx").exists());
    assert_eq!(ExportFormat::from_path(Path::new("x.txt")), None);
}
