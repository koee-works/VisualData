//! Saved step results, addressed by cache key.
//!
//! Layout inside a project folder:
//!
//! ```text
//! artifacts/<cache-key>.parquet   table results
//! artifacts/<cache-key>.vl.json   chart specifications
//! ```
//!
//! A cache key already covers the step definition, its inputs, the source
//! content and the engine/Python versions, so a file with the current key is
//! by construction the current result. Files are written atomically; a file
//! that cannot be read is treated as missing and the step simply runs again.

use crate::duck;
use crate::json::Json;
use crate::session::Output;
use crate::Error;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    dir: PathBuf,
}

const TABLE_EXT: &str = ".parquet";
const CHART_EXT: &str = ".vl.json";

impl ArtifactStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        ArtifactStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, key: &str, chart: bool) -> PathBuf {
        self.dir.join(format!("{key}{}", if chart { CHART_EXT } else { TABLE_EXT }))
    }

    pub fn save(&self, key: &str, output: &Output) -> Result<(), Error> {
        fs::create_dir_all(&self.dir)?;
        let chart = matches!(output, Output::Chart(_));
        let target = self.path(key, chart);
        let tmp = self.dir.join(format!(".{key}.tmp{}", if chart { CHART_EXT } else { TABLE_EXT }));
        match output {
            Output::Table(t) => duck::write_parquet(t, &tmp)?,
            Output::Chart(spec) => fs::write(&tmp, spec.to_string_pretty())?,
        }
        fs::rename(&tmp, &target)?; // atomic replace: readers never see a partial file
        Ok(())
    }

    /// The saved result for `key`, or `None` if absent or unreadable.
    pub fn load(&self, key: &str, chart: bool) -> Option<Output> {
        let path = self.path(key, chart);
        if !path.exists() {
            return None;
        }
        if chart {
            let text = fs::read_to_string(&path).ok()?;
            Json::parse(&text).ok().map(Output::Chart)
        } else {
            duck::read_parquet(&path).ok().map(Output::Table)
        }
    }

    /// Delete saved results whose key is not in `keep`. Returns how many files
    /// were removed.
    pub fn prune(&self, keep: &HashSet<String>) -> Result<usize, Error> {
        if !self.dir.exists() {
            return Ok(0);
        }
        let mut removed = 0;
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            let key = name.strip_suffix(TABLE_EXT).or_else(|| name.strip_suffix(CHART_EXT));
            // Leftover temp files from an interrupted write are removed too.
            let stale = match key {
                Some(k) if !k.starts_with('.') => !keep.contains(k),
                _ => name.starts_with('.') && name.contains(".tmp"),
            };
            if stale {
                fs::remove_file(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}
