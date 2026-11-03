//! Exporting a result (or a source) to a file the user chose: CSV, Parquet
//! or Excel.
//!
//! Export runs on its own DuckDB connection, not the analysis engine's:
//! that one is confined to the project folder (ADR-0004), and an export is
//! an action the user takes, with SQL the app writes and no user SQL in it.
//! Files are written beside the target and renamed into place, so a failed
//! export never leaves half a file under the chosen name.

use crate::dataset::Dataset;
use crate::table::{Table, Value, MICROS_PER_DAY};
use crate::Error;
use duckdb::Connection;
use std::fs;
use std::path::{Path, PathBuf};

/// Excel's limits: 1,048,576 rows including the header, 16,384 columns.
pub const EXCEL_MAX_ROWS: u64 = 1_048_575;
const EXCEL_MAX_COLS: usize = 16_384;
const EXCEL_MAX_TEXT: usize = 32_767;
const CHUNK: u64 = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Parquet,
    Excel,
}

impl ExportFormat {
    /// The format a file name asks for.
    pub fn from_path(path: &Path) -> Option<ExportFormat> {
        match path.extension()?.to_string_lossy().to_ascii_lowercase().as_str() {
            "csv" => Some(ExportFormat::Csv),
            "parquet" | "pq" => Some(ExportFormat::Parquet),
            "xlsx" => Some(ExportFormat::Excel),
            _ => None,
        }
    }
}

fn lit(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

fn conn() -> Result<Connection, Error> {
    let c = Connection::open_in_memory().map_err(|e| Error::Data(format!("cannot start DuckDB for export: {e}")))?;
    // One thread keeps rows in result order.
    c.execute_batch("SET threads = 1; SET preserve_insertion_order = true; SET autoinstall_known_extensions = false;")
        .map_err(|e| Error::Data(format!("cannot configure export: {e}")))?;
    Ok(c)
}

fn temp_beside(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.vda-export"))
}

/// Write `ds` to `path` in `format`. Returns the number of rows written.
pub fn export(ds: &Dataset, path: &Path, format: ExportFormat) -> Result<u64, Error> {
    let tmp = temp_beside(path);
    let written = (|| -> Result<(), Error> {
        match format {
            ExportFormat::Parquet => {
                fs::copy(&ds.path, &tmp)?;
            }
            ExportFormat::Csv => {
                conn()?
                    .execute_batch(&format!(
                        "COPY (SELECT * FROM read_parquet({})) TO {} (FORMAT csv, HEADER true);",
                        lit(&ds.path),
                        lit(&tmp)
                    ))
                    .map_err(|e| Error::Data(format!("cannot write CSV: {e}")))?;
            }
            ExportFormat::Excel => excel(ds, &tmp)?,
        }
        Ok(())
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(ds.rows)
}

fn excel(ds: &Dataset, out: &Path) -> Result<(), Error> {
    use rust_xlsxwriter::{Format, Workbook};
    if ds.rows > EXCEL_MAX_ROWS {
        return Err(Error::Data(format!(
            "this result has {} rows; an Excel sheet holds at most {EXCEL_MAX_ROWS}. Export it as CSV or Parquet, or filter it first",
            ds.rows
        )));
    }
    if ds.schema.fields.len() > EXCEL_MAX_COLS {
        return Err(Error::Data(format!("an Excel sheet holds at most {EXCEL_MAX_COLS} columns")));
    }
    let xl = |e: rust_xlsxwriter::XlsxError| Error::Data(format!("cannot write Excel file: {e}"));
    let mut wb = Workbook::new();
    let sheet = wb.add_worksheet_with_constant_memory().map_err(xl)?;
    sheet.set_name("Result").map_err(xl)?;
    let bold = Format::new().set_bold();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    let stamp = Format::new().set_num_format("yyyy-mm-dd hh:mm:ss");
    for (c, f) in ds.schema.fields.iter().enumerate() {
        sheet.write_string_with_format(0, c as u16, &f.name, &bold).map_err(xl)?;
    }
    sheet.set_freeze_panes(1, 0).map_err(xl)?;
    let conn = conn()?;
    let mut offset = 0;
    while offset < ds.rows {
        let page: Table = crate::duck::query_table(
            &conn,
            &format!("SELECT * FROM read_parquet({}) LIMIT {CHUNK} OFFSET {offset}", lit(&ds.path)),
        )?;
        for (i, row) in page.rows.iter().enumerate() {
            let r = (offset + i as u64 + 1) as u32;
            for (c, v) in row.iter().enumerate() {
                let c = c as u16;
                match v {
                    Value::Null => {}
                    Value::Bool(b) => {
                        sheet.write_boolean(r, c, *b).map_err(xl)?;
                    }
                    // Excel numbers are doubles: integers past 2^53 are kept exact as text.
                    Value::Int(n) if n.unsigned_abs() > (1u64 << 53) => {
                        sheet.write_string(r, c, n.to_string()).map_err(xl)?;
                    }
                    Value::Int(n) => {
                        sheet.write_number(r, c, *n as f64).map_err(xl)?;
                    }
                    Value::Float(x) => {
                        sheet.write_number(r, c, *x).map_err(xl)?;
                    }
                    Value::Text(s) if s.chars().count() > EXCEL_MAX_TEXT => {
                        return Err(Error::Data(format!(
                            "a text value in row {r} is longer than Excel allows ({EXCEL_MAX_TEXT} characters); export as CSV or Parquet"
                        )))
                    }
                    Value::Text(s) => {
                        sheet.write_string(r, c, s).map_err(xl)?;
                    }
                    // Excel serial dates count days from 1899-12-30.
                    Value::Date(d) => {
                        sheet.write_number_with_format(r, c, *d as f64 + 25_569.0, &date).map_err(xl)?;
                    }
                    Value::Timestamp(t) => {
                        let serial = t.div_euclid(MICROS_PER_DAY) as f64
                            + 25_569.0
                            + t.rem_euclid(MICROS_PER_DAY) as f64 / MICROS_PER_DAY as f64;
                        sheet.write_number_with_format(r, c, serial, &stamp).map_err(xl)?;
                    }
                }
            }
        }
        offset += CHUNK;
    }
    wb.save(out).map_err(xl)
}
