//! Execution engines for relational steps.
//!
//! The session talks to an engine only through this trait (ADR-0001), so the
//! desktop app can use embedded DuckDB and a future server can use workers.
//!
//! Status:
//! - `ReferenceEngine`: implemented. Interprets visual steps directly; custom
//!   SQL is reported as unsupported.
//! - `DuckDbEngine`: NOT IMPLEMENTED in this slice (crates.io unreachable).
//!   It will register the upstream result as `input`, run `compile::step_sql`
//!   (or the user's SQL), and read CSV sources with `read_csv_auto`.

use crate::compile;
use crate::csv;
use crate::model::{Step, StepKind};
use crate::reference;
use crate::table::Table;
use crate::Error;
use std::path::Path;

pub trait Engine {
    /// Engine name and version. Part of every cache key, so upgrading the
    /// engine marks results stale instead of silently reusing them.
    fn fingerprint(&self) -> String;

    fn import_csv(&mut self, path: &Path) -> Result<Table, Error>;

    /// Execute a relational step (visual or custom SQL) against `input`.
    fn run_step(&mut self, step: &Step, input: &Table) -> Result<Table, Error>;
}

#[derive(Debug, Default)]
pub struct ReferenceEngine;

impl Engine for ReferenceEngine {
    fn fingerprint(&self) -> String {
        format!("reference-engine {}", env!("CARGO_PKG_VERSION"))
    }

    fn import_csv(&mut self, path: &Path) -> Result<Table, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Data(format!("cannot read source {}: {e}", path.display())))?;
        csv::read_table(&text)
    }

    fn run_step(&mut self, step: &Step, input: &Table) -> Result<Table, Error> {
        match &step.kind {
            StepKind::Sql { .. } => Err(Error::Unsupported(format!(
                "step `{}`: custom SQL needs the DuckDB engine, which is not built yet. Its SQL is:\n{}",
                step.id,
                compile::step_sql(&step.kind).unwrap_or_default()
            ))),
            _ => reference::eval_step(step, input),
        }
    }
}
