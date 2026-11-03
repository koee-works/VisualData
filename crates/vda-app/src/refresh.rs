//! Refreshing an analysis without the app: `vda-run`, started by the
//! computer's own scheduler (cron, launchd, Task Scheduler).
//!
//! A refresh re-reads the sources (a changed file outdates what reads it),
//! runs what is outdated, records the run as "scheduled", and can then
//! export results and the notebook. It follows the same rules as the app:
//! Python code runs only if this user has approved it, and SQL stays inside
//! the project folder. Only one refresh of a project runs at a time.

use crate::{fail, App, Result, StepEvent};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// A lock older than this is from a refresh that did not finish.
pub const STALE_LOCK_SECS: u64 = 6 * 3600;

/// Held while a refresh runs: `runs/refresh.lock`, removed when dropped.
pub struct Lock(PathBuf);

impl Lock {
    pub fn take(project: &Path) -> Result<Lock> {
        let dir = project.join("runs");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("refresh.lock");
        for _ in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = writeln!(f, "{}", std::process::id());
                    return Ok(Lock(path));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs());
                    if age.is_some_and(|a| a > STALE_LOCK_SECS) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    return fail(format!("another refresh of this project is running ({} exists)", path.display()));
                }
                Err(e) => return Err(e.into()),
            }
        }
        fail(format!("could not take {}", path.display()))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RefreshReport {
    pub succeeded: bool,
    /// Steps that ran, were reused, failed, or wait for code review.
    pub ran: usize,
    pub reused: usize,
    pub failed: Vec<(String, String)>,
    pub needs_review: Vec<String>,
    /// Files written after the run: (path, rows or 0 for a notebook).
    pub exported: Vec<(String, u64)>,
}

impl RefreshReport {
    /// The process exit code: 0 all current, 1 a step failed, 3 code
    /// waits for review (approve it in the app).
    pub fn exit_code(&self) -> i32 {
        if !self.failed.is_empty() {
            1
        } else if !self.needs_review.is_empty() {
            3
        } else {
            0
        }
    }
}

impl App {
    /// Refresh the open project: run, then export each `(step or source,
    /// file)` and the notebook if asked. Exports wait for a fully
    /// successful run, so a file never mixes old and new results.
    pub fn refresh(&mut self, exports: &[(String, PathBuf)], notebook: Option<&Path>, on_step: &mut dyn FnMut(StepEvent)) -> Result<RefreshReport> {
        let project = self.session()?.project_dir().to_path_buf();
        let _lock = Lock::take(&project)?;
        for (id, _) in exports {
            let a = &self.session()?.analysis;
            if !a.is_source(id) && a.step(id).is_none() {
                return fail(format!("cannot export `{id}`: no such step or source"));
            }
        }
        let summary = self.run_as(Some("scheduled"), on_step)?;
        let mut report = RefreshReport { succeeded: summary.succeeded, ran: 0, reused: 0, failed: vec![], needs_review: vec![], exported: vec![] };
        for e in &summary.steps {
            match e.status.as_str() {
                "succeeded" if e.cached => report.reused += 1,
                "succeeded" => report.ran += 1,
                "failed" => report.failed.push((e.step_id.clone(), e.detail.clone().unwrap_or_default())),
                "needs review" => report.needs_review.push(e.step_id.clone()),
                _ => {}
            }
        }
        if summary.succeeded {
            for (id, path) in exports {
                let rows = self.export(id, path)?;
                report.exported.push((path.to_string_lossy().into_owned(), rows));
            }
            if let Some(path) = notebook {
                self.export_notebook(path)?;
                report.exported.push((path.to_string_lossy().into_owned(), 0));
            }
        }
        Ok(report)
    }
}

/// How often a schedule runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Every {
    Hour,
    Day,
    Weekday,
    Week,
}

