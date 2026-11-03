//! In-memory tables, values and column profiles.
//!
//! Used by the reference engine, for previews, and as the in-memory form of
//! results. Dates are days since 1970-01-01; timestamps are microseconds since
//! 1970-01-01 00:00:00, without a time zone (matching DuckDB's DATE and
//! TIMESTAMP).

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    Bool,
    Int,
    Float,
    Text,
    Date,
    Timestamp,
}

impl DataType {
    /// The type in words, for messages.
    pub fn plain(self) -> &'static str {
        match self {
            DataType::Bool => "true/false",
            DataType::Int => "whole number",
            DataType::Float => "decimal",
            DataType::Text => "text",
            DataType::Date => "date",
            DataType::Timestamp => "date and time",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DataType::Bool => "BOOLEAN",
            DataType::Int => "BIGINT",
            DataType::Float => "DOUBLE",
            DataType::Text => "VARCHAR",
            DataType::Date => "DATE",
            DataType::Timestamp => "TIMESTAMP",
        }
    }

    pub fn parse(s: &str) -> Option<DataType> {
        Some(match s {
            "BOOLEAN" => DataType::Bool,
            "BIGINT" => DataType::Int,
            "DOUBLE" => DataType::Float,
            "VARCHAR" => DataType::Text,
            "DATE" => DataType::Date,
            "TIMESTAMP" => DataType::Timestamp,
            _ => return None,
        })
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, DataType::Int | DataType::Float)
    }

    pub fn is_temporal(self) -> bool {
        matches!(self, DataType::Date | DataType::Timestamp)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    /// Days since 1970-01-01.
    Date(i32),
    /// Microseconds since 1970-01-01 00:00:00 (no time zone).
    Timestamp(i64),
}

