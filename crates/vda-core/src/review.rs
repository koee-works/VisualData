//! Review gate for code that runs with the user's own permissions (ADR-0004).
//!
//! Python steps run as the user, with access to their files and network, so
//! a Python step only runs once its exact code has been approved. Approvals
//! are SHA-256 fingerprints of the code, kept in a file *outside* any project
//! (a project cannot approve itself), and code is never stored there.
//! Changing a single character of approved code needs a new approval.
//!
//! Custom SQL needs no approval: it runs sandboxed (one read-only query,
//! file access limited to the project; see `duck::EngineOptions`). Visual
//! steps generate their own SQL.

use crate::hash::Sha256;
use crate::json::Json;
use crate::model::StepKind;
use crate::Error;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const FORMAT_VERSION: f64 = 1.0;

/// Fingerprint of a step's executable code, if the step needs review.
pub fn fingerprint(kind: &StepKind) -> Option<String> {
    match kind {
        StepKind::Python { code, .. } => {
            let mut h = Sha256::new();
            h.update(b"vda-review-v1\npython\n");
            h.update(code.as_bytes());
            Some(h.hex_digest())
        }
        _ => None,
    }
}

/// Approved code fingerprints, optionally persisted to a file.
#[derive(Debug, Clone, Default)]
pub struct Approvals {
    path: Option<PathBuf>,
    /// fingerprint → approval time (Unix ms)
    approved: BTreeMap<String, u128>,
}

impl Approvals {
    /// Approvals that are not saved anywhere (tests, one-off tools).
    pub fn in_memory() -> Self {
        Approvals::default()
    }

    /// Load approvals from `path`. A missing file means nothing is approved
    /// yet. A damaged file is an error rather than an empty list, so a
    /// broken file never silently discards earlier approvals.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, Error> {
        let path = path.into();
        let mut approved = BTreeMap::new();
        match fs::read_to_string(&path) {
            Ok(text) => {
                let j = Json::parse(&text)
                    .map_err(|e| Error::Format(format!("approvals file {} is damaged: {e}", path.display())))?;
                if let Some(Json::Obj(m)) = j.get("approved") {
                    for (fp, entry) in m {
                        let at = entry.get("approved_unix_ms").and_then(Json::as_str).and_then(|s| s.parse().ok());
                        approved.insert(fp.clone(), at.unwrap_or(0));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(Approvals { path: Some(path), approved })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn is_approved(&self, fingerprint: &str) -> bool {
        self.approved.contains_key(fingerprint)
    }

    pub fn approve(&mut self, fingerprint: &str) -> Result<(), Error> {
        self.merge_from_disk()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
        self.approved.entry(fingerprint.to_string()).or_insert(now);
        self.save()
    }

    pub fn revoke(&mut self, fingerprint: &str) -> Result<(), Error> {
        self.merge_from_disk()?;
        self.approved.remove(fingerprint);
        self.save()
    }

    /// Pick up approvals another window saved since this list was loaded,
    /// so saving never drops them.
    fn merge_from_disk(&mut self) -> Result<(), Error> {
        if let Some(path) = &self.path {
            for (fp, at) in Approvals::load(path.clone())?.approved {
                self.approved.entry(fp).or_insert(at);
            }
        }
        Ok(())
    }

    fn save(&self) -> Result<(), Error> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let entries = self
            .approved
            .iter()
            .map(|(fp, at)| (fp.clone(), Json::obj(vec![("approved_unix_ms", Json::str(at.to_string()))])))
            .collect();
        let doc = Json::obj(vec![("format_version", Json::Num(FORMAT_VERSION)), ("approved", Json::Obj(entries))]);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, doc.to_string_pretty())?;
        fs::rename(tmp, path)?;
        Ok(())
    }
}

/// Whether code must be approved before it runs.
#[derive(Debug, Clone, Default)]
pub enum ReviewPolicy {
    /// Everything runs. For programmatic use where the caller wrote the
    /// analysis (tests, scripts). The desktop app never uses this.
    #[default]
    RunEverything,
    /// Python steps run only when their code is approved.
    RequireApproval(Approvals),
}

impl ReviewPolicy {
    /// True if this step's code is waiting for review.
    pub fn pending(&self, kind: &StepKind) -> bool {
        match (self, fingerprint(kind)) {
            (ReviewPolicy::RequireApproval(a), Some(fp)) => !a.is_approved(&fp),
            _ => false,
        }
    }

    pub fn approve(&mut self, kind: &StepKind) -> Result<(), Error> {
        match (self, fingerprint(kind)) {
            (ReviewPolicy::RequireApproval(a), Some(fp)) => a.approve(&fp),
            _ => Ok(()),
        }
    }
}

/// Where the desktop app keeps approvals: `$VDA_APPROVALS`, else the
/// platform's per-user data directory.
pub fn default_approvals_path() -> PathBuf {
    if let Some(p) = std::env::var_os("VDA_APPROVALS") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".local/share")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("vda").join("approvals.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn py(code: &str) -> StepKind {
        StepKind::Python { code: code.into(), timeout_ms: 1000 }
    }

    #[test]
    fn approval_is_for_exact_code_and_persists() {
        let path = std::env::temp_dir().join(format!("vda-approvals-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut policy = ReviewPolicy::RequireApproval(Approvals::load(&path).unwrap());
        assert!(policy.pending(&py("result = df")));
        policy.approve(&py("result = df")).unwrap();
        assert!(!policy.pending(&py("result = df")));
        assert!(policy.pending(&py("result = df ")), "any change needs a new approval");

        let reloaded = ReviewPolicy::RequireApproval(Approvals::load(&path).unwrap());
        assert!(!reloaded.pending(&py("result = df")));
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("result = df"), "code is never stored with approvals");
    }

    #[test]
    fn only_python_needs_review() {
        let policy = ReviewPolicy::RequireApproval(Approvals::in_memory());
        assert!(!policy.pending(&StepKind::Sql { sql: "SELECT 1".into() }));
        assert!(!policy.pending(&StepKind::Filter { predicates: vec![] }));
        assert!(!ReviewPolicy::RunEverything.pending(&py("import os")));
    }

    #[test]
    fn damaged_file_is_an_error_not_an_empty_list() {
        let path = std::env::temp_dir().join(format!("vda-approvals-bad-{}.json", std::process::id()));
        fs::write(&path, "{ not json").unwrap();
        assert!(Approvals::load(&path).is_err());
    }
}
