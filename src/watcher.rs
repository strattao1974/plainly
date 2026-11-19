//! Background thread that notices changes to the PC and writes them to the timeline.

use crate::collect;
use crate::eventlog;
use crate::journal::{now, Action, Event, Kind};
use crate::shared::{notify_batch, Shared, Status};
use crate::snapshot::{self, Notify, Snapshot};
use crate::winutil::fmt_bytes;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const SCAN_EVERY_SECS: u64 = 180;
const BACKFILL_DAYS: u32 = 14;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    snapshot: Option<Snapshot>,
    last_scan: i64,
    watching_since: i64,
    bookmarks: BTreeMap<String, u64>,
    disk_baseline: BTreeMap<String, (i64, u64)>,
    last_full_warning: BTreeMap<String, i64>,
    last_disk_check: i64,
}

pub fn spawn(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("watcher".into())
        .spawn(move || run(shared))
        .ok();
}

fn run(shared: Arc<Shared>) {
    let path = shared.dir.join("state.json");
    let mut st: State = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut first_run = st.snapshot.is_none();
    if first_run {
        st.watching_since = now();
    }

    loop {
        let t = now();
        scan_changes(&shared, &mut st, t, first_run);
        scan_event_logs(&shared, &mut st, first_run);
        if t - st.last_disk_check >= 3600 {
            scan_disks(&shared, &mut st, t);
            st.last_disk_check = t;
        }
        st.last_scan = t;
        if let Ok(s) = serde_json::to_string(&st) {
            let _ = std::fs::write(&path, s);
        }
        shared.write_status(&Status { watching_since: st.watching_since, last_scan: t });
        first_run = false;

        for _ in 0..SCAN_EVERY_SECS {
            if shared.scan_now.swap(false, Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn scan_changes(shared: &Shared, st: &mut State, t: i64, first_run: bool) {
    let new = collect::snapshot();
    let mut events = Vec::new();
    let mut to_notify = Vec::new();
    if let Some(old) = &st.snapshot {
        let settings = shared.settings();
        let gap = if st.last_scan > 0 { t - st.last_scan } else { 0 };
        for mut change in snapshot::diff(old, &new, t) {
            if gap > 15 * 60 {
                change.event.approx = true;
                change.event.detail = format!(
                    "{}\nSpotted when Plainly started; it happened some time after {}.",
                    change.event.detail,
                    fmt_time(st.last_scan)
                )
                .trim()
                .to_string();
            }
            let wanted = match change.notify {
                Notify::No => false,
                Notify::Startup => settings.notify_startup,
                Notify::Service => settings.notify_services,
                Notify::Default => settings.notify_defaults,
                Notify::Install => settings.notify_installs,
            };
            if wanted && !change.event.noise {
                to_notify.push((change.event.title.clone(), change.event.detail.clone()));
            }
            events.push(change.event);
        }
    } else if first_run {
        events.push(
            Event::new(Kind::Info, t, "Plainly started keeping watch")
                .detail("From now on, new startup apps, background services, scheduled tasks, installs, changed default apps, updates, restarts, crashes, slowdowns and internet drop-outs will show up here. Anything older comes from Windows' own records."),
        );
        for (name, version, publisher, when) in collect::recent_installs(BACKFILL_DAYS as i64) {
            if name.to_ascii_lowercase().contains("plainly") {
                continue;
            }
            let mut detail = String::new();
            if !version.is_empty() {
                detail.push_str(&format!("Version {version}. "));
            }
            if !publisher.is_empty() {
                detail.push_str(&format!("Made by {publisher}."));
            }
            events.push(
                Event::new(Kind::Program, when, format!("Installed: {name}"))
                    .detail(detail.trim().to_string())
                    .approx(true)
                    .action(Action::open("Apps settings", "ms-settings:appsfeatures")),
            );
        }
    }
    st.snapshot = Some(new);
    shared.add_events(events);
    notify_batch(&to_notify);
}

fn scan_event_logs(shared: &Shared, st: &mut State, first_run: bool) {
    let mut all = Vec::new();
    for src in eventlog::SOURCES {
        let bookmark = st.bookmarks.get(src.channel).copied();
        let raw = match bookmark {
            Some(b) => eventlog::query_since(src.channel, src.filter, Some(b), BACKFILL_DAYS),
            None if first_run => eventlog::query_since(src.channel, src.filter, None, BACKFILL_DAYS),
            None => Vec::new(),
        };
        let newest = raw.iter().map(|e| e.record_id).max();
        match (newest, bookmark) {
            (Some(n), _) => {
                st.bookmarks.insert(src.channel.to_string(), n);
            }
            (None, None) => {
                if let Some(latest) = eventlog::latest_record_id(src.channel) {
                    st.bookmarks.insert(src.channel.to_string(), latest);
                }
            }
            _ => {}
        }
        all.extend(raw);
    }
    let events = eventlog::to_events(&all);
    if !first_run && shared.settings().notify_threats {
        let threats: Vec<(String, String)> = events
            .iter()
            .filter(|e| e.kind == Kind::Threat)
            .map(|e| (e.title.clone(), e.detail.clone()))
            .collect();
        notify_batch(&threats);
    }
    shared.add_events(events);
}

fn scan_disks(shared: &Shared, st: &mut State, t: i64) {
    let mut events = Vec::new();
    for d in collect::drives() {
        let storage = || Action::open("Storage settings", "ms-settings:storagesense");
        match st.disk_baseline.get(&d.root).copied() {
            None => {
                st.disk_baseline.insert(d.root.clone(), (t, d.free));
            }
            Some((then, free_then)) if t - then >= 20 * 3600 => {
                let drop = free_then.saturating_sub(d.free);
                let threshold = (5_000_000_000u64).max(d.total / 20);
                if drop >= threshold && t - then <= 3 * 86_400 {
                    events.push(
                        Event::new(Kind::Disk, t, format!("Drive {} lost {} of free space since {}", d.root, fmt_bytes(drop), day_word(then)))
                            .detail(format!(
                                "{} is now free out of {}. Big drops usually come from updates, downloads, games, or apps keeping large caches.",
                                fmt_bytes(d.free),
                                fmt_bytes(d.total)
                            ))
                            .action(storage()),
                    );
                }
                st.disk_baseline.insert(d.root.clone(), (t, d.free));
            }
            _ => {}
        }
        let pct = d.free as f64 / d.total as f64 * 100.0;
        let last_warn = st.last_full_warning.get(&d.root).copied().unwrap_or(0);
        if pct < 8.0 && t - last_warn > 3 * 86_400 {
            events.push(
                Event::new(Kind::Disk, t, format!("Drive {} is nearly full", d.root))
                    .detail(format!(
                        "Only {} left ({pct:.0}% free). Windows needs free space for updates and runs slower without it.",
                        fmt_bytes(d.free)
                    ))
                    .action(storage()),
            );
            st.last_full_warning.insert(d.root.clone(), t);
        }
    }
    shared.add_events(events);
}

fn fmt_time(t: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(t, 0)
        .single()
        .map(|d| d.format("%-I:%M %p on %a %-d %b").to_string())
        .unwrap_or_else(|| "the last check".into())
}

fn day_word(t: i64) -> String {
    use chrono::TimeZone;
    let Some(then) = chrono::Local.timestamp_opt(t, 0).single() else { return "the last check".into() };
    let today = chrono::Local::now().date_naive();
    let d = then.date_naive();
    if d == today {
        "earlier today".into()
    } else if Some(d) == today.pred_opt() {
        "yesterday".into()
    } else {
        then.format("%A").to_string()
    }
}
