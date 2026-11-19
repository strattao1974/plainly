use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub welcomed: bool,
    pub notify_startup: bool,
    pub notify_services: bool,
    pub notify_defaults: bool,
    pub notify_installs: bool,
    pub notify_threats: bool,
    pub notify_internet: bool,
    pub notify_slowdowns: bool,
    pub show_noise: bool,
    pub watch_slowdowns: bool,
    pub watch_internet: bool,
    pub last_viewed: i64,
    pub told_about_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            welcomed: false,
            notify_startup: true,
            notify_services: true,
            notify_defaults: true,
            notify_installs: false,
            notify_threats: true,
            notify_internet: false,
            notify_slowdowns: false,
            show_noise: false,
            watch_slowdowns: true,
            watch_internet: true,
            last_viewed: 0,
            told_about_tray: false,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Written to a temp file first so the background watcher never reads a half-written file.
    pub fn save(&self, path: &Path) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }
}
