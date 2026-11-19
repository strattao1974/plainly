//! Once-a-day check for a newer Plainly on GitHub. It only tells people and links to the release
//! page; it never downloads or runs anything by itself.

use crate::journal::now;
use crate::shared::Shared;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub const REPO: &str = "strattao1974/plainly";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const CHECK_EVERY_SECS: i64 = 86_400;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateInfo {
    /// Latest released version, e.g. "0.2.0" (empty if none found).
    pub latest: String,
    pub url: String,
    pub checked: i64,
    /// The version we've already sent a notification about.
    pub notified: String,
}

impl UpdateInfo {
    pub fn available(&self) -> bool {
        is_newer(&self.latest, CURRENT)
    }
}

pub fn load(dir: &Path) -> UpdateInfo {
    std::fs::read_to_string(dir.join("update.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(dir: &Path, info: &UpdateInfo) {
    if let Ok(s) = serde_json::to_string(info) {
        let tmp = dir.join("update.json.tmp");
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(tmp, dir.join("update.json"));
        }
    }
}

pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>());
    let major = parts.next()?.ok()?;
    let minor = parts.next().unwrap_or(Ok(0)).ok()?;
    let patch = parts.next().unwrap_or(Ok(0)).ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
fn parse_release(json: &str) -> Option<(String, String)> {
    parse_release_for(REPO, json)
}

/// Pulls (version, release page) out of GitHub's "latest release" answer.
fn parse_release_for(repo: &str, json: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v.get("draft").and_then(|d| d.as_bool()).unwrap_or(false) || v.get("prerelease").and_then(|d| d.as_bool()).unwrap_or(false) {
        return None;
    }
    let tag = v.get("tag_name")?.as_str()?;
    let (major, minor, patch) = parse_version(tag)?;
    let url = v.get("html_url")?.as_str()?;
    // Only ever send people to this project's own releases.
    if !url.starts_with(&format!("https://github.com/{repo}/releases/")) {
        return None;
    }
    Some((format!("{major}.{minor}.{patch}"), url.to_string()))
}

/// Asks GitHub for the latest release. `Ok(None)` means there isn't one (or the repo is still private).
pub fn fetch_latest() -> Result<Option<(String, String)>, String> {
    fetch_latest_from(REPO)
}

fn fetch_latest_from(repo: &str) -> Result<Option<(String, String)>, String> {
    let r = crate::http::get_body("api.github.com", &format!("/repos/{repo}/releases/latest"), 512 * 1024, Duration::from_secs(10))?;
    match r.status {
        200 => Ok(parse_release_for(repo, &String::from_utf8_lossy(&r.body))),
        404 => Ok(None),
        s => Err(format!("GitHub answered with code {s}")),
    }
}

/// Checks now and records the result. Returns the updated info.
pub fn check_now(dir: &Path) -> Result<UpdateInfo, String> {
    let mut info = load(dir);
    let latest = fetch_latest()?;
    info.checked = now();
    match latest {
        Some((version, url)) => {
            info.latest = version;
            info.url = url;
        }
        None => {
            info.latest.clear();
            info.url.clear();
        }
    }
    save(dir, &info);
    Ok(info)
}

pub fn spawn(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("updates".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(120));
            loop {
                let due = now() - load(&shared.dir).checked >= CHECK_EVERY_SECS;
                if shared.settings().check_updates && due {
                    if let Ok(mut info) = check_now(&shared.dir) {
                        if info.available() && info.notified != info.latest {
                            crate::tray::notify(
                                &format!("Plainly {} is available", info.latest),
                                "Open Plainly to see what's new and download it.",
                            );
                            info.notified = info.latest.clone();
                            save(&shared.dir, &info);
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(3600));
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "calls the real GitHub API; run with --ignored --nocapture"]
    fn live_github() {
        println!("plainly: {:?}", fetch_latest());
        println!("ripgrep: {:?}", fetch_latest_from("BurntSushi/ripgrep"));
    }

    #[test]
    fn version_comparison() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.3"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
        assert_eq!(parse_version("v1.2.3-beta"), Some((1, 2, 3)));
    }

    #[test]
    fn parses_github_release() {
        let json = format!(
            r#"{{"tag_name":"v0.2.0","html_url":"https://github.com/{REPO}/releases/tag/v0.2.0","draft":false,"prerelease":false,"assets":[]}}"#
        );
        assert_eq!(parse_release(&json), Some(("0.2.0".into(), format!("https://github.com/{REPO}/releases/tag/v0.2.0"))));
    }

    #[test]
    fn ignores_prereleases_and_foreign_links() {
        let pre = format!(r#"{{"tag_name":"v0.3.0","html_url":"https://github.com/{REPO}/releases/tag/v0.3.0","prerelease":true}}"#);
        assert_eq!(parse_release(&pre), None);
        let foreign = r#"{"tag_name":"v9.0.0","html_url":"https://evil.example.com/releases/x"}"#;
        assert_eq!(parse_release(foreign), None);
    }
}
