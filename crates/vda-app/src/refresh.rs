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

const RUNNER: &str = if cfg!(windows) { "vda-run.exe" } else { "vda-run" };

/// `vda-run` for a schedule: beside the running app when the app is
/// installed, else a copy kept in the user's data folder (see
/// [`runs_from_a_temporary_place`]), else whatever `vda-run` is on the PATH.
pub fn runner_path() -> PathBuf {
    let exe = std::env::current_exe().ok();
    let beside = exe.as_ref().and_then(|e| e.parent()).map(|d| d.join(RUNNER)).filter(|p| p.is_file());
    let Some(beside) = beside else { return PathBuf::from(RUNNER) };
    let temporary = exe.as_deref().is_some_and(|e| runs_from_a_temporary_place(e, std::env::var_os("APPIMAGE").is_some()));
    if !temporary {
        return beside;
    }
    keep_runner(&beside, &kept_runner_path()).unwrap_or(beside)
}

/// Where a copy of `vda-run` is kept for schedules: beside approvals.json.
pub fn kept_runner_path() -> PathBuf {
    let approvals = vda_core::review::default_approvals_path();
    approvals.parent().map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir).join("bin").join(RUNNER)
}

/// Whether the app runs from somewhere that goes away or moves: an
/// AppImage (mounted afresh each time it starts), a Mac app run from its
/// disk image, or one macOS has moved aside until it is put in Applications.
pub fn runs_from_a_temporary_place(exe: &Path, appimage: bool) -> bool {
    let p = exe.to_string_lossy();
    appimage || p.contains("/AppTranslocation/") || p.starts_with("/Volumes/")
}

/// Copy `from` to `to` unless it is already the same file; the copy is
/// written beside `to` first and renamed, so a scheduled run that starts
/// meanwhile never reads half a file.
pub fn keep_runner(from: &Path, to: &Path) -> std::io::Result<PathBuf> {
    let new = std::fs::read(from)?;
    if std::fs::read(to).ok().as_deref() == Some(&new[..]) {
        return Ok(to.to_path_buf());
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = to.with_extension("part");
    std::fs::write(&part, &new)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&part, to)?;
    Ok(to.to_path_buf())
}

/// After an update, bring the kept copy up to date with the app (only when
/// one was made: schedules made earlier point at it).
pub fn refresh_kept_runner() {
    let kept = kept_runner_path();
    if kept.is_file() {
        let _ = runner_path();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_do_not_point_into_a_temporary_mount() {
        assert!(runs_from_a_temporary_place(Path::new("/tmp/.mount_VDAx1/usr/bin/vda-desktop"), true));
        assert!(runs_from_a_temporary_place(Path::new("/private/var/folders/x/AppTranslocation/ABC/d/VDA.app/Contents/MacOS/vda-desktop"), false));
        assert!(runs_from_a_temporary_place(Path::new("/Volumes/VDA/VDA.app/Contents/MacOS/vda-desktop"), false));
        assert!(!runs_from_a_temporary_place(Path::new("/usr/bin/vda-desktop"), false));
        assert!(!runs_from_a_temporary_place(Path::new("/Applications/VDA.app/Contents/MacOS/vda-desktop"), false));
    }

    #[test]
    fn the_kept_copy_follows_the_app() {
        let dir = std::env::temp_dir().join(format!("vda-keep-runner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let from = dir.join("mount").join("vda-run");
        std::fs::create_dir_all(from.parent().unwrap()).unwrap();
        std::fs::write(&from, b"v1").unwrap();
        let kept = dir.join("data").join("bin").join("vda-run");
        assert_eq!(keep_runner(&from, &kept).unwrap(), kept);
        assert_eq!(std::fs::read(&kept).unwrap(), b"v1");
        // An updated app replaces the copy, and leaves no partial file.
        std::fs::write(&from, b"v2").unwrap();
        keep_runner(&from, &kept).unwrap();
        assert_eq!(std::fs::read(&kept).unwrap(), b"v2");
        assert!(!kept.with_extension("part").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&kept).unwrap().permissions().mode() & 0o111, 0o111);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
