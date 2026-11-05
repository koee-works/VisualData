//! How much of the computer the app is using, for the status bar. It reads
//! the app's own process and the programs it started (Python, R, the web
//! view), never other programs, and it does not touch the open project, so
//! it keeps answering while a long run holds the project.

use serde::Serialize;
use std::collections::HashMap;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// One kind of process and what all of that kind use together.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PartView {
    /// "App" (the window and the DuckDB engine inside it), "Python", "R",
    /// "Web view" or "Other".
    pub name: String,
    /// How many processes of this kind are running.
    pub count: usize,
    /// Percent of the whole computer's processing power.
    pub cpu: f32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResourceView {
    /// Percent of the whole computer's processing power, all parts together.
    pub cpu: f32,
    pub memory_bytes: u64,
    pub parts: Vec<PartView>,
    pub cores: usize,
    /// The computer's memory.
    pub total_memory_bytes: u64,
}

pub struct Monitor {
    /// `None` on a computer sysinfo cannot read; the readout is then empty.
    system: Option<System>,
    me: Pid,
}

impl Default for Monitor {
    fn default() -> Self {
        Monitor { system: System::new().ok(), me: Pid::from_u32(std::process::id()) }
    }
}

/// Which part a process belongs to, from its program's name.
fn part_of(name: &str, is_app: bool) -> &'static str {
    if is_app {
        return "App";
    }
    let n = name.to_ascii_lowercase();
    if n.starts_with("python") {
        "Python"
    } else if n == "r" || n == "r.exe" || n.starts_with("rscript") || n.starts_with("rterm") || n == "exec" {
        "R"
    } else if n.contains("webkit") || n.contains("webview") || n.contains("webcontent") {
        "Web view"
    } else {
        "Other"
    }
}

impl Monitor {
    /// Read the app's usage now. Processor use is measured since the last
    /// call, so the first answer reads 0 and later ones are meaningful.
    pub fn sample(&mut self) -> ResourceView {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let Some(system) = self.system.as_mut() else {
            return ResourceView { cpu: 0.0, memory_bytes: 0, parts: vec![], cores, total_memory_bytes: 0 };
        };
        system.refresh_memory();
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory());

        // Everything that descends from this process.
        let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
        for (pid, p) in system.processes() {
            if let Some(parent) = p.parent() {
                children.entry(parent).or_default().push(*pid);
            }
        }
        let mut mine = vec![self.me];
        let mut i = 0;
        while i < mine.len() {
            if let Some(kids) = children.get(&mine[i]) {
                mine.extend(kids.iter().copied());
            }
            i += 1;
        }

        let mut parts: Vec<PartView> = Vec::new();
        for pid in &mine {
            let Some(p) = system.process(*pid) else { continue };
            let name = part_of(&p.name().to_string_lossy(), *pid == self.me);
            // sysinfo counts one busy core as 100; a share of the computer is
            // that over the number of cores.
            let cpu = p.cpu_usage() / cores as f32;
            match parts.iter_mut().find(|x| x.name == name) {
                Some(x) => {
                    x.count += 1;
                    x.cpu += cpu;
                    x.memory_bytes += p.memory();
                }
                None => parts.push(PartView { name: name.to_string(), count: 1, cpu, memory_bytes: p.memory() }),
            }
        }
        // Biggest memory first, the app itself always on top.
        parts.sort_by(|a, b| (b.name == "App").cmp(&(a.name == "App")).then(b.memory_bytes.cmp(&a.memory_bytes)));
        ResourceView {
            cpu: parts.iter().map(|p| p.cpu).sum::<f32>().min(100.0),
            memory_bytes: parts.iter().map(|p| p.memory_bytes).sum(),
            parts,
            cores,
            total_memory_bytes: system.total_memory(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_programs_by_what_they_are() {
        assert_eq!(part_of("python3.13", false), "Python");
        assert_eq!(part_of("Python", false), "Python");
        assert_eq!(part_of("Rscript", false), "R");
        assert_eq!(part_of("R.exe", false), "R");
        assert_eq!(part_of("WebKitWebProcess", false), "Web view");
        assert_eq!(part_of("msedgewebview2.exe", false), "Web view");
        assert_eq!(part_of("sh", false), "Other");
        assert_eq!(part_of("python3", true), "App");
    }

    #[test]
    fn sees_this_process_and_a_child_it_started() {
        let mut m = Monitor::default();
        let mut child = std::process::Command::new("sleep").arg("5").spawn().expect("sleep starts");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let v = m.sample();
        child.kill().ok();
        child.wait().ok();
        let app = v.parts.iter().find(|p| p.name == "App").expect("the app is listed");
        assert!(app.memory_bytes > 1_000_000, "the test program uses some memory");
        assert_eq!(v.parts[0].name, "App", "the app is listed first");
        assert!(v.parts.iter().any(|p| p.name == "Other" && p.count >= 1), "the child is listed: {:?}", v.parts);
        assert!(v.memory_bytes >= app.memory_bytes);
        assert!(v.cpu >= 0.0 && v.cpu <= 100.0);
        assert!(v.total_memory_bytes >= v.memory_bytes);
    }
}
