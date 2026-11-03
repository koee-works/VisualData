//! Per-user settings, kept beside the approvals file (outside every project):
//! for now, the Python interpreter chosen in the app.

use crate::{fail, App, AppError, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use vda_core::python::PythonRunner;

#[derive(Debug, Clone, Default)]
pub struct Settings {
    path: Option<PathBuf>,
    pub python: Option<PathBuf>,
}

impl Settings {
    pub fn in_memory() -> Self {
        Settings::default()
    }

    /// Settings in this file; a missing or unreadable file means defaults.
    pub fn load(path: PathBuf) -> Self {
        let python = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|j| j["python"].as_str().map(PathBuf::from));
        Settings { path: Some(path), python }
    }

    fn save(&self) -> Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let j = serde_json::json!({ "python": self.python.as_ref().map(|p| p.to_string_lossy()) });
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
