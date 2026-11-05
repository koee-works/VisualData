//! Core of the visual data analysis app: analysis model, visual-op → SQL
//! compilation, planning and cache keys, execution sessions, run records and
//! replay. No UI or Tauri dependency (see docs/adr/0001-deployment-model.md).

pub mod artifacts;
pub mod chart;
pub mod check;
pub mod clean;
pub mod compile;
pub mod console;
pub mod csv;
pub mod database;
pub mod dataset;
pub mod demo;
pub mod describe;
pub mod duck;
pub mod engine;
pub mod excel;
pub mod export;
pub mod formula;
pub mod hash;
pub mod json;
pub mod jupyter;
pub mod lint;
pub mod model;
pub mod notebook;
pub mod params;
pub mod persist;
pub mod profile;
pub mod plan;
pub mod python;
pub mod rlang;
pub mod reference;
pub mod rename;
pub mod review;
pub mod session;
pub mod stat_test;
pub mod stats;
pub mod table;

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Invalid analysis definition (unknown column, bad type, broken graph).
    Validation(String),
    /// Problem with the data itself.
    Data(String),
    /// Unreadable or incompatible file format.
    Format(String),
    /// Custom Python step failed, timed out, or could not start.
    Python(String),
    /// Custom R step failed, timed out, or could not start.
    R(String),
    /// Feature or boundary not implemented in this build.
    Unsupported(String),
    Cancelled,
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Validation(m) => write!(f, "invalid analysis: {m}"),
            Error::Data(m) => write!(f, "data error: {m}"),
            Error::Format(m) => write!(f, "format error: {m}"),
            Error::Python(m) => write!(f, "python step: {m}"),
            Error::R(m) => write!(f, "R step: {m}"),
            Error::Unsupported(m) => write!(f, "not supported yet: {m}"),
            Error::Cancelled => write!(f, "cancelled"),
            Error::Io(e) => write!(f, "i/o error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
