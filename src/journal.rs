use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

const KEEP_DAYS: i64 = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Startup,
    Service,
    Task,
    Program,
    DefaultApp,
    Update,
    Restart,
    Crash,
    Threat,
    Disk,
    Slowdown,
    Internet,
    Info,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Startup => "Startup",
            Kind::Service => "Background service",
            Kind::Task => "Scheduled task",
            Kind::Program => "Apps",
            Kind::DefaultApp => "Default apps",
            Kind::Update => "Windows Update",
            Kind::Restart => "Restart",
            Kind::Crash => "Crash",
            Kind::Threat => "Security",
            Kind::Disk => "Storage",
            Kind::Slowdown => "Slowdown",
            Kind::Internet => "Internet",
            Kind::Info => "Plainly",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// ShellExecute "open" on a URI, settings page or file.
    Open { label: String, target: String },
    Run { label: String, exe: String, args: String },
    Reveal { label: String, path: String },
    DisableStartup { label: String, hive: String, approved_key: String, name: String },
    Page { label: String, page: String },
}

impl Action {
    pub fn label(&self) -> &str {
        match self {
            Action::Open { label, .. }
            | Action::Run { label, .. }
            | Action::Reveal { label, .. }
            | Action::DisableStartup { label, .. }
            | Action::Page { label, .. } => label,
        }
    }
    pub fn open(label: &str, target: &str) -> Self {
        Action::Open { label: label.into(), target: target.into() }
    }
    pub fn run(label: &str, exe: &str, args: &str) -> Self {
        Action::Run { label: label.into(), exe: exe.into(), args: args.into() }
    }
    pub fn page(label: &str, page: &str) -> Self {
        Action::Page { label: label.into(), page: page.into() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    #[serde(default)]
    pub id: u64,
    pub time: i64,
    pub kind: Kind,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// The time is a best guess (e.g. a date with no time, or "some time while Plainly was off").
    #[serde(default)]
    pub approx: bool,
    /// Routine Windows housekeeping, hidden unless the user asks to see everything.
    #[serde(default)]
    pub noise: bool,
    #[serde(default)]
    pub actions: Vec<Action>,
}

impl Event {
    pub fn new(kind: Kind, time: i64, title: impl Into<String>) -> Self {
        Event { id: 0, time, kind, title: title.into(), detail: String::new(), approx: false, noise: false, actions: Vec::new() }
    }
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }
    pub fn noise(mut self, n: bool) -> Self {
        self.noise = n;
        self
    }
    pub fn approx(mut self, a: bool) -> Self {
        self.approx = a;
        self
    }
    pub fn action(mut self, a: Action) -> Self {
        self.actions.push(a);
        self
    }
}

type FileSig = (u64, Option<std::time::SystemTime>);

fn file_sig(path: &PathBuf) -> FileSig {
    fs::metadata(path).map(|m| (m.len(), m.modified().ok())).unwrap_or((0, None))
}

/// The window's read-only view of the timeline file, reloaded whenever the background watcher appends to it.
pub struct Journal {
    path: PathBuf,
    events: Vec<Event>,
    next_id: u64,
    sig: FileSig,
}

impl Journal {
    pub fn load(path: PathBuf) -> Self {
        let sig = file_sig(&path);
        let mut events: Vec<Event> = fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let cutoff = now() - KEEP_DAYS * 86_400;
        events.retain(|e| e.time >= cutoff);
        events.sort_by_key(|e| e.time);
        let next_id = events.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        Journal { path, events, next_id, sig }
    }

    pub fn reload_if_changed(&mut self) -> bool {
        if file_sig(&self.path) == self.sig {
            return false;
        }
        *self = Journal::load(self.path.clone());
        true
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn clear(&mut self) {
        self.events.clear();
        let _ = fs::write(&self.path, "");
        self.sig = file_sig(&self.path);
    }

    fn rewrite(&self) {
        let body: String = self
            .events
            .iter()
            .filter_map(|e| serde_json::to_string(e).ok())
            .map(|l| l + "\n")
            .collect();
        let _ = fs::write(&self.path, body);
    }
}

/// The background watcher's append-only handle; keeps no events in memory.
pub struct Writer {
    path: PathBuf,
    next_id: u64,
}

impl Writer {
    /// Also drops events past the retention period.
    pub fn open(path: PathBuf) -> Self {
        let j = Journal::load(path.clone());
        let lines = fs::read_to_string(&path).map(|s| s.lines().count()).unwrap_or(0);
        if lines != j.events.len() {
            j.rewrite();
        }
        Writer { path, next_id: j.next_id }
    }

    pub fn append(&mut self, mut e: Event) {
        e.id = self.next_id;
        self.next_id += 1;
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            if let Ok(line) = serde_json::to_string(&e) {
                let _ = writeln!(f, "{line}");
            }
        }
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("plainly-test-{name}-{}.jsonl", std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn appended_events_load_sorted_and_reload_on_change() {
        let p = temp_path("sorted");
        let mut w = Writer::open(p.clone());
        w.append(Event::new(Kind::Info, now(), "later"));
        let mut j = Journal::load(p.clone());
        assert_eq!(j.events().len(), 1);
        w.append(Event::new(Kind::Info, now() - 100, "earlier"));
        assert!(j.reload_if_changed());
        assert_eq!(j.events()[0].title, "earlier");
        assert_eq!(j.events()[1].title, "later");
        assert!(!j.reload_if_changed());
        let w2 = Writer::open(p.clone());
        assert!(w2.next_id > 2);
        let _ = fs::remove_file(p);
    }

    #[test]
    fn writer_drops_events_older_than_retention() {
        let p = temp_path("trim");
        let mut w = Writer::open(p.clone());
        w.append(Event::new(Kind::Info, now() - (KEEP_DAYS + 5) * 86_400, "ancient"));
        w.append(Event::new(Kind::Info, now(), "fresh"));
        Writer::open(p.clone());
        assert_eq!(fs::read_to_string(&p).unwrap().lines().count(), 1);
        let _ = fs::remove_file(p);
    }
}
