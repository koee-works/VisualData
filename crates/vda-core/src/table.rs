//! In-memory tables, values and column profiles.
//!
//! Used by the reference engine and for previews. The DuckDB engine will hold
//! full results as Parquet artifacts and only materialize bounded previews here.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    Bool,
    Int,
    Float,
    Text,
}

impl DataType {
    pub fn name(self) -> &'static str {
        match self {
            DataType::Bool => "BOOLEAN",
            DataType::Int => "BIGINT",
            DataType::Float => "DOUBLE",
            DataType::Text => "VARCHAR",
        }
    }

    pub fn parse(s: &str) -> Option<DataType> {
        Some(match s {
            "BOOLEAN" => DataType::Bool,
            "BIGINT" => DataType::Int,
            "DOUBLE" => DataType::Float,
            "VARCHAR" => DataType::Text,
            _ => return None,
        })
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, DataType::Int | DataType::Float)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl Value {
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
        }
    }
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

    /// Plain-text preview, bounded to `limit` rows, labelled when truncated.
    pub fn preview(&self, limit: usize) -> String {
        let shown: Vec<&Vec<Value>> = self.rows.iter().take(limit).collect();
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
        for r in &shown {
            out.push_str("| ");
            out.push_str(&r.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" | "));
            out.push_str(" |\n");
        }
        if self.rows.len() > limit {
            out.push_str(&format!(
                "\n_Preview: first {limit} of {} rows._\n",
                self.rows.len()
            ));
        } else {
            out.push_str(&format!("\n_{} rows (complete)._\n", self.rows.len()));
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
