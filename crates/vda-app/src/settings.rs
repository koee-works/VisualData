//! Per-user settings, kept beside the approvals file (outside every project):
//! the Python and R chosen in the app, and the interface's preferences
//! (theme, custom themes, editor keys, ...), which the app reads and writes
//! as a whole and this layer keeps without interpreting, apart from the
//! name comments are signed with.

use crate::{fail, App, AppError, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use vda_core::python::PythonRunner;

#[derive(Debug, Clone, Default)]
pub struct Settings {
    path: Option<PathBuf>,
    pub python: Option<PathBuf>,
    /// `Rscript` for R steps.
    pub r: Option<PathBuf>,
    /// The interface's preferences: a JSON object.
    pub preferences: serde_json::Value,
}

/// Most a preferences object may take on disk (custom themes are small).
const MAX_PREFERENCES: usize = 512 * 1024;

impl Settings {
    pub fn in_memory() -> Self {
        Settings::default()
    }

    /// Settings in this file; a missing or unreadable file means defaults.
    pub fn load(path: PathBuf) -> Self {
        let j = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
        let get = |k: &str| j.as_ref().and_then(|j| j[k].as_str().map(PathBuf::from));
        let preferences = j.as_ref().map(|j| j["preferences"].clone()).filter(|p| p.is_object()).unwrap_or_else(|| serde_json::json!({}));
        Settings { python: get("python"), r: get("r"), preferences, path: Some(path) }
    }

    fn save(&self) -> Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let j = serde_json::json!({
            "python": self.python.as_ref().map(|p| p.to_string_lossy()),
            "r": self.r.as_ref().map(|p| p.to_string_lossy()),
            "preferences": self.preferences,
        });
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&j).unwrap())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Next to the approvals file.
pub fn default_settings_path() -> PathBuf {
    let approvals = vda_core::review::default_approvals_path();
    approvals.parent().map(Path::to_path_buf).unwrap_or_default().join("settings.json")
}

/// Which interpreter to use, and why: `VDA_PYTHON` wins over the choice made
/// in the app, which wins over `python3` on the PATH.
pub fn choose_interpreter(settings: &Settings) -> (PathBuf, &'static str) {
    if let Some(p) = std::env::var_os("VDA_PYTHON").filter(|p| !p.is_empty()) {
        return (PathBuf::from(p), "env");
    }
    match &settings.python {
        Some(p) => (p.clone(), "chosen"),
        None => (vda_core::python::default_interpreter(), "default"),
    }
}

pub fn choose_rscript(settings: &Settings) -> (PathBuf, &'static str) {
    if let Some(p) = std::env::var_os("VDA_R").filter(|p| !p.is_empty()) {
        return (PathBuf::from(p), "env");
    }
    match &settings.r {
        Some(p) => (p.clone(), "chosen"),
        None => (PathBuf::from("Rscript"), "default"),
    }
}

