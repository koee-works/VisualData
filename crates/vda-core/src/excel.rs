//! Excel and OpenDocument spreadsheets (xlsx, xlsm, xlsb, xls, ods) as
//! sources, read with calamine.
//!
//! One sheet becomes one table: the first row holds column names, later
//! rows are data. Column types are inferred like CSV import: integers,
//! decimals, true/false, dates and timestamps where every filled cell agrees,
//! text otherwise. Error cells (`#DIV/0!`, `#N/A`) and empty cells are
//! missing values. Fully empty rows are skipped.

use crate::table::{
    days_from_civil, parse_date, parse_timestamp, DataType, Field, Schema, Table, Value,
    MICROS_PER_DAY,
};
use crate::Error;
use calamine::{open_workbook_auto, Data, Reader};
use std::path::Path;

/// File extensions read as spreadsheets.
pub const EXTENSIONS: [&str; 5] = ["xlsx", "xlsm", "xlsb", "xls", "ods"];

fn open(path: &Path) -> Result<calamine::Sheets<std::io::BufReader<std::fs::File>>, Error> {
    open_workbook_auto(path)
        .map_err(|e| Error::Data(format!("cannot read spreadsheet {}: {e}", path.display())))
}

/// Sheet names in workbook order.
pub fn sheet_names(path: &Path) -> Result<Vec<String>, Error> {
    Ok(open(path)?.sheet_names())
}

/// A cell as a core value, before the column's type is settled.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Empty,
    Bool(bool),
    Num(f64),
    Int(i64),
    Date(i32),
    Timestamp(i64),
    Text(String),
}

fn cell(d: &Data) -> Cell {
    match d {
        Data::Empty | Data::Error(_) => Cell::Empty,
        Data::Bool(b) => Cell::Bool(*b),
        Data::Int(i) => Cell::Int(*i),
        Data::Float(f) => Cell::Num(*f),
        Data::String(s) if s.trim().is_empty() => Cell::Empty,
        Data::String(s) => Cell::Text(s.clone()),
        Data::DateTime(dt) if dt.is_datetime() => {
            let (y, mo, da, h, mi, s, ms) = dt.to_ymd_hms_milli();
            let day = days_from_civil(y as i64, mo as u32, da as u32);
            let micros = day * MICROS_PER_DAY
                + ((h as i64 * 60 + mi as i64) * 60 + s as i64) * 1_000_000
                + ms as i64 * 1000;
            if micros % MICROS_PER_DAY == 0 {
                Cell::Date(day as i32)
            } else {
                Cell::Timestamp(micros)
            }
        }
        // A duration ([h]:mm:ss) is kept as its number of days.
        Data::DateTime(dt) => Cell::Num(dt.as_f64()),
        Data::DateTimeIso(s) => match (parse_date(s), parse_timestamp(s)) {
            (Some(d), _) => Cell::Date(d),
            (None, Some(t)) => Cell::Timestamp(t),
            _ => Cell::Text(s.clone()),
        },
        Data::DurationIso(s) => Cell::Text(s.clone()),
    }
}

fn text_of(c: &Cell) -> Value {
    match c {
        Cell::Empty => Value::Null,
        Cell::Bool(b) => Value::Text(b.to_string()),
        Cell::Int(i) => Value::Text(i.to_string()),
        Cell::Num(f) if f.fract() == 0.0 && f.abs() < 1e15 => Value::Text(format!("{}", *f as i64)),
        Cell::Num(f) => Value::Text(f.to_string()),
        Cell::Date(d) => Value::Text(Value::Date(*d).to_string()),
        Cell::Timestamp(t) => Value::Text(Value::Timestamp(*t).to_string()),
        Cell::Text(s) => Value::Text(s.clone()),
    }
}