impl Every {
    pub fn parse(s: &str) -> Option<Every> {
        Some(match s {
            "hour" | "hourly" => Every::Hour,
            "day" | "daily" => Every::Day,
            "weekday" | "weekdays" => Every::Weekday,
            "week" | "weekly" => Every::Week,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ScheduleView {
    /// The command a schedule runs.
    pub command: String,
    /// The entry for this computer's scheduler, and where it goes.
    pub entry: String,
    pub how: String,
    /// "cron", "launchd" or "schtasks".
    pub scheduler: String,
}

fn shell_quote(s: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `vda-run` beside the running app, or on the PATH.
pub fn runner_path() -> PathBuf {
    let name = if cfg!(windows) { "vda-run.exe" } else { "vda-run" };
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(name))).filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(name))
}

/// The scheduler entry that refreshes `project` every hour, day, weekday
/// or week (at `hour:minute`, ignored for hourly), for this computer's OS.
pub fn schedule(project: &Path, every: Every, hour: u32, minute: u32, exports: &[(String, PathBuf)]) -> Result<ScheduleView> {
    if hour > 23 || minute > 59 {
        return fail("choose a time between 00:00 and 23:59");
    }
    let mut args = vec![runner_path().to_string_lossy().into_owned(), project.to_string_lossy().into_owned()];
    for (id, path) in exports {
        args.push("--export".into());
        args.push(format!("{id}={}", path.display()));
    }
    let command = args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
    let log = project.join("runs").join("scheduled.log");
    if cfg!(target_os = "macos") {
        let interval = match every {
            Every::Hour => format!("<key>StartCalendarInterval</key><dict><key>Minute</key><integer>{minute}</integer></dict>"),
            Every::Day => format!("<key>StartCalendarInterval</key><dict><key>Hour</key><integer>{hour}</integer><key>Minute</key><integer>{minute}</integer></dict>"),
            Every::Weekday => format!(
                "<key>StartCalendarInterval</key><array>{}</array>",
                (1..=5).map(|d| format!("<dict><key>Weekday</key><integer>{d}</integer><key>Hour</key><integer>{hour}</integer><key>Minute</key><integer>{minute}</integer></dict>")).collect::<String>()
            ),
            Every::Week => format!("<key>StartCalendarInterval</key><dict><key>Weekday</key><integer>1</integer><key>Hour</key><integer>{hour}</integer><key>Minute</key><integer>{minute}</integer></dict>"),
        };
        let xml = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let program: String = args.iter().map(|a| format!("<string>{}</string>", xml(a))).collect();
        let label = format!("vda.refresh.{}", project.file_name().map(|n| n.to_string_lossy().replace(|c: char| !c.is_ascii_alphanumeric(), "-")).unwrap_or_default());
        let entry = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{label}</string>\n<key>ProgramArguments</key><array>{program}</array>\n{interval}\n<key>StandardOutPath</key><string>{}</string>\n<key>StandardErrorPath</key><string>{}</string>\n</dict></plist>\n",
            xml(&log.to_string_lossy()),
            xml(&log.to_string_lossy())
        );
        return Ok(ScheduleView {
            command,
            entry,
            how: format!("Save as ~/Library/LaunchAgents/{label}.plist, then run: launchctl load ~/Library/LaunchAgents/{label}.plist"),
            scheduler: "launchd".into(),
        });
    }
    if cfg!(windows) {
        let (sc, extra) = match every {
            Every::Hour => ("HOURLY", String::new()),
            Every::Day => ("DAILY", format!(" /ST {hour:02}:{minute:02}")),
            Every::Weekday => ("WEEKLY", format!(" /D MON,TUE,WED,THU,FRI /ST {hour:02}:{minute:02}")),
            Every::Week => ("WEEKLY", format!(" /D MON /ST {hour:02}:{minute:02}")),
        };
        let entry = format!("schtasks /Create /TN \"VDA refresh {}\" /SC {sc}{extra} /TR \"{}\"", project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), command.replace('"', "\\\""));
        return Ok(ScheduleView { command, entry, how: "Run this once in a Command Prompt.".into(), scheduler: "schtasks".into() });
    }
    let when = match every {
        Every::Hour => format!("{minute} * * * *"),
        Every::Day => format!("{minute} {hour} * * *"),
        Every::Weekday => format!("{minute} {hour} * * 1-5"),
        Every::Week => format!("{minute} {hour} * * 1"),
    };
    let entry = format!("{when} {command} >> {} 2>&1", shell_quote(&log.to_string_lossy()));
    Ok(ScheduleView { command, entry, how: "Add this line with: crontab -e".into(), scheduler: "cron".into() })
}

impl App {
    /// The scheduler entry for the open project.
    pub fn schedule(&mut self, every: &str, hour: u32, minute: u32, exports: &[(String, PathBuf)]) -> Result<ScheduleView> {
        let Some(every) = Every::parse(every) else { return fail("choose hourly, daily, weekdays or weekly") };
        let project = self.session()?.project_dir().canonicalize()?;
        schedule(&project, every, hour, minute, exports)
    }
}
