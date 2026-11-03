//! Execution engines.
//!
//! The session talks to an engine only through this trait (ADR-0001), so the
//! desktop app uses embedded DuckDB and a future server can use workers.
//! Engines exchange results as Parquet files (`Dataset`), never as whole
//! in-memory tables; only previews, profiles and chart data are materialized.
//!
//! - `duck::DuckDbEngine`: the production engine.
//! - `ReferenceEngine`: evaluates visual steps directly in memory. It is the
//!   correctness oracle for DuckDB (tests/equivalence.rs) and is meant for
//!   small data only; custom SQL is reported as unsupported.

use crate::compile;
use crate::csv;
use crate::dataset::Dataset;
use crate::duck::{self, DuckDbEngine};
use crate::model::{Step, StepKind};
use crate::reference;
use crate::table::{ColumnProfile, Table};
use crate::Error;
use std::path::Path;

pub trait Engine {
    /// Engine name and version. Part of every cache key, so upgrading the
    /// engine marks results stale instead of silently reusing them.
    fn fingerprint(&self) -> String;

    /// Convert a CSV source into a dataset written at `out`.
    fn import_csv(&mut self, src: &Path, out: &Path) -> Result<Dataset, Error>;

    /// Execute a relational step (visual or custom SQL), writing its result at `out`.
    fn run_step(&mut self, step: &Step, input: &Dataset, out: &Path) -> Result<Dataset, Error>;

    /// Turn a Parquet file produced elsewhere (a Python step) into a dataset
    /// with core types, written at `out`.
    fn adopt_parquet(&mut self, src: &Path, out: &Path) -> Result<Dataset, Error>;

    /// Rows `offset..offset+limit` in result order.
    fn preview(&mut self, ds: &Dataset, offset: u64, limit: u64) -> Result<Table, Error>;

    /// The first `limit` rows of selected columns (chart data).
    fn columns(&mut self, ds: &Dataset, columns: &[&str], limit: u64) -> Result<Table, Error>;

    /// Per-column type, missing values, distinct count, min and max.
    fn profile(&mut self, ds: &Dataset) -> Result<Vec<ColumnProfile>, Error>;
}

/// Reference engine: evaluation in memory, file I/O through DuckDB.
pub struct ReferenceEngine {
    io: DuckDbEngine,
}

impl ReferenceEngine {
    pub fn new() -> Self {
        ReferenceEngine { io: DuckDbEngine::new().expect("DuckDB starts") }
    }

    fn write(&mut self, t: &Table, out: &Path) -> Result<Dataset, Error> {
        let raw = out.with_extension("ref.parquet");
        duck::write_parquet(t, &raw)?;
        let ds = self.io.adopt_parquet(&raw, out);
        let _ = std::fs::remove_file(&raw);
        ds
    }
}

impl Default for ReferenceEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for ReferenceEngine {
    fn fingerprint(&self) -> String {
        format!("reference-engine {}", env!("CARGO_PKG_VERSION"))
    }

    fn import_csv(&mut self, src: &Path, out: &Path) -> Result<Dataset, Error> {
        let text = std::fs::read_to_string(src)
            .map_err(|e| Error::Data(format!("cannot read source {}: {e}", src.display())))?;
        let t = csv::read_table(&text)?;
        self.write(&t, out)
    }

    fn run_step(&mut self, step: &Step, input: &Dataset, out: &Path) -> Result<Dataset, Error> {
        if let StepKind::Sql { .. } = step.kind {
            return Err(Error::Unsupported(format!(
                "step `{}`: custom SQL needs the DuckDB engine. Its SQL is:\n{}",
                step.id,
                compile::step_sql(&step.kind).unwrap_or_default()
            )));
        }
        let t = duck::read_parquet(&input.path)?;
        let result = reference::eval_step(step, &t)?;
        self.write(&result, out)
    }

    fn adopt_parquet(&mut self, src: &Path, out: &Path) -> Result<Dataset, Error> {
        self.io.adopt_parquet(src, out)
    }

    fn preview(&mut self, ds: &Dataset, offset: u64, limit: u64) -> Result<Table, Error> {
        self.io.preview(ds, offset, limit)
    }

    fn columns(&mut self, ds: &Dataset, columns: &[&str], limit: u64) -> Result<Table, Error> {
        self.io.columns(ds, columns, limit)
    }

    fn profile(&mut self, ds: &Dataset) -> Result<Vec<ColumnProfile>, Error> {
        // Computed in memory, independently of DuckDB's SQL.
        Ok(duck::read_parquet(&ds.path)?.profile())
    }
}
