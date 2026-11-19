//! Reads Windows event logs and turns the handful of events people care about into timeline entries.

use crate::journal::{Action, Event, Kind};
use crate::winutil::wide;
use std::collections::BTreeMap;
use windows::core::PCWSTR;
use windows::Win32::System::EventLog::{
    EvtClose, EvtNext, EvtQuery, EvtQueryChannelPath, EvtQueryForwardDirection, EvtQueryReverseDirection, EvtRender,
    EvtRenderEventXml, EVT_HANDLE,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawEvent {
    pub record_id: u64,
    pub time: i64,
    pub provider: String,
    pub event_id: u32,
    pub data: Vec<(String, String)>,
}

impl RawEvent {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.data.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn nth(&self, i: usize) -> Option<&str> {
        self.data.get(i).map(|(_, v)| v.as_str())
    }
}

pub struct Source {
    pub channel: &'static str,
    pub filter: &'static str,
}

pub const SOURCES: &[Source] = &[
    Source {
        channel: "System",
        filter: "(Provider[@Name='Microsoft-Windows-WindowsUpdateClient'] and (EventID=19 or EventID=20)) or (Provider[@Name='User32'] and EventID=1074) or (Provider[@Name='Microsoft-Windows-Kernel-Power'] and EventID=41)",
    },
    Source {
        channel: "Application",
        filter: "(Provider[@Name='Application Error'] and EventID=1000) or (Provider[@Name='Application Hang'] and EventID=1002)",
    },
    Source { channel: "Microsoft-Windows-Windows Defender/Operational", filter: "EventID=1116" },
];

pub fn query_since(channel: &str, filter: &str, after_record: Option<u64>, max_age_days: u32) -> Vec<RawEvent> {
    let cond = match after_record {
        Some(r) => format!("({filter}) and EventRecordID > {r}"),
        None => format!("({filter}) and TimeCreated[timediff(@SystemTime) <= {}]", max_age_days as u64 * 86_400_000),
    };
    run_query(channel, &format!("*[System[{cond}]]"), false, 2000)
}

pub fn latest_record_id(channel: &str) -> Option<u64> {
    run_query(channel, "*", true, 1).first().map(|e| e.record_id)
}

fn run_query(channel: &str, xpath: &str, newest_first: bool, max: usize) -> Vec<RawEvent> {
    let mut out = Vec::new();
    let c = wide(channel);
    let q = wide(xpath);
    let dir = if newest_first { EvtQueryReverseDirection.0 } else { EvtQueryForwardDirection.0 };
    unsafe {
        let Ok(results) = EvtQuery(None, PCWSTR(c.as_ptr()), PCWSTR(q.as_ptr()), EvtQueryChannelPath.0 | dir) else {
            return out;
        };
        'outer: loop {
            let mut handles = [0isize; 64];
            let mut returned = 0u32;
            if EvtNext(results, &mut handles, 2000, 0, &mut returned).is_err() || returned == 0 {
                break;
            }
            for &h in &handles[..returned as usize] {
                let ev = EVT_HANDLE(h);
                if let Some(xml) = render_xml(ev) {
                    if let Some(raw) = parse_event_xml(&xml) {
                        out.push(raw);
                    }
                }
                let _ = EvtClose(ev);
                if out.len() >= max {
                    break 'outer;
                }
            }
        }
        let _ = EvtClose(results);
    }
    out
}

unsafe fn render_xml(ev: EVT_HANDLE) -> Option<String> {
    let mut used = 0u32;
    let mut count = 0u32;
    let _ = EvtRender(None, ev, EvtRenderEventXml.0, 0, None, &mut used, &mut count);
    if used == 0 {
        return None;
    }
    let mut buf = vec![0u16; (used as usize / 2) + 1];
    EvtRender(
        None,
        ev,
        EvtRenderEventXml.0,
        (buf.len() * 2) as u32,
        Some(buf.as_mut_ptr() as *mut _),
        &mut used,
        &mut count,
    )
    .ok()?;
    Some(crate::winutil::from_wide(&buf))
}

fn attr(xml: &str, tag: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag} "))?;
    let rest = &xml[start..];
    let end = rest.find('>')?;
    let head = &rest[..end];
    let key = format!("{name}=");
    let i = head.find(&key)? + key.len();
    let quote = head[i..].chars().next()?;
    let val = &head[i + 1..];
    Some(val[..val.find(quote)?].to_string())
}

