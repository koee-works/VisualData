//! Minimal RFC 4180 CSV reading (with type inference) and writing.
//!
//! TEMPORARY in two roles:
//! - Source import for the reference engine. The DuckDB engine will use
//!   `read_csv_auto`, whose inference rules are richer (dates, timestamps).
//! - Python interchange. CSV loses type information across the boundary;
//!   it is replaced by Parquet once pyarrow is available (see STATUS.md).

use crate::table::{DataType, Field, Schema, Table, Value};
use crate::Error;

pub fn parse_records(text: &str) -> Result<Vec<Vec<String>>, Error> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut line = 1;
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => in_quotes = false,
                '\n' => {
                    line += 1;
                    field.push(c)
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => in_quotes = true,
            ',' => record.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                line += 1;
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                any = false;
            }
            _ => field.push(c),
        }
    }
    if in_quotes {
        return Err(Error::Data(format!("CSV: unterminated quoted field near line {line}")));
    }
    if any {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

fn infer(values: &[&str]) -> DataType {
    let present: Vec<&&str> = values.iter().filter(|v| !v.is_empty()).collect();
    if present.is_empty() {
        return DataType::Text;
    }
    if present.iter().all(|v| v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("false")) {
        return DataType::Bool;
    }
    if present.iter().all(|v| v.parse::<i64>().is_ok()) {
        return DataType::Int;
    }
    if present.iter().all(|v| v.parse::<f64>().map(|f| f.is_finite()).unwrap_or(false)) {
        return DataType::Float;
    }
    DataType::Text
}

fn convert(raw: &str, t: DataType) -> Value {
    if raw.is_empty() {
        return Value::Null;
    }
    match t {
        DataType::Bool => Value::Bool(raw.eq_ignore_ascii_case("true")),
        DataType::Int => Value::Int(raw.parse().unwrap()),
        DataType::Float => Value::Float(raw.parse().unwrap()),
        DataType::Text => Value::Text(raw.to_string()),
    }
}

/// Parse CSV text with a header row, inferring one type per column.
pub fn read_table(text: &str) -> Result<Table, Error> {
    let mut records = parse_records(text)?;
    if records.is_empty() {
        return Err(Error::Data("CSV: no header row".into()));
    }
    let header = records.remove(0);
    for (i, r) in records.iter().enumerate() {
        if r.len() != header.len() {
            return Err(Error::Data(format!(
                "CSV: row {} has {} fields, header has {}",
                i + 2,
                r.len(),
                header.len()
            )));
        }
    }
    let mut fields = Vec::new();
    for (i, name) in header.iter().enumerate() {
        let col: Vec<&str> = records.iter().map(|r| r[i].as_str()).collect();
        fields.push(Field { name: name.trim().to_string(), dtype: infer(&col) });
    }
    let rows = records
        .iter()
        .map(|r| r.iter().zip(&fields).map(|(raw, f)| convert(raw, f.dtype)).collect())
        .collect();
    Ok(Table { schema: Schema { fields }, rows })
}

fn escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn write_table(t: &Table) -> String {
    let mut out = t.schema.fields.iter().map(|f| escape(&f.name)).collect::<Vec<_>>().join(",");
    out.push('\n');
    for row in &t.rows {
        let cells: Vec<String> = row
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                Value::Float(f) => format!("{f:?}"),
                other => escape(&other.to_string()),
            })
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_types_and_nulls() {
        let t = read_table("id,name,score,ok\n1,a,1.5,true\n2,\"b, c\",,false\n3,,2,TRUE\n").unwrap();
        let types: Vec<DataType> = t.schema.fields.iter().map(|f| f.dtype).collect();
        assert_eq!(types, vec![DataType::Int, DataType::Text, DataType::Float, DataType::Bool]);
        assert_eq!(t.rows[1][1], Value::Text("b, c".into()));
        assert_eq!(t.rows[1][2], Value::Null);
        assert_eq!(t.rows[2][1], Value::Null);
        assert_eq!(t.rows[2][2], Value::Float(2.0));
    }

    #[test]
    fn round_trips() {
        let src = "a,b\n1,\"x \"\"q\"\"\"\n,y\n";
        let t = read_table(src).unwrap();
        assert_eq!(read_table(&write_table(&t)).unwrap(), t);
    }

    #[test]
    fn rejects_ragged_rows() {
        assert!(read_table("a,b\n1\n").is_err());
    }
}