/// R installations worth offering: `Rscript` on the PATH and where the R
/// installers and Posit's builds put versions side by side.
pub fn discover_r() -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            found.push(dir.join(if cfg!(windows) { "Rscript.exe" } else { "Rscript" }));
        }
    }
    let mut versions = |root: &str, tail: &[&str]| {
        if let Ok(entries) = std::fs::read_dir(root) {
            let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            dirs.sort();
            for d in dirs {
                found.push(tail.iter().fold(d, |p, t| p.join(t)));
            }
        }
    };
    versions("/opt/R", &["bin", "Rscript"]);
    versions("/Library/Frameworks/R.framework/Versions", &["Resources", "bin", "Rscript"]);
    versions("C:\\Program Files\\R", &["bin", "Rscript.exe"]);
    for p in ["/usr/local/bin/Rscript", "/opt/homebrew/bin/Rscript", "/usr/bin/Rscript"] {
        found.push(PathBuf::from(p));
    }
    let mut seen = std::collections::HashSet::new();
    found.into_iter().filter(|p| p.is_file()).filter(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))).collect()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RView {
    pub rscript: String,
    pub version: Option<String>,
    /// Package libraries R steps see (where RStudio installs packages).
    pub libraries: Vec<String>,
    pub packages: Vec<PackageView>,
    pub problem: Option<String>,
    pub chosen_by: String,
    pub candidates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PackageView {
    pub name: String,
    pub purpose: String,
    pub required: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PythonView {
    pub interpreter: String,
    pub executable: Option<String>,
    pub version: Option<String>,
    pub packages: Vec<PackageView>,
    /// Why Python steps cannot run with it, if they cannot.
    pub problem: Option<String>,
    /// "env" (`VDA_PYTHON`), "chosen" (in the app) or "default".
    pub chosen_by: String,
    /// Other interpreters found on this computer.
    pub candidates: Vec<String>,
}

impl App {
    fn python_view(&self, env: vda_core::python::Environment) -> PythonView {
        let project = self.session.as_ref().map(|s| s.project_dir().to_path_buf());
        let current = std::fs::canonicalize(&env.interpreter).ok();
        PythonView {
            interpreter: env.interpreter.to_string_lossy().into_owned(),
            executable: env.executable.clone(),
            version: env.version.clone(),
            packages: env
                .packages
                .iter()
                .map(|(name, purpose, required, version)| PackageView { name: name.clone(), purpose: purpose.clone(), required: *required, version: version.clone() })
                .collect(),
            problem: env.problem.clone(),
            chosen_by: self.python_chosen_by.to_string(),
            candidates: vda_core::python::discover(project.as_deref())
                .into_iter()
                .filter(|p| std::fs::canonicalize(p).ok() != current)
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        }
    }

    /// The interpreter Python steps use: its version, location and packages.
    pub fn python_environment(&mut self) -> Result<PythonView> {
        let env = PythonRunner::new(self.interpreter.clone(), std::env::temp_dir()).environment();
        Ok(self.python_view(env))
    }

    /// Use another interpreter for Python steps, remembered for this user.
    /// It must have pandas and pyarrow. Python steps whose results came from
    /// another interpreter (or other package versions) become outdated.
    fn r_view(&self, env: vda_core::rlang::REnvironment) -> RView {
        let current = std::fs::canonicalize(vda_core::python::resolve(&env.rscript)).ok();
        RView {
            rscript: env.rscript.to_string_lossy().into_owned(),
            version: env.version.clone(),
            libraries: env.libraries.clone(),
            packages: env
                .packages
                .iter()
                .map(|(name, purpose, version)| PackageView { name: name.clone(), purpose: purpose.clone(), required: false, version: version.clone() })
                .collect(),
            problem: env.problem.clone(),
            chosen_by: self.r_chosen_by.to_string(),
            candidates: discover_r().into_iter().filter(|p| std::fs::canonicalize(p).ok() != current).map(|p| p.to_string_lossy().into_owned()).collect(),
        }
    }

    pub fn r_environment(&mut self) -> Result<RView> {
        let env = vda_core::rlang::RRunner::new(self.rscript.clone(), std::env::temp_dir()).environment();
        Ok(self.r_view(env))
    }

    pub fn set_r(&mut self, path: &Path) -> Result<RView> {
        if self.r_chosen_by == "env" {
            return fail("VDA_R is set, so it decides which R runs; unset it to choose one here");
        }
        let env = vda_core::rlang::RRunner::new(path, std::env::temp_dir()).environment();
        if let Some(problem) = &env.problem {
            return Err(AppError::from(format!("cannot use {}: {problem}", path.display())));
        }
        self.settings.r = Some(path.to_path_buf());
        self.settings.save()?;
        self.rscript = path.to_path_buf();
        self.r_chosen_by = "chosen";
        if let Some(dir) = self.session.as_ref().map(|s| s.project_dir().to_path_buf()) {
            self.start(None, &dir)?;
        }
        Ok(self.r_view(env))
    }

    /// The interface's preferences, with `author` filled in.
    pub fn preferences(&self) -> serde_json::Value {
        let mut p = if self.settings.preferences.is_object() { self.settings.preferences.clone() } else { serde_json::json!({}) };
        p["author"] = serde_json::Value::String(self.author.clone());
        p
    }

    /// Replace the interface's preferences. A non-empty `author` signs comments.
    pub fn set_preferences(&mut self, p: &serde_json::Value) -> Result<serde_json::Value> {
        if !p.is_object() {
            return fail("preferences must be an object");
        }
        if serde_json::to_string(p).map(|t| t.len()).unwrap_or(0) > MAX_PREFERENCES {
            return fail("preferences are too large to keep (most is 512 KB)");
        }
        if let Some(name) = p["author"].as_str().map(str::trim) {
            if !name.is_empty() {
                if name.chars().count() > 80 {
                    return fail("a name for comments is at most 80 characters");
                }
                self.author = name.to_string();
            }
        }
        let before = self.code_memory_limit();
        self.settings.preferences = p.clone();
        self.settings.save()?;
        // Python and R programs start with their limit, so a new limit
        // applies from a fresh session (nothing is lost: it is all saved).
        if before != self.code_memory_limit() {
            if let Some(dir) = self.session.as_ref().map(|s| s.project_dir().to_path_buf()) {
                self.start(None, &dir)?;
            }
        }
        Ok(self.preferences())
    }

    pub fn set_python(&mut self, path: &Path) -> Result<PythonView> {
        if self.python_chosen_by == "env" {
            return fail("VDA_PYTHON is set, so it decides the interpreter; unset it to choose one here");
        }
        let env = PythonRunner::new(path, std::env::temp_dir()).environment();
        if let Some(problem) = &env.problem {
            return Err(AppError::from(format!("cannot use {}: {problem}", path.display())));
        }
        self.settings.python = Some(path.to_path_buf());
        self.settings.save()?;
        self.interpreter = path.to_path_buf();
        self.python_chosen_by = "chosen";
        // Reopen so the new interpreter is part of every Python step's key.
        if let Some(dir) = self.session.as_ref().map(|s| s.project_dir().to_path_buf()) {
            self.start(None, &dir)?;
        }
        Ok(self.python_view(env))
    }
}

/// Where the app keeps what is yours, and what it holds.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AboutView {
    /// This build's version.
    pub version: String,
    pub settings_path: Option<String>,
    pub approvals_path: Option<String>,
    /// Pieces of Python and R code approved to run.
    pub approvals: usize,
}

impl App {
    /// The memory limit for Python and R programs from Settings: `None` when
    /// unset (each language keeps its own), `Some(None)` for no limit, and
    /// `Some(Some(mb))` for a limit.
    pub(crate) fn code_memory_limit(&self) -> Option<Option<u64>> {
        let mb = self.settings.preferences["codeMemoryMb"].as_u64()?;
        Some(if mb == 0 { None } else { Some(mb.clamp(256, 262_144)) })
    }

    /// How many saved versions a project's history keeps.
    pub(crate) fn versions_kept(&self) -> usize {
        match self.settings.preferences["versionsKept"].as_u64() {
            Some(n) => (n as usize).clamp(*crate::history::KEEP_RANGE.start(), *crate::history::KEEP_RANGE.end()),
            None => crate::history::MAX_VERSIONS,
        }
    }

    /// Approvals: from the file when there is one, else this App's own.
    fn approvals_now(&self) -> Result<vda_core::review::Approvals> {
        Ok(match &self.approvals_path {
            Some(p) => vda_core::review::Approvals::load(p.clone())?,
            None => self.memory_approvals.clone(),
        })
    }

    pub fn about(&self) -> Result<AboutView> {
        Ok(AboutView {
            version: env!("CARGO_PKG_VERSION").to_string(),
            settings_path: self.settings.path.as_ref().map(|p| p.display().to_string()),
            approvals_path: self.approvals_path.as_ref().map(|p| p.display().to_string()),
            approvals: self.approvals_now()?.count(),
        })
    }

    /// Forget every approval: Python and R steps ask for review again. With a
    /// project open, returns it as it now is.
    pub fn forget_approvals(&mut self) -> Result<Option<crate::AnalysisView>> {
        match &self.approvals_path {
            Some(p) => vda_core::review::Approvals::load(p.clone())?.clear()?,
            None => self.memory_approvals.clear()?,
        }
        let Some(dir) = self.session.as_ref().map(|s| s.project_dir().to_path_buf()) else { return Ok(None) };
        self.start(None, &dir)?;
        self.view().map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn memory_limit_and_versions_read_from_preferences() {
        // In memory: `App::default()` would write the real settings file.
        let mut app = App::new(Default::default(), "python3".into());
        assert_eq!(app.code_memory_limit(), None);
        assert_eq!(app.versions_kept(), crate::history::MAX_VERSIONS);
        app.set_preferences(&json!({"codeMemoryMb": 2048, "versionsKept": 100})).unwrap();
        assert_eq!(app.code_memory_limit(), Some(Some(2048)));
        assert_eq!(app.versions_kept(), 100);
        app.set_preferences(&json!({"codeMemoryMb": 0, "versionsKept": 1})).unwrap();
        assert_eq!(app.code_memory_limit(), Some(None));
        assert_eq!(app.versions_kept(), 10);
        // Too small a limit to run anything is raised; text is ignored.
        app.set_preferences(&json!({"codeMemoryMb": 5, "versionsKept": "many"})).unwrap();
        assert_eq!(app.code_memory_limit(), Some(Some(256)));
        assert_eq!(app.versions_kept(), crate::history::MAX_VERSIONS);
    }
}