fn element(xml: &str, tag: &str) -> Option<String> {
    let open = xml.find(&format!("<{tag}"))?;
    let rest = &xml[open..];
    let gt = rest.find('>')?;
    if rest[..gt].ends_with('/') {
        return Some(String::new());
    }
    let body = &rest[gt + 1..];
    Some(unescape(&body[..body.find(&format!("</{tag}>"))?]))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

pub fn parse_event_xml(xml: &str) -> Option<RawEvent> {
    let event_id = element(xml, "EventID")?.trim().parse().ok()?;
    let record_id = element(xml, "EventRecordID")?.trim().parse().ok()?;
    let provider = attr(xml, "Provider", "Name").unwrap_or_default();
    let time = attr(xml, "TimeCreated", "SystemTime")
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
        .map(|t| t.timestamp())?;

    let mut data = Vec::new();
    if let Some(ed) = xml.find("<EventData>") {
        let mut rest = &xml[ed..];
        while let Some(i) = rest.find("<Data") {
            rest = &rest[i + 5..];
            let Some(gt) = rest.find('>') else { break };
            let head = &rest[..gt];
            let name = head
                .find("Name=")
                .and_then(|n| {
                    let q = head[n + 5..].chars().next()?;
                    let v = &head[n + 6..];
                    Some(v[..v.find(q)?].to_string())
                })
                .unwrap_or_default();
            if head.ends_with('/') {
                data.push((name, String::new()));
                rest = &rest[gt + 1..];
                continue;
            }
            let body = &rest[gt + 1..];
            let Some(close) = body.find("</Data>") else { break };
            data.push((name, unescape(&body[..close])));
            rest = &body[close..];
        }
    }
    Some(RawEvent { record_id, time, provider, event_id, data })
}

fn file_name(path: &str) -> String {
    let p = path.split(" (").next().unwrap_or(path);
    p.rsplit(['\\', '/']).next().unwrap_or(p).to_string()
}

fn local_day(t: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(t, 0).single().map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

/// Store app updates show up as Windows Update events with titles like "9NBLGGH4NNS1-Microsoft.DesktopAppInstaller".
pub fn is_store_update(title: &str) -> bool {
    let id = title.split('-').next().unwrap_or("");
    id.len() == 12 && id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Turns raw events into timeline entries. Crashes and updates are grouped per app/day so the timeline stays readable.
pub fn to_events(raw: &[RawEvent]) -> Vec<Event> {
    let mut out = Vec::new();
    let mut updates: BTreeMap<String, Vec<&RawEvent>> = BTreeMap::new();
    let mut failed: BTreeMap<String, Vec<&RawEvent>> = BTreeMap::new();
    let mut crashes: BTreeMap<(String, String, bool), Vec<&RawEvent>> = BTreeMap::new();

    for e in raw {
        match (e.provider.as_str(), e.event_id) {
            ("Microsoft-Windows-WindowsUpdateClient", 19) => updates.entry(local_day(e.time)).or_default().push(e),
            ("Microsoft-Windows-WindowsUpdateClient", 20) => failed.entry(local_day(e.time)).or_default().push(e),
            ("User32", 1074) => out.push(restart_event(e)),
            ("Microsoft-Windows-Kernel-Power", 41) => out.push(power_loss_event(e)),
            ("Application Error", 1000) | ("Application Hang", 1002) => {
                let app = e.nth(0).unwrap_or("An app").to_string();
                crashes.entry((local_day(e.time), app, e.event_id == 1002)).or_default().push(e);
            }
            (_, 1116) => out.push(threat_event(e)),
            _ => {}
        }
    }

    for (_, list) in updates {
        let titles: Vec<String> = list.iter().filter_map(|e| e.get("updateTitle").map(str::to_string)).collect();
        let (store, windows): (Vec<_>, Vec<_>) = titles.into_iter().partition(|t| is_store_update(t));
        let time = list.iter().map(|e| e.time).max().unwrap_or(0);
        if !windows.is_empty() {
            let title = if windows.len() == 1 {
                "Windows Update installed an update".to_string()
            } else {
                format!("Windows Update installed {} updates", windows.len())
            };
            out.push(
                Event::new(Kind::Update, time, title)
                    .detail(windows.join("\n"))
                    .action(Action::open("Update history", "ms-settings:windowsupdate-history")),
            );
        }
        if !store.is_empty() {
            out.push(
                Event::new(Kind::Update, time, format!("Microsoft Store updated {} app(s)", store.len()))
                    .detail(store.join("\n"))
                    .noise(true),
            );
        }
    }
    for (_, list) in failed {
        let titles: Vec<String> = list.iter().filter_map(|e| e.get("updateTitle").map(str::to_string)).collect();
        let time = list.iter().map(|e| e.time).max().unwrap_or(0);
        out.push(
            Event::new(Kind::Update, time, "An update failed to install")
                .detail(format!("{}\nWindows usually retries by itself. If it keeps failing, open Windows Update and try again.", titles.join("\n")))
                .action(Action::open("Windows Update", "ms-settings:windowsupdate")),
        );
    }
    for ((_, app, hang), list) in crashes {
        let time = list.iter().map(|e| e.time).max().unwrap_or(0);
        let lower = app.to_ascii_lowercase();
        let name = match crate::kb::process(&lower) {
            Some(k) => k.name,
            None => app.trim_end_matches(".exe").trim_end_matches(".EXE"),
        };
        let n = list.len();
        let title = match (hang, n) {
            (false, 1) => format!("{name} crashed"),
            (false, _) => format!("{name} crashed {n} times"),
            (true, 1) => format!("{name} stopped responding"),
            (true, _) => format!("{name} stopped responding {n} times"),
        };
        out.push(
            Event::new(Kind::Crash, time, title)
                .detail(if hang {
                    "It froze and had to be closed."
                } else {
                    "It closed unexpectedly. If this keeps happening, updating or reinstalling the app usually helps."
                })
                .action(Action::run("Reliability history", "perfmon.exe", "/rel")),
        );
    }
    out
}

const UPDATE_RESTARTERS: &[&str] = &["trustedinstaller", "mousocoreworker", "usoclient", "wuauclt", "musnotification", "usosvc", "tiworker"];
const USER_RESTARTERS: &[&str] = &["explorer.exe", "runtimebroker", "startmenuexperiencehost", "winlogon", "shellexperiencehost", "searchhost"];

fn restart_event(e: &RawEvent) -> Event {
    let process = e.get("param1").or(e.nth(0)).unwrap_or("").to_string();
    let reason = e.get("param3").or(e.nth(2)).unwrap_or("").to_string();
    let kind = e.get("param5").or(e.nth(4)).unwrap_or("restart").to_ascii_lowercase();
    let lower = process.to_ascii_lowercase();
    let restart = kind.contains("restart") || kind.contains("neustart") || kind.contains("redémarr");
    let verb = if restart { "restarted" } else { "shut down" };
    let by_update = UPDATE_RESTARTERS.iter().any(|p| lower.contains(p));
    let by_user = USER_RESTARTERS.iter().any(|p| lower.contains(p));
    let title = if by_update {
        format!("Windows Update {verb} your PC")
    } else if by_user {
        format!("You {verb} the PC")
    } else {
        format!("Your PC was {verb} by {}", file_name(&process))
    };
    let mut detail = String::new();
    if !by_user && !process.is_empty() {
        detail.push_str(&format!("Requested by {}. ", file_name(&process)));
    }
    if !reason.is_empty() && !reason.eq_ignore_ascii_case("No title for this reason could be found") {
        detail.push_str(&format!("Reason given: {reason}."));
    }
    Event::new(Kind::Restart, e.time, title).detail(detail.trim().to_string()).noise(by_user)
}

fn power_loss_event(e: &RawEvent) -> Event {
    let code = e
        .get("BugcheckCode")
        .and_then(|c| c.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if code != 0 {
        Event::new(Kind::Crash, e.time, "Your PC crashed (blue screen)")
            .detail(format!(
                "Windows hit a serious error and had to restart. Stop code 0x{code:X}. One-off crashes happen; repeated ones usually point to a driver or failing hardware."
            ))
            .action(Action::run("Reliability history", "perfmon.exe", "/rel"))
    } else {
        Event::new(Kind::Crash, e.time, "Your PC turned off without shutting down properly")
            .detail("This happens after a power cut, a freeze, or holding the power button. Unsaved work may have been lost.")
            .action(Action::run("Reliability history", "perfmon.exe", "/rel"))
    }
}

fn threat_event(e: &RawEvent) -> Event {
    let name = e.get("Threat Name").unwrap_or("a threat").to_string();
    let path = e.get("Path").unwrap_or("").to_string();
    let path = path.trim_start_matches("file:_").trim_start_matches("containerfile:_").to_string();
    let mut detail = format!("Microsoft Defender spotted {name}.");
    if !path.is_empty() {
        detail.push_str(&format!(" Found in: {path}"));
    }
    Event::new(Kind::Threat, e.time, "Microsoft Defender found a threat")
        .detail(detail)
        .action(Action::open("Open Windows Security", "windowsdefender://threat"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESTART_XML: &str = "<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='User32' Guid='{b0aa8734-56f7-41cc-b2f4-de228e98b946}' EventSourceName='User32'/><EventID Qualifiers='32768'>1074</EventID><Version>0</Version><TimeCreated SystemTime='2026-11-19T15:04:05.1234567Z'/><EventRecordID>98765</EventRecordID></System><EventData><Data Name='param1'>C:\\WINDOWS\\system32\\MoUsoCoreWorker.exe (DESKTOP-1)</Data><Data Name='param2'>DESKTOP-1</Data><Data Name='param3'>Operating System: Service pack (Planned)</Data><Data Name='param4'>0x80020010</Data><Data Name='param5'>restart</Data><Data Name='param6'></Data><Data Name='param7'>NT AUTHORITY\\SYSTEM</Data></EventData></Event>";

    #[test]
    fn parses_event_xml() {
        let e = parse_event_xml(RESTART_XML).unwrap();
        assert_eq!(e.event_id, 1074);
        assert_eq!(e.record_id, 98765);
        assert_eq!(e.provider, "User32");
        assert_eq!(e.get("param5"), Some("restart"));
        assert_eq!(e.time, 1_795_100_645);
    }

    #[test]
    fn windows_update_restart_is_named_plainly() {
        let e = parse_event_xml(RESTART_XML).unwrap();
        let out = to_events(&[e]);
        assert_eq!(out[0].title, "Windows Update restarted your PC");
        assert!(!out[0].noise);
    }

    #[test]
    fn crashes_are_grouped_per_app_and_day() {
        let mk = |t| RawEvent {
            record_id: 1,
            time: t,
            provider: "Application Error".into(),
            event_id: 1000,
            data: vec![(String::new(), "chrome.exe".into())],
        };
        let out = to_events(&[mk(1_795_600_000), mk(1_795_600_100), mk(1_795_600_200)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Google Chrome crashed 3 times");
    }

    #[test]
    fn store_updates_are_separated_from_windows_updates() {
        assert!(is_store_update("9NBLGGH4NNS1-Microsoft.DesktopAppInstaller"));
        assert!(!is_store_update("2026-11 Cumulative Update for Windows 11 (KB5050000)"));
        let mk = |title: &str| RawEvent {
            record_id: 1,
            time: 1_795_600_000,
            provider: "Microsoft-Windows-WindowsUpdateClient".into(),
            event_id: 19,
            data: vec![("updateTitle".into(), title.into())],
        };
        let out = to_events(&[mk("Security Intelligence Update (KB2267602)"), mk("9WZDNCRFJ3PT-Microsoft.ZuneMusic")]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|e| e.title == "Windows Update installed an update" && !e.noise));
        assert!(out.iter().any(|e| e.noise));
    }

    #[test]
    fn empty_and_escaped_data_elements() {
        let xml = "<Event><System><Provider Name='X'/><EventID>1116</EventID><TimeCreated SystemTime='2026-01-01T00:00:00Z'/><EventRecordID>5</EventRecordID></System><EventData><Data Name='Threat Name'>Trojan:Win32/Foo&amp;Bar</Data><Data Name='Empty'/><Data Name='Path'>file:_C:\\x.exe</Data></EventData></Event>";
        let e = parse_event_xml(xml).unwrap();
        assert_eq!(e.get("Threat Name"), Some("Trojan:Win32/Foo&Bar"));
        assert_eq!(e.get("Empty"), Some(""));
        let ev = threat_event(&e);
        assert!(ev.detail.contains("C:\\x.exe"));
    }
}
