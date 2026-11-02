//! Validates the step graph, orders execution, and computes cache keys.
//!
//! A step's cache key covers everything that can change its result: its
//! executable definition, its upstream key (or the source content hash), the
//! engine version, and for Python the interpreter/library versions. Because
//! keys chain, changing any upstream input changes every downstream key, which
//! is how stale results are detected.

use crate::hash::Sha256;
use crate::model::{Analysis, StepKind, SOURCE_ID};
use crate::persist::step_kind_to_json;
use crate::Error;
use std::collections::{HashMap, HashSet};

const KEY_VERSION: &str = "vda-key-v1";

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Step ids in a valid execution order.
    pub order: Vec<String>,
    pub keys: HashMap<String, String>,
}

pub struct Fingerprints<'a> {
    pub source: &'a str,
    pub engine: &'a str,
    /// `None` when no Python interpreter is available; Python steps then fail
    /// validation at run time rather than here.
    pub python: Option<&'a str>,
}

pub fn validate(a: &Analysis) -> Result<Vec<String>, Error> {
    let mut seen = HashSet::new();
    for s in &a.steps {
        if s.id == SOURCE_ID || s.id.is_empty() {
            return Err(Error::Validation(format!("invalid step id `{}`", s.id)));
        }
        if !seen.insert(s.id.as_str()) {
            return Err(Error::Validation(format!("duplicate step id `{}`", s.id)));
        }
    }
    for s in &a.steps {
        if s.input == SOURCE_ID {
            continue;
        }
        let up = a
            .step(&s.input)
            .ok_or_else(|| Error::Validation(format!("step `{}` reads unknown step `{}`", s.id, s.input)))?;
        if !up.kind.produces_table() {
            return Err(Error::Validation(format!(
                "step `{}` reads `{}`, which is a chart and has no table output",
                s.id, up.id
            )));
        }
    }
    for c in &a.cells {
        if let crate::model::Cell::Step { step_id } = c {
            if a.step(step_id).is_none() {
                return Err(Error::Validation(format!("notebook cell refers to unknown step `{step_id}`")));
            }
        }
    }
    // Each step has one input, so the graph is a forest unless there is a cycle.
    let mut order: Vec<String> = Vec::new();
    let mut done: HashSet<&str> = HashSet::new();
    for s in &a.steps {
        let mut chain = Vec::new();
        let mut cur = s.id.as_str();
        let mut on_path = HashSet::new();
        while cur != SOURCE_ID && !done.contains(cur) {
            if !on_path.insert(cur) {
                return Err(Error::Validation(format!("cycle detected at step `{cur}`")));
            }
            chain.push(cur);
            cur = &a.step(cur).unwrap().input;
        }
        for id in chain.into_iter().rev() {
            done.insert(id);
            order.push(id.to_string());
        }
    }
    Ok(order)
}

pub fn plan(a: &Analysis, fp: &Fingerprints) -> Result<Plan, Error> {
    let order = validate(a)?;
    let mut keys: HashMap<String, String> = HashMap::new();
    for id in &order {
        let step = a.step(id).unwrap();
        let upstream = if step.input == SOURCE_ID {
            format!("source:{}", fp.source)
        } else {
            format!("step:{}", keys[&step.input])
        };
        let mut h = Sha256::new();
        for part in [
            KEY_VERSION,
            fp.engine,
            &upstream,
            &step_kind_to_json(&step.kind).to_string_compact(),
        ] {
            h.update(part.as_bytes());
            h.update(b"\n");
        }
        if let StepKind::Python { .. } = step.kind {
            h.update(fp.python.unwrap_or("python:unavailable").as_bytes());
        }
        keys.insert(id.clone(), h.hex_digest());
    }
    Ok(Plan { order, keys })
}

/// Steps that depend (directly or transitively) on `step_id`, in execution order.
pub fn downstream(a: &Analysis, order: &[String], step_id: &str) -> Vec<String> {
    let mut affected: HashSet<String> = HashSet::from([step_id.to_string()]);
    let mut out = Vec::new();
    for id in order {
        let s = a.step(id).unwrap();
        if affected.contains(&s.input) {
            affected.insert(id.clone());
            out.push(id.clone());
        }
    }
    out
}
