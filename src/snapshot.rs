//! What Plainly remembers about the PC between checks, and how two snapshots become timeline events.

use crate::journal::{Action, Event, Kind};
use crate::winutil::exe_from_command;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupLoc {
    UserRun,
    MachineRun,
    MachineRun32,
    UserFolder,
    MachineFolder,
}

impl StartupLoc {
    pub fn label(self) -> &'static str {
        match self {
            StartupLoc::UserRun => "Your startup list",
            StartupLoc::MachineRun | StartupLoc::MachineRun32 => "The startup list for everyone on this PC",
            StartupLoc::UserFolder => "Your Startup folder",
            StartupLoc::MachineFolder => "The shared Startup folder",
        }
    }
    pub fn hive(self) -> &'static str {
        match self {
            StartupLoc::UserRun | StartupLoc::UserFolder => "HKCU",
            _ => "HKLM",
        }
    }
    pub fn approved_key(self) -> &'static str {
        match self {
            StartupLoc::UserRun | StartupLoc::MachineRun => "Run",
            StartupLoc::MachineRun32 => "Run32",
            StartupLoc::UserFolder | StartupLoc::MachineFolder => "StartupFolder",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartupItem {
    pub name: String,
    pub command: String,
    pub location: StartupLoc,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceItem {
    pub display: String,
    pub image: String,
    pub start: u32,
    pub windows: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub name: String,
    pub version: String,
    pub publisher: String,
    #[serde(default)]
    pub store: bool,
    #[serde(default)]
    pub microsoft: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub startup: BTreeMap<String, StartupItem>,
    pub services: BTreeMap<String, ServiceItem>,
    /// Task path -> enabled.
    pub tasks: BTreeMap<String, bool>,
    pub programs: BTreeMap<String, Program>,
    /// Friendly label ("web browser") -> app name.
    pub defaults: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notify {
    No,
    Startup,
    Service,
    Default,
    Install,
}

pub struct Change {
    pub event: Event,
    pub notify: Notify,
}

fn is_self(name: &str) -> bool {
    name.to_ascii_lowercase().contains("plainly")
}

pub fn start_label(start: u32) -> &'static str {
    match start {
        0 | 1 => "with Windows itself",
        2 => "automatically when Windows starts",
        3 => "only when needed",
        4 => "never (it's disabled)",
        _ => "in an unusual way",
    }
}

pub fn diff(old: &Snapshot, new: &Snapshot, now: i64) -> Vec<Change> {
    let mut out = Vec::new();
    diff_startup(old, new, now, &mut out);
    diff_services(old, new, now, &mut out);
    diff_tasks(old, new, now, &mut out);
    diff_programs(old, new, now, &mut out);
    diff_defaults(old, new, now, &mut out);
    out
}

fn startup_actions(item: &StartupItem) -> Vec<Action> {
    let mut a = Vec::new();
    if item.enabled && item.location.hive() == "HKCU" {
        let value_name = match item.location {
            StartupLoc::UserFolder | StartupLoc::MachineFolder => {
                item.command.rsplit('\\').next().unwrap_or(&item.name).to_string()
            }
            _ => item.name.clone(),
        };
        a.push(Action::DisableStartup {
            label: "Turn it off".into(),
            hive: "HKCU".into(),
            approved_key: item.location.approved_key().into(),
            name: value_name,
        });
    }
    a.push(Action::open("Startup apps…", "ms-settings:startupapps"));
    if let Some(exe) = exe_from_command(&item.command) {
        if exe.contains('\\') {
            a.push(Action::Reveal { label: "Show file".into(), path: exe });
        }
    }
    a
}

fn diff_startup(old: &Snapshot, new: &Snapshot, now: i64, out: &mut Vec<Change>) {
    for (key, item) in &new.startup {
        if is_self(&item.name) {
            continue;
        }
        match old.startup.get(key) {
            None => {
                let (title, notify) = if item.enabled {
                    (format!("{} now starts with Windows", item.name), Notify::Startup)
                } else {
                    (format!("{} was added to startup (but is turned off)", item.name), Notify::No)
                };
                let mut e = Event::new(Kind::Startup, now, title).detail(format!(
                    "Added to {}. Every time you sign in it runs: {}",
                    item.location.label().to_lowercase(),
                    item.command
                ));
                e.actions = startup_actions(item);
                out.push(Change { event: e, notify });
            }
            Some(prev) => {
                if !prev.enabled && item.enabled {
                    let mut e = Event::new(Kind::Startup, now, format!("{} was switched back on at startup", item.name))
                        .detail("It had been turned off. Something turned it on again.");
                    e.actions = startup_actions(item);
                    out.push(Change { event: e, notify: Notify::Startup });
                } else if prev.enabled && !item.enabled {
                    out.push(Change {
                        event: Event::new(Kind::Startup, now, format!("{} was turned off at startup", item.name))
                            .detail("It's still installed, it just won't open by itself when you sign in."),
                        notify: Notify::No,
                    });
                }
                if prev.command != item.command {
                    let mut e = Event::new(Kind::Startup, now, format!("{} changed what it runs at startup", item.name))
                        .detail(format!("Before: {}\nNow: {}", prev.command, item.command));
                    e.actions = startup_actions(item);
                    let notify = if item.enabled { Notify::Startup } else { Notify::No };
                    out.push(Change { event: e, notify });
                }
            }
        }
    }
    for (key, item) in &old.startup {
        if !new.startup.contains_key(key) && !is_self(&item.name) {
            out.push(Change {
                event: Event::new(Kind::Startup, now, format!("{} no longer starts with Windows", item.name))
                    .detail(format!("Removed from {}.", item.location.label().to_lowercase())),
                notify: Notify::No,
            });
        }
    }
}

fn service_action() -> Action {
    Action::run("Open Services", "services.msc", "")
}

fn diff_services(old: &Snapshot, new: &Snapshot, now: i64, out: &mut Vec<Change>) {
    for (key, svc) in &new.services {
        if is_self(&svc.display) {
            continue;
        }
        match old.services.get(key) {
            None => {
                let notify = if !svc.windows && svc.start <= 2 { Notify::Service } else { Notify::No };
                out.push(Change {
                    event: Event::new(Kind::Service, now, format!("New background service: {}", svc.display))
                        .detail(format!(
                            "Services run in the background without a window. This one starts {}. Program: {}",
                            start_label(svc.start),
                            svc.image
                        ))
                        .noise(svc.windows)
                        .action(service_action()),
                    notify,
                });
            }
            Some(prev) => {
                if prev.start != svc.start {
                    let notify = if !svc.windows && svc.start <= 2 && prev.start > 2 { Notify::Service } else { Notify::No };
                    out.push(Change {
                        event: Event::new(Kind::Service, now, format!("{} now starts {}", svc.display, start_label(svc.start)))
                            .detail(format!("Before, it started {}.", start_label(prev.start)))
                            .noise(svc.windows)
                            .action(service_action()),
                        notify,
                    });
                }
                if !prev.image.eq_ignore_ascii_case(&svc.image) {
                    out.push(Change {
                        event: Event::new(Kind::Service, now, format!("{} now runs a different program", svc.display))
                            .detail(format!("Before: {}\nNow: {}", prev.image, svc.image))
                            .noise(true),
                        notify: Notify::No,
                    });
                }
            }
        }
    }
    for (key, svc) in &old.services {
        if !new.services.contains_key(key) && !is_self(&svc.display) {
            out.push(Change {
                event: Event::new(Kind::Service, now, format!("Background service removed: {}", svc.display)).noise(svc.windows),
                notify: Notify::No,
            });
        }
    }
}

pub fn is_windows_task(path: &str) -> bool {
    path.to_ascii_lowercase().starts_with("\\microsoft\\")
}

fn task_name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}

fn diff_tasks(old: &Snapshot, new: &Snapshot, now: i64, out: &mut Vec<Change>) {
    let action = || Action::run("Open Task Scheduler", "taskschd.msc", "");
    for (path, &enabled) in &new.tasks {
        if is_self(path) {
            continue;
        }
        let noise = is_windows_task(path);
        match old.tasks.get(path) {
            None => out.push(Change {
                event: Event::new(Kind::Task, now, format!("New scheduled task: {}", task_name(path)))
                    .detail(format!(
                        "Scheduled tasks run programs automatically at set times or events, even when the app isn't open. Location: {path}{}",
                        if enabled { "" } else { " (currently turned off)" }
                    ))
                    .noise(noise)
                    .action(action()),
                notify: if noise || !enabled { Notify::No } else { Notify::Service },
            }),
            Some(&was) if was != enabled => out.push(Change {
                event: Event::new(
                    Kind::Task,
                    now,
                    format!("Scheduled task {} was turned {}", task_name(path), if enabled { "on" } else { "off" }),
                )
                .detail(format!("Location: {path}"))
                .noise(noise)
                .action(action()),
                notify: Notify::No,
            }),
            _ => {}
        }
    }
    for path in old.tasks.keys() {
        if !new.tasks.contains_key(path) && !is_self(path) {
            out.push(Change {
                event: Event::new(Kind::Task, now, format!("Scheduled task removed: {}", task_name(path)))
                    .detail(format!("Location: {path}"))
                    .noise(is_windows_task(path)),
                notify: Notify::No,
            });
        }
    }
}

fn program_noise(p: &Program) -> bool {
    p.store && p.microsoft
}

fn diff_programs(old: &Snapshot, new: &Snapshot, now: i64, out: &mut Vec<Change>) {
    let apps = || Action::open("Apps settings", "ms-settings:appsfeatures");
    for (key, p) in &new.programs {
        if is_self(&p.name) {
            continue;
        }
        match old.programs.get(key) {
            None => {
                let mut detail = String::new();
                if !p.version.is_empty() {
                    detail.push_str(&format!("Version {}. ", p.version));
                }
                if !p.publisher.is_empty() {
                    detail.push_str(&format!("Made by {}.", p.publisher));
                }
                if p.store {
                    detail.push_str(" From the Microsoft Store.");
                }
                out.push(Change {
                    event: Event::new(Kind::Program, now, format!("Installed: {}", p.name))
                        .detail(detail.trim().to_string())
                        .noise(program_noise(p))
                        .action(apps()),
                    notify: if program_noise(p) { Notify::No } else { Notify::Install },
                });
            }
            Some(prev) if prev.version != p.version && !p.version.is_empty() => out.push(Change {
                event: Event::new(Kind::Program, now, format!("Updated: {}", p.name))
                    .detail(if prev.version.is_empty() {
                        format!("Now version {}.", p.version)
                    } else {
                        format!("From version {} to {}.", prev.version, p.version)
                    })
                    .noise(p.store),
                notify: Notify::No,
            }),
            _ => {}
        }
    }
    for (key, p) in &old.programs {
        if !new.programs.contains_key(key) && !is_self(&p.name) {
            out.push(Change {
                event: Event::new(Kind::Program, now, format!("Uninstalled: {}", p.name)).noise(program_noise(p)),
                notify: Notify::No,
            });
        }
    }
}

fn diff_defaults(old: &Snapshot, new: &Snapshot, now: i64, out: &mut Vec<Change>) {
    for (label, app) in &new.defaults {
        if let Some(prev) = old.defaults.get(label) {
            if prev != app && !prev.is_empty() && !app.is_empty() {
                out.push(Change {
                    event: Event::new(Kind::DefaultApp, now, format!("Your default {label} changed from {prev} to {app}"))
                        .detail("If you didn't choose this, an app or update switched it for you.")
                        .action(Action::open("Change it back…", "ms-settings:defaultapps")),
                    notify: Notify::Default,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn startup(name: &str, enabled: bool) -> StartupItem {
        StartupItem { name: name.into(), command: format!("\"C:\\Apps\\{name}.exe\" --tray"), location: StartupLoc::UserRun, enabled }
    }

    #[test]
    fn new_enabled_startup_item_notifies_and_offers_turn_off() {
        let old = Snapshot::default();
        let mut new = Snapshot::default();
        new.startup.insert("u|Zoom".into(), startup("Zoom", true));
        let changes = diff(&old, &new, 100);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].notify, Notify::Startup);
        assert_eq!(changes[0].event.title, "Zoom now starts with Windows");
        assert!(matches!(changes[0].event.actions[0], Action::DisableStartup { .. }));
    }

    #[test]
    fn turning_off_is_logged_quietly_and_turning_on_notifies() {
        let mut a = Snapshot::default();
        a.startup.insert("k".into(), startup("Spotify", true));
        let mut b = Snapshot::default();
        b.startup.insert("k".into(), startup("Spotify", false));
        let off = diff(&a, &b, 1);
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].notify, Notify::No);
        let on = diff(&b, &a, 2);
        assert_eq!(on[0].notify, Notify::Startup);
    }

    #[test]
    fn plainly_never_reports_itself() {
        let mut new = Snapshot::default();
        new.startup.insert("k".into(), startup("Plainly", true));
        assert!(diff(&Snapshot::default(), &new, 1).is_empty());
    }

    #[test]
    fn windows_services_are_noise_third_party_auto_services_notify() {
        let mut new = Snapshot::default();
        new.services.insert("a".into(), ServiceItem { display: "Acme Updater".into(), image: r"C:\Acme\up.exe".into(), start: 2, windows: false });
        new.services.insert("b".into(), ServiceItem { display: "Win Thing".into(), image: r"C:\Windows\system32\svchost.exe".into(), start: 2, windows: true });
        let c = diff(&Snapshot::default(), &new, 1);
        let acme = c.iter().find(|c| c.event.title.contains("Acme")).unwrap();
        let win = c.iter().find(|c| c.event.title.contains("Win Thing")).unwrap();
        assert_eq!(acme.notify, Notify::Service);
        assert!(!acme.event.noise);
        assert_eq!(win.notify, Notify::No);
        assert!(win.event.noise);
    }

    #[test]
    fn microsoft_tasks_are_noise() {
        let mut new = Snapshot::default();
        new.tasks.insert("\\Microsoft\\Windows\\Foo".into(), true);
        new.tasks.insert("\\GoogleUpdateTaskMachineUA".into(), true);
        let c = diff(&Snapshot::default(), &new, 1);
        let ms = c.iter().find(|c| c.event.title.ends_with("Foo")).unwrap();
        let g = c.iter().find(|c| c.event.title.contains("Google")).unwrap();
        assert!(ms.event.noise);
        assert_eq!(g.notify, Notify::Service);
    }

    #[test]
    fn program_install_update_and_removal() {
        let p = |v: &str| Program { name: "VLC".into(), version: v.into(), publisher: "VideoLAN".into(), store: false, microsoft: false };
        let mut a = Snapshot::default();
        let mut b = Snapshot::default();
        b.programs.insert("VLC".into(), p("3.0"));
        assert_eq!(diff(&a, &b, 1)[0].event.title, "Installed: VLC");
        a.programs.insert("VLC".into(), p("2.0"));
        let up = diff(&a, &b, 1);
        assert_eq!(up[0].event.title, "Updated: VLC");
        assert_eq!(up[0].event.detail, "From version 2.0 to 3.0.");
        assert_eq!(diff(&b, &Snapshot::default(), 1)[0].event.title, "Uninstalled: VLC");
    }

    #[test]
    fn default_app_change_notifies_but_first_sighting_does_not() {
        let mut a = Snapshot::default();
        let mut b = Snapshot::default();
        b.defaults.insert("web browser".into(), "Microsoft Edge".into());
        assert!(diff(&a, &b, 1).is_empty());
        a.defaults.insert("web browser".into(), "Google Chrome".into());
        let c = diff(&a, &b, 1);
        assert_eq!(c[0].event.title, "Your default web browser changed from Google Chrome to Microsoft Edge");
        assert_eq!(c[0].notify, Notify::Default);
    }
}
