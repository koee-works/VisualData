//! Validates the step graph, orders execution, and computes cache keys.
//!
//! A step's cache key covers everything that can change its result: its
//! executable definition, its upstream keys (or source content hashes), the
//! engine version, and for Python the interpreter/library versions. Because
//! keys chain, changing any upstream input changes every downstream key, which
//! is how stale results are detected.

use crate::hash::Sha256;
use crate::model::{Analysis, StepKind};
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
    /// Content hash of each source, by source id.
    pub sources: &'a HashMap<String, String>,
    pub engine: &'a str,
    /// `None` when no Python interpreter is available; Python steps then fail
    /// validation at run time rather than here.
    pub python: Option<&'a str>,
}

/// Source ids are referenced unquoted in custom SQL, so they must be plain
/// identifiers.
pub fn valid_source_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !id.eq_ignore_ascii_case("input")
}

pub fn validate(a: &Analysis) -> Result<Vec<String>, Error> {
    if a.sources.is_empty() {
        return Err(Error::Validation("the analysis has no source".into()));
    }
    let mut seen = HashSet::new();
    for src in &a.sources {
        if !valid_source_id(&src.id) {
            return Err(Error::Validation(format!(
                "source id `{}` must start with a letter and use only letters, digits and _",
                src.id
            )));
        }
        if !seen.insert(src.id.as_str()) {
            return Err(Error::Validation(format!("duplicate source id `{}`", src.id)));
        }
    }
    for s in &a.steps {
        if s.id.is_empty() || s.id.eq_ignore_ascii_case("input") {
            return Err(Error::Validation(format!("invalid step id `{}`", s.id)));
        }
        if !seen.insert(s.id.as_str()) {
            return Err(Error::Validation(format!("duplicate step id `{}`", s.id)));
        }
    }
    for s in &a.steps {
        let (min, max) = s.kind.arity();
        let n = s.inputs.len();
        if n < min || max.is_some_and(|m| n > m) {
            let want = match max {
                Some(m) if m == min => format!("{min}"),
                Some(m) => format!("{min} to {m}"),
                None => format!("at least {min}"),
            };
            return Err(Error::Validation(format!(
                "step `{}` ({}) reads {want} input{}, not {n}",
                s.id,
                s.kind.type_name(),
                if want == "1" { "" } else { "s" }
            )));
        }
        let mut distinct = HashSet::new();
        for input in &s.inputs {
            if !distinct.insert(input.as_str()) {
                return Err(Error::Validation(format!("step `{}` reads `{input}` twice", s.id)));
            }
            if a.is_source(input) {
                continue;
            }
            let up = a
                .step(input)
                .ok_or_else(|| Error::Validation(format!("step `{}` reads unknown step `{input}`", s.id)))?;
            if !up.kind.produces_table() {
                return Err(Error::Validation(format!(
                    "step `{}` reads `{}`, which is a chart and has no table output",
                    s.id, up.id
                )));
            }
        }
    }
    for c in &a.cells {
        if let crate::model::Cell::Step { step_id } = c {
            if a.step(step_id).is_none() {
                return Err(Error::Validation(format!("notebook cell refers to unknown step `{step_id}`")));
            }
        }
    }
    // Depth-first topological order; steps keep their listed order where the
    // graph allows it.
    fn visit<'a>(
        a: &'a Analysis,
        id: &'a str,
        done: &mut HashSet<&'a str>,
        on_path: &mut HashSet<&'a str>,
        order: &mut Vec<String>,
    ) -> Result<(), Error> {
        if done.contains(id) || a.is_source(id) {
            return Ok(());
        }
        if !on_path.insert(id) {
            return Err(Error::Validation(format!("cycle detected at step `{id}`")));
        }
        for input in &a.step(id).unwrap().inputs {
            visit(a, input, done, on_path, order)?;
        }
        on_path.remove(id);
        done.insert(id);
        order.push(id.to_string());
        Ok(())
    }
    let mut order = Vec::new();
    let mut done = HashSet::new();
    for s in &a.steps {
        visit(a, &s.id, &mut done, &mut HashSet::new(), &mut order)?;
    }
    Ok(order)
}

pub fn plan(a: &Analysis, fp: &Fingerprints) -> Result<Plan, Error> {
    let order = validate(a)?;
    let mut keys: HashMap<String, String> = HashMap::new();
    for id in &order {
        let step = a.step(id).unwrap();
        // For one input this is exactly the format-1 key input, so projects
        // saved before multi-input steps keep their saved results. With
        // several inputs their ids matter too (a join names clashing columns
        // after the right input, an append records each row's input), so
        // they are part of the key.
        let mut upstream = step
            .inputs
            .iter()
            .map(|input| match fp.sources.get(input) {
                Some(hash) if a.is_source(input) => Ok(format!("source:{hash}")),
                _ if a.is_source(input) => Err(Error::Data(format!("source `{input}` has no content hash"))),
                _ => Ok(format!("step:{}", keys[input])),
            })
            .collect::<Result<Vec<_>, Error>>()?
            .join("\n");
        if step.inputs.len() > 1 {
            upstream = format!("inputs:{}\n{upstream}", step.inputs.join(","));
        }
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
            // Python can read any parameter. Without parameters the key is
            // unchanged, so older projects keep their saved results.
            if !a.parameters.is_empty() {
                h.update(b"\nparams:");
                h.update(crate::params::python_json(&a.parameters).as_bytes());
            }
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
        if s.inputs.iter().any(|i| affected.contains(i)) {
            affected.insert(id.clone());
            out.push(id.clone());
        }
    }
    out
}
