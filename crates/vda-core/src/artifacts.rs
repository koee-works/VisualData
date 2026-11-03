//! Saved step results, addressed by cache key.
//!
//! Layout inside a project folder:
//!
//! ```text
//! artifacts/<cache-key>.parquet     table result
//! artifacts/<cache-key>.meta.json   its schema, row count, size and hash
//! artifacts/<cache-key>.vl.json     chart specification
//! ```
//!
//! A cache key already covers the step definition, its inputs, the source
//! content and the engine/Python versions, so a file with the current key is
//! by construction the current result. Results are written to a temporary
//! name and renamed into place, metadata last; a result whose metadata is
//! missing or does not match the file is treated as absent and recomputed.

use crate::dataset::Dataset;
use crate::json::Json;
use crate::session::Output;
use crate::Error;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const TABLE_EXT: &str = ".parquet";
const META_EXT: &str = ".meta.json";
const CHART_EXT: &str = ".vl.json";

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct ArtifactStore {
    dir: PathBuf,
    /// Deleted when dropped (clean replay runs use a throwaway store).
    ephemeral: bool,
}

impl ArtifactStore {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, Error> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        // Absolute, because Python steps run with their own working directory.
        Ok(ArtifactStore { dir: fs::canonicalize(&dir)?, ephemeral: false })
    }

    /// An empty store in the system temp directory, removed on drop.
    pub fn ephemeral() -> Result<Self, Error> {
        let n = TMP_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("vda-clean-run-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut store = Self::new(dir)?;
        store.ephemeral = true;
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn table_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}{TABLE_EXT}"))
    }

    fn meta_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}{META_EXT}"))
    }

    fn chart_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}{CHART_EXT}"))
    }

    /// A unique temporary path in the store (same filesystem, so the final
    /// rename is atomic). Names start with `.` so they are never mistaken for results.
    pub fn temp_path(&self, key: &str) -> PathBuf {
        let n = TMP_SEQ.fetch_add(1, Ordering::SeqCst);
        self.dir.join(format!(".{key}.{}-{n}.tmp{TABLE_EXT}", std::process::id()))
    }

    /// Move a dataset written at a temporary path into place under `key`.
    pub fn commit_table(&self, key: &str, mut ds: Dataset) -> Result<Dataset, Error> {
        let target = self.table_path(key);
        fs::rename(&ds.path, &target)?;
        ds.path = target;
        let tmp_meta = self.dir.join(format!(".{key}{META_EXT}.tmp"));
        fs::write(&tmp_meta, ds.meta_json().to_string_pretty())?;
        fs::rename(tmp_meta, self.meta_path(key))?;
        Ok(ds)
    }

    pub fn save_chart(&self, key: &str, spec: &Json) -> Result<(), Error> {
        let tmp = self.dir.join(format!(".{key}{CHART_EXT}.tmp"));
        fs::write(&tmp, spec.to_string_pretty())?;
        fs::rename(tmp, self.chart_path(key))?;
        Ok(())
    }

    /// The saved result for `key`, or `None` if absent or not intact.
    /// Never reads the data itself: metadata, file size and the Parquet
    /// footer magic are checked instead.
    pub fn load(&self, key: &str, chart: bool) -> Option<Output> {
        if chart {
            let text = fs::read_to_string(self.chart_path(key)).ok()?;
            return Json::parse(&text).ok().map(Output::Chart);
        }
        let meta = Json::parse(&fs::read_to_string(self.meta_path(key)).ok()?).ok()?;
        let path = self.table_path(key);
        let ds = Dataset::from_meta(path.clone(), &meta).ok()?;
        let actual = fs::metadata(&path).ok()?.len();
        (actual == ds.bytes && ends_with_parquet_magic(&path)).then_some(Output::Table(ds))
    }

    /// Delete saved results whose key is not in `keep`, plus leftover
    /// temporary files. Returns how many results were removed.
    pub fn prune(&self, keep: &HashSet<String>) -> Result<usize, Error> {
        let mut removed = 0;
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if name.starts_with('.') {
                fs::remove_file(&path)?;
                continue;
            }
            let key = name
                .strip_suffix(META_EXT)
                .or_else(|| name.strip_suffix(CHART_EXT))
                .or_else(|| name.strip_suffix(TABLE_EXT));
            if let Some(k) = key {
                if !keep.contains(k) {
                    fs::remove_file(&path)?;
                    if !name.ends_with(META_EXT) {
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }
}

impl Drop for ArtifactStore {
    fn drop(&mut self) {
        if self.ephemeral {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

fn ends_with_parquet_magic(path: &Path) -> bool {
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut magic = [0u8; 4];
    f.seek(SeekFrom::End(-4)).is_ok() && f.read_exact(&mut magic).is_ok() && &magic == b"PAR1"
}
