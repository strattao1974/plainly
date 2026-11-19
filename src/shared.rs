//! State for the background watcher process (tray + watchers). The window runs in its own process
//! and talks to this one through files in the data folder plus a few messages (see ipc.rs).

use crate::journal::{Event, Writer};
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::SystemTime;

pub struct Shared {
    pub dir: PathBuf,
    writer: Mutex<Writer>,
    settings: Mutex<(Settings, Option<SystemTime>)>,
    pub scan_now: AtomicBool,
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl Shared {
    pub fn load(dir: PathBuf) -> Self {
        let settings_path = dir.join("settings.json");
        Shared {
            writer: Mutex::new(Writer::open(dir.join("timeline.jsonl"))),
            settings: Mutex::new((Settings::load(&settings_path), modified(&settings_path))),
            dir,
            scan_now: AtomicBool::new(false),
        }
    }

    /// The window owns settings.json; pick up its edits without a restart.
    pub fn settings(&self) -> Settings {
        let path = self.dir.join("settings.json");
        let mut s = self.settings.lock().unwrap();
        let m = modified(&path);
        if m != s.1 {
            *s = (Settings::load(&path), m);
        }
        s.0.clone()
    }

    pub fn add_events(&self, events: Vec<Event>) {
        let mut w = self.writer.lock().unwrap();
        for e in events {
            w.append(e);
        }
    }

    pub fn write_status(&self, status: &Status) {
        if let Ok(s) = serde_json::to_string(status) {
            let _ = std::fs::write(self.dir.join("status.json"), s);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub watching_since: i64,
    pub last_scan: i64,
}

pub fn read_status(dir: &Path) -> Status {
    std::fs::read_to_string(dir.join("status.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Shows one notification for a batch, so five changes don't produce five pop-ups.
pub fn notify_batch(items: &[(String, String)]) {
    match items.len() {
        0 => {}
        1 => crate::tray::notify(&items[0].0, &truncate(&items[0].1, 200)),
        n => {
            let mut body: Vec<String> = items.iter().take(3).map(|i| i.0.clone()).collect();
            if n > 3 {
                body.push(format!("…and {} more", n - 3));
            }
            crate::tray::notify(&format!("{n} changes on your PC"), &body.join("\n"));
        }
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}