fn column_type(cells: &[&Cell]) -> DataType {
    let filled: Vec<&&Cell> = cells.iter().filter(|c| ***c != Cell::Empty).collect();
    if filled.is_empty() {
        return DataType::Text;
    }
    let all = |f: fn(&Cell) -> bool| filled.iter().all(|c| f(c));
    if all(|c| matches!(c, Cell::Bool(_))) {
        DataType::Bool
    } else if all(|c| {
        matches!(c, Cell::Int(_))
            || matches!(c, Cell::Num(f) if f.fract() == 0.0 && f.abs() < 9.0e15)
    }) {
        DataType::Int
    } else if all(|c| matches!(c, Cell::Int(_) | Cell::Num(_))) {
        DataType::Float
    } else if all(|c| matches!(c, Cell::Date(_))) {
        DataType::Date
    } else if all(|c| matches!(c, Cell::Date(_) | Cell::Timestamp(_))) {
        DataType::Timestamp
    } else {
        DataType::Text
    }
}

fn value(c: &Cell, t: DataType) -> Value {
    match (t, c) {
        (_, Cell::Empty) => Value::Null,
        (DataType::Bool, Cell::Bool(b)) => Value::Bool(*b),
        (DataType::Int, Cell::Int(i)) => Value::Int(*i),
        (DataType::Int, Cell::Num(f)) => Value::Int(*f as i64),
        (DataType::Float, Cell::Int(i)) => Value::Float(*i as f64),
        (DataType::Float, Cell::Num(f)) => Value::Float(*f),
        (DataType::Date, Cell::Date(d)) => Value::Date(*d),
        (DataType::Timestamp, Cell::Date(d)) => Value::Timestamp(*d as i64 * MICROS_PER_DAY),
        (DataType::Timestamp, Cell::Timestamp(t)) => Value::Timestamp(*t),
        (_, c) => text_of(c),
    }
}

/// Read one sheet (the first when `sheet` is `None`) as a table.
pub fn read_sheet(path: &Path, sheet: Option<&str>) -> Result<Table, Error> {
    let mut wb = open(path)?;
    let names = wb.sheet_names();
    let name = match sheet {
        Some(s) if names.iter().any(|n| n == s) => s.to_string(),
        Some(s) => {
            return Err(Error::Data(format!(
                "{} has no sheet named `{s}` (sheets: {})",
                path.display(),
                names.join(", ")
            )))
        }
        None => names
            .first()
            .cloned()
            .ok_or_else(|| Error::Data(format!("{} has no sheets", path.display())))?,
    };
    let range = wb.worksheet_range(&name).map_err(|e| {
        Error::Data(format!(
            "cannot read sheet `{name}` of {}: {e}",
            path.display()
        ))
    })?;
    let mut rows = range
        .rows()
        .map(|r| r.iter().map(cell).collect::<Vec<Cell>>())
        .filter(|r| r.iter().any(|c| *c != Cell::Empty));
    let header = rows
        .next()
        .ok_or_else(|| Error::Data(format!("sheet `{name}` is empty")))?;
    let data: Vec<Vec<Cell>> = rows.collect();

    let mut names_out: Vec<String> = Vec::new();
    for (i, h) in header.iter().enumerate() {
        let base = match text_of(h) {
            Value::Text(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => format!("column_{}", i + 1),
        };
        let mut n = base.clone();
        let mut k = 2;
        while names_out.contains(&n) {
            n = format!("{base}_{k}");
            k += 1;
        }
        names_out.push(n);
    }
    let width = names_out.len();
    let empty = Cell::Empty;
    let types: Vec<DataType> = (0..width)
        .map(|c| {
            column_type(
                &data
                    .iter()
                    .map(|r| r.get(c).unwrap_or(&empty))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let rows = data
        .iter()
        .map(|r| {
            (0..width)
                .map(|c| value(r.get(c).unwrap_or(&empty), types[c]))
                .collect()
        })
        .collect();
    Ok(Table {
        schema: Schema {
            fields: names_out
                .into_iter()
                .zip(types)
                .map(|(name, dtype)| Field { name, dtype })
                .collect(),
        },
        rows,
    })
}
