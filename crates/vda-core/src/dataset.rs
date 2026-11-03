//! A step result held on disk: a Parquet file plus the metadata the session
//! needs without opening it (schema, row count, size, content hash).
//!
//! Only bounded previews and chart data are ever read into memory.

use crate::json::Json;
use crate::table::{DataType, Field, Schema};
use crate::Error;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Dataset {
    pub path: PathBuf,
    pub schema: Schema,
    pub rows: u64,
    pub bytes: u64,
    /// SHA-256 of the Parquet file. DuckDB writes byte-identical files for the
    /// same result with `threads = 1`, so this identifies the content for
    /// replay without re-reading every row.
    pub content_hash: String,
}

impl Dataset {
    pub fn meta_json(&self) -> Json {
        Json::obj(vec![
            (
                "schema",
                Json::Arr(
                    self.schema
                        .fields
                        .iter()
                        .map(|f| Json::obj(vec![("name", Json::str(f.name.clone())), ("type", Json::str(f.dtype.name()))]))
                        .collect(),
                ),
            ),
            // Counts as strings: JSON numbers lose precision past 2^53.
            ("rows", Json::str(self.rows.to_string())),
            ("bytes", Json::str(self.bytes.to_string())),
            ("sha256", Json::str(self.content_hash.clone())),
        ])
    }

    pub fn from_meta(path: PathBuf, j: &Json) -> Result<Dataset, Error> {
        let num = |k: &str| -> Result<u64, Error> {
            j.req_str(k)?.parse().map_err(|_| Error::Format(format!("bad `{k}` in dataset metadata")))
        };
        let fields = j
            .req_arr("schema")?
            .iter()
            .map(|f| {
                Ok(Field {
                    name: f.req_str("name")?.to_string(),
                    dtype: DataType::parse(f.req_str("type")?)
                        .ok_or_else(|| Error::Format("unknown type in dataset metadata".into()))?,
                })
            })
            .collect::<Result<_, Error>>()?;
        Ok(Dataset {
            path,
            schema: Schema { fields },
            rows: num("rows")?,
            bytes: num("bytes")?,
            content_hash: j.req_str("sha256")?.to_string(),
        })
    }
}