impl Value {
    /// The value's type; `None` for a missing value.
    pub fn dtype(&self) -> Option<DataType> {
        Some(match self {
            Value::Null => return None,
            Value::Bool(_) => DataType::Bool,
            Value::Int(_) => DataType::Int,
            Value::Float(_) => DataType::Float,
            Value::Text(_) => DataType::Text,
            Value::Date(_) => DataType::Date,
            Value::Timestamp(_) => DataType::Timestamp,
        })
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// SQL-style comparison: `None` when either side is NULL or the types are
    /// not comparable. Int and Float compare numerically.
    pub fn sql_cmp(&self, other: &Value) -> Option<Ordering> {
        match (self, other) {
            (Value::Null, _) | (_, Value::Null) => None,
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
            (Value::Text(a), Value::Text(b)) => Some(a.as_bytes().cmp(b.as_bytes())),
            (Value::Date(a), Value::Date(b)) => Some(a.cmp(b)),
            (Value::Timestamp(a), Value::Timestamp(b)) => Some(a.cmp(b)),
            // A date compares as midnight of that day, as in SQL.
            (Value::Date(a), Value::Timestamp(b)) => Some((*a as i64 * MICROS_PER_DAY).cmp(b)),
            (Value::Timestamp(a), Value::Date(b)) => Some(a.cmp(&(*b as i64 * MICROS_PER_DAY))),
            (a, b) => match (a.as_f64(), b.as_f64()) {
                (Some(x), Some(y)) => x.partial_cmp(&y),
                _ => None,
            },
        }
    }

    /// Total order used for sorting result rows: NULLs last, matching
    /// DuckDB's default `ORDER BY ... ASC NULLS LAST`.
    pub fn sort_cmp(&self, other: &Value) -> Ordering {
        match (self.is_null(), other.is_null()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => self.sql_cmp(other).unwrap_or(Ordering::Equal),
        }
    }

    /// Stable textual key used for grouping and hashing.
    pub fn key(&self) -> String {
        match self {
            Value::Null => "\u{0}N".into(),
            Value::Bool(b) => format!("b{b}"),
            Value::Int(i) => format!("i{i}"),
            Value::Float(f) => format!("f{:?}", f),
            Value::Text(s) => format!("t{s}"),
            Value::Date(d) => format!("d{d}"),
            Value::Timestamp(t) => format!("s{t}"),
        }
    }
}

pub const MICROS_PER_DAY: i64 = 86_400_000_000;

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

fn digits(s: &str, n: usize) -> Option<u32> {
    (s.len() == n && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

/// Parse a strict ISO date `YYYY-MM-DD`, rejecting impossible dates.
pub fn parse_date(s: &str) -> Option<i32> {
    let mut parts = s.splitn(3, '-');
    let (y, m, d) = (digits(parts.next()?, 4)?, digits(parts.next()?, 2)?, digits(parts.next()?, 2)?);
    if !(1..=12).contains(&m) || d == 0 {
        return None;
    }
    let days = days_from_civil(y as i64, m, d);
    // Round-trip check rejects days past the end of the month (e.g. 02-30).
    (civil_from_days(days) == (y as i64, m, d)).then_some(days as i32)
}

/// Parse an ISO timestamp `YYYY-MM-DD HH:MM:SS[.ffffff]` (a `T` separator is
/// also accepted). A bare date is not a timestamp.
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let (date, time) = s.split_once([' ', 'T'])?;
    let days = parse_date(date)? as i64;
    let (hms, frac) = match time.split_once('.') {
        Some((h, f)) => (h, Some(f)),
        None => (time, None),
    };
    let mut parts = hms.splitn(3, ':');
    let (h, mi, se) = (digits(parts.next()?, 2)?, digits(parts.next()?, 2)?, digits(parts.next()?, 2)?);
    if h > 23 || mi > 59 || se > 59 {
        return None;
    }
    let micros = match frac {
        None => 0,
        Some(f) if (1..=6).contains(&f.len()) && f.bytes().all(|b| b.is_ascii_digit()) => {
            f.parse::<i64>().ok()? * 10i64.pow(6 - f.len() as u32)
        }
        Some(_) => return None,
    };
    Some(days * MICROS_PER_DAY + ((h as i64 * 60 + mi as i64) * 60 + se as i64) * 1_000_000 + micros)
}

pub fn format_date(days: i32) -> String {
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Formats like DuckDB: fractional seconds only when present, trailing zeros trimmed.
pub fn format_timestamp(micros: i64) -> String {
    let days = micros.div_euclid(MICROS_PER_DAY);
    let rem = micros.rem_euclid(MICROS_PER_DAY);
    let (secs, frac) = (rem / 1_000_000, rem % 1_000_000);
    let mut out = format!(
        "{} {:02}:{:02}:{:02}",
        format_date(days as i32),
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    );
    if frac != 0 {
        out.push_str(format!(".{frac:06}").trim_end_matches('0'));
    }
    out
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "NULL"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => {
                if x.fract() == 0.0 && x.abs() < 1e15 {
                    write!(f, "{x:.1}")
                } else {
                    write!(f, "{x}")
                }
            }
            Value::Text(s) => write!(f, "{s}"),
            Value::Date(d) => write!(f, "{}", format_date(*d)),
            Value::Timestamp(t) => write!(f, "{}", format_timestamp(*t)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub dtype: DataType,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Schema {
    pub fields: Vec<Field>,
}

impl Schema {
    pub fn new(fields: Vec<(&str, DataType)>) -> Schema {
        Schema {
            fields: fields
                .into_iter()
                .map(|(n, t)| Field { name: n.to_string(), dtype: t })
                .collect(),
        }
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name)
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    pub schema: Schema,
    pub rows: Vec<Vec<Value>>,
}

impl Table {
    pub fn column_values(&self, idx: usize) -> impl Iterator<Item = &Value> {
        self.rows.iter().map(move |r| &r[idx])
    }

    /// Deterministic content fingerprint (schema + rows) used to verify replay.
    pub fn content_hash(&self) -> String {
        let mut h = crate::hash::Sha256::new();
        for f in &self.schema.fields {
            h.update(f.name.as_bytes());
            h.update(b"\x1f");
            h.update(f.dtype.name().as_bytes());
            h.update(b"\x1e");
        }
        h.update(b"\x1d");
        for row in &self.rows {
            for v in row {
                h.update(v.key().as_bytes());
                h.update(b"\x1f");
            }
            h.update(b"\x1e");
        }
        h.hex_digest()
    }

    pub fn profile(&self) -> Vec<ColumnProfile> {
        self.schema
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let mut nulls = 0;
                let mut distinct = HashSet::new();
                let mut min: Option<Value> = None;
                let mut max: Option<Value> = None;
                for v in self.column_values(i) {
                    if v.is_null() {
                        nulls += 1;
                        continue;
                    }
                    distinct.insert(v.key());
                    if min.as_ref().is_none_or(|m| v.sort_cmp(m) == Ordering::Less) {
                        min = Some(v.clone());
                    }
                    if max.as_ref().is_none_or(|m| v.sort_cmp(m) == Ordering::Greater) {
                        max = Some(v.clone());
                    }
                }
                ColumnProfile {
                    name: f.name.clone(),
                    dtype: f.dtype,
                    rows: self.rows.len(),
                    nulls,
                    distinct: distinct.len(),
                    min,
                    max,
                }
            })
            .collect()
    }

    /// Markdown preview of this table (typically one page of a larger
    /// result), labelled as partial when `total_rows` is larger.
    pub fn preview(&self, total_rows: u64) -> String {
        let mut out = String::new();
        out.push_str("| ");
        out.push_str(
            &self
                .schema
                .fields
                .iter()
                .map(|f| format!("{} ({})", f.name, f.dtype.name()))
                .collect::<Vec<_>>()
                .join(" | "),
        );
        out.push_str(" |\n|");
        for _ in &self.schema.fields {
            out.push_str("---|");
        }
        out.push('\n');
        for r in &self.rows {
            out.push_str("| ");
            out.push_str(&r.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" | "));
            out.push_str(" |\n");
        }
        if total_rows > self.rows.len() as u64 {
            out.push_str(&format!("\n_Preview: first {} of {total_rows} rows._\n", self.rows.len()));
        } else {
            out.push_str(&format!("\n_{total_rows} rows (complete)._\n"));
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnProfile {
    pub name: String,
    pub dtype: DataType,
    pub rows: usize,
    pub nulls: usize,
    pub distinct: usize,
    pub min: Option<Value>,
    pub max: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_round_trips_and_validation() {
        for s in ["1970-01-01", "2026-07-03", "2000-02-29", "1600-03-01", "0001-01-01", "9999-12-31"] {
            assert_eq!(format_date(parse_date(s).unwrap()), s);
        }
        assert_eq!(parse_date("1970-01-02"), Some(1));
        assert_eq!(parse_date("1969-12-31"), Some(-1));
        for bad in ["2026-02-29", "2026-13-01", "2026-04-31", "26-07-03", "2026-7-3", "2026/07/03", ""] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn timestamp_round_trips_like_duckdb() {
        for s in ["2026-07-03 10:00:00", "2026-07-03 10:00:00.5", "2026-07-03 10:00:00.123456", "1969-12-31 23:59:59.999999"] {
            assert_eq!(format_timestamp(parse_timestamp(s).unwrap()), s);
        }
        assert_eq!(parse_timestamp("2026-07-03T10:00:00"), parse_timestamp("2026-07-03 10:00:00"));
        for bad in ["2026-07-03", "2026-07-03 24:00:00", "2026-07-03 10:00", "2026-07-03 10:00:00.1234567"] {
            assert_eq!(parse_timestamp(bad), None, "{bad}");
        }
    }

    #[test]
    fn dates_compare_with_timestamps_as_midnight() {
        let d = Value::Date(parse_date("2026-07-03").unwrap());
        let midnight = Value::Timestamp(parse_timestamp("2026-07-03 00:00:00").unwrap());
        let later = Value::Timestamp(parse_timestamp("2026-07-03 00:00:01").unwrap());
        assert_eq!(d.sql_cmp(&midnight), Some(Ordering::Equal));
        assert_eq!(d.sql_cmp(&later), Some(Ordering::Less));
    }
}
