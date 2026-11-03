//! Sources other than CSV: Excel and OpenDocument sheets, Parquet, JSON.

use std::fs;
use std::path::{Path, PathBuf};
use vda_core::duck::DuckDbEngine;
use vda_core::engine::{Engine, ReferenceEngine};
use vda_core::model::*;
use vda_core::session::{Session, StepState};
use vda_core::table::{DataType, DataType::*, Value};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vda-formats-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn import(format: SourceFormat, file: &str, sheet: Option<&str>) -> vda_core::table::Table {
    let out = scratch(&format!("import-{file}-{}", sheet.unwrap_or("first"))).join("out.parquet");
    let mut duck = DuckDbEngine::new().unwrap();
    let ds = duck.import_source(&fixture(file), format, sheet, &out).unwrap_or_else(|e| panic!("{file}: {e}"));
    vda_core::duck::read_parquet(&ds.path).unwrap()
}

fn types(t: &vda_core::table::Table) -> Vec<DataType> {
    t.schema.fields.iter().map(|f| f.dtype).collect()
}

fn names(t: &vda_core::table::Table) -> Vec<String> {
    t.schema.fields.iter().map(|f| f.name.clone()).collect()
}

#[test]
fn excel_sheets_are_typed_like_csv() {
    let t = import(SourceFormat::Excel, "regions.xlsx", Some("Targets"));
    assert_eq!(names(&t), ["region", "manager", "target", "share", "since", "updated", "active", "region_2", "column_9"]);
    assert_eq!(types(&t), [Text, Text, Int, Float, Date, Timestamp, Bool, Text, Text]);
    assert_eq!(t.rows.len(), 5, "the blank row is skipped");
    let west = t.rows.iter().find(|r| r[0] == Value::Text("West".into())).unwrap();
    assert_eq!(west[2], Value::Null, "a formula without a value is missing");
    assert_eq!(t.rows[0][5].to_string(), "2026-07-01 09:30:00");

    // The first sheet by default; an unknown sheet names the ones there are.
    let notes = import(SourceFormat::Excel, "regions.xlsx", None);
    assert_eq!(notes.rows.len(), 0);
    let out = scratch("bad-sheet").join("out.parquet");
    let err = DuckDbEngine::new()
        .unwrap()
        .import_source(&fixture("regions.xlsx"), SourceFormat::Excel, Some("Q3"), &out)
        .unwrap_err()
        .to_string();
    assert!(err.contains("no sheet named `Q3` (sheets: Notes, Targets)"), "{err}");
    assert_eq!(vda_core::excel::sheet_names(&fixture("regions.xlsx")).unwrap(), ["Notes", "Targets"]);
}

#[test]
fn parquet_types_are_normalized() {
    let t = import(SourceFormat::Parquet, "targets.parquet", None);
    assert_eq!(types(&t), [Text, Int, Float, Date], "32-bit types widen to the core's");
    assert_eq!(t.rows.len(), 5);
    assert_eq!(t.rows[2][2], Value::Null);
}

#[test]
fn json_lines_and_arrays_are_read_with_nested_values_as_text() {
    let t = import(SourceFormat::Json, "events.jsonl", None);
    assert_eq!(names(&t), ["id", "region", "kind", "at", "detail"]);
    assert_eq!(types(&t), [Int, Text, Text, Timestamp, Text]);
    assert!(t.rows[2][4].to_string().contains("Tom"), "{:?}", t.rows[2][4]);
    let a = import(SourceFormat::Json, "events_array.json", None);
    assert_eq!(types(&a), [Int, Text, Bool]);
    assert_eq!(a.rows[1][1], Value::Null);
}

#[test]
fn the_reference_engine_reads_the_same_files() {
    for (format, file, sheet) in
        [(SourceFormat::Excel, "regions.xlsx", Some("Targets")), (SourceFormat::Parquet, "targets.parquet", None)]
    {
        let out = scratch(&format!("ref-{file}")).join("out.parquet");
        let ds = ReferenceEngine::new().import_source(&fixture(file), format, sheet, &out).unwrap();
        assert_eq!(vda_core::duck::read_parquet(&ds.path).unwrap(), import(format, file, sheet), "{file}");
    }
}

fn project() -> (PathBuf, Analysis) {
    let dir = scratch("project");
    for f in ["regions.xlsx", "targets.parquet", "events.jsonl"] {
        fs::copy(fixture(f), dir.join(f)).unwrap();
    }
    let sheet = |mut s: Source, name: &str| {
        s.sheet = Some(name.into());
        s
    };
    let a = Analysis {
        parameters: vec![],
        name: "formats".into(),
        sources: vec![
            sheet(Source::new("sheet", "regions.xlsx"), "Targets"),
            Source::new("targets", "targets.parquet"),
            Source::new("events", "events.jsonl"),
        ],
        steps: vec![
            Step {
                id: "joined".into(),
                inputs: vec!["sheet".into(), "targets".into()],
                label: None,
                kind: StepKind::Join { how: JoinKind::Left, on: vec![JoinKey::equal("region", "region")] },
            },
            Step {
                id: "visits".into(),
                inputs: vec!["events".into(), "joined".into()],
                label: None,
                kind: StepKind::Sql {
                    sql: "SELECT e.region, j.manager, count(*) AS n FROM input AS e JOIN joined AS j USING (region) \
                          GROUP BY ALL ORDER BY e.region"
                        .into(),
                },
            },
        ],
        cells: vec![],
    };
    (dir, a)
}

#[test]
fn analyses_mix_formats_and_a_sheet_change_outdates_readers() {
    let (dir, a) = project();
    assert_eq!(a.sources.iter().map(|s| s.format).collect::<Vec<_>>(), [SourceFormat::Excel, SourceFormat::Parquet, SourceFormat::Json]);
    vda_core::persist::save_analysis(&a, &dir).unwrap();
    let text = fs::read_to_string(dir.join("analysis.json")).unwrap();
    assert!(text.contains("\"format\": \"excel\"") && text.contains("\"sheet\": \"Targets\""), "{text}");
    assert_eq!(vda_core::persist::load_analysis(&dir).unwrap(), a);

    let mut s = Session::open(&dir, Box::new(DuckDbEngine::new().unwrap()), None).unwrap();
    let run = s.run_all().unwrap();
    assert!(run.succeeded(), "{run:?}");
    let joined = s.table("joined").unwrap();
    assert!(names(&joined).contains(&"ratio".to_string()), "Parquet columns joined to the sheet");
    let visits = s.table("visits").unwrap();
    assert_eq!(visits.rows.len(), 3);

    // The inspectable SQL reads Parquet and JSON directly; the sheet is named.
    let mut s2 = Session::open(&dir, Box::new(DuckDbEngine::new().unwrap()), None).unwrap();
    let sql = s2.pipeline_sql("visits").unwrap();
    assert!(sql.contains("read_parquet(") && sql.contains("read_json(") && sql.contains("sheet Targets of regions.xlsx"), "{sql}");

    // Choosing another sheet is a different source: its readers are outdated.
    s.analysis.sources[0].sheet = Some("Notes".into());
    assert_eq!(s.state("joined").unwrap(), StepState::Stale);
}
