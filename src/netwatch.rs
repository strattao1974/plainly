//! Checks the internet every 30 seconds and records drop-outs, with which link failed.

use crate::journal::{now, Action, Event, Kind};
use crate::net;
use crate::shared::{notify_batch, Shared};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

const EVERY_SECS: u64 = 30;
const MIN_OUTAGE_SECS: i64 = 45;
/// A longer gap between checks means the PC was asleep; don't count that as an outage.
const SLEEP_GAP_SECS: i64 = 120;

#[derive(Clone, Copy, Debug)]
pub struct Probe {
    pub online: bool,
    pub adapter_up: bool,
    pub router_ok: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outage {
    pub start: i64,
    pub end: i64,
    pub samples: u32,
    pub adapter_down: u32,
    pub router_down: u32,
}

#[derive(Default)]
pub struct OutageTracker {
    last: Option<i64>,
    first_fail: Option<i64>,
    fails: u32,
    adapter_down: u32,
    router_down: u32,
}

impl OutageTracker {
    pub fn feed(&mut self, now: i64, p: Probe) -> Option<Outage> {
        if let Some(last) = self.last {
            if now - last > SLEEP_GAP_SECS {
                *self = OutageTracker::default();
            }
        }
        self.last = Some(now);
        if !p.online {
            self.first_fail.get_or_insert(now);
            self.fails += 1;
            if !p.adapter_up {
                self.adapter_down += 1;
            } else if !p.router_ok {
                self.router_down += 1;
            }
            return None;
        }
        let out = match self.first_fail {
            Some(start) if self.fails >= 2 && now - start >= MIN_OUTAGE_SECS => Some(Outage {
                start,
                end: now,
                samples: self.fails,
                adapter_down: self.adapter_down,
                router_down: self.router_down,
            }),
            _ => None,
        };
        self.first_fail = None;
        self.fails = 0;
        self.adapter_down = 0;
        self.router_down = 0;
        out
    }
}

fn clock(t: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(t, 0).single().map(|d| d.format("%-I:%M %p").to_string()).unwrap_or_default()
}

impl Outage {
    pub fn to_event(&self) -> Event {
        let dur = crate::pulse::duration_words(self.end - self.start);
        let span = format!("From {} to {}", clock(self.start), clock(self.end));
        let check = Action::page("Check internet now", "internet");
        if self.adapter_down == self.samples {
            return Event::new(Kind::Internet, self.start, format!("Your PC was offline for {dur}"))
                .detail(format!("{span}, the PC had no network connection at all (Wi-Fi off or out of range, or the cable unplugged)."))
                .noise(true);
        }
        if self.adapter_down > 0 {
            return Event::new(Kind::Internet, self.start, format!("Your Wi-Fi dropped out for {dur}"))
                .detail(format!("{span}, the PC lost its connection to the network. Weak signal or interference is the usual cause."))
                .action(check);
        }
        if self.router_down * 2 >= self.samples {
            return Event::new(Kind::Internet, self.start, format!("Your internet dropped out for {dur}"))
                .detail(format!("{span}. Your PC couldn't reach the router, so the problem was the Wi-Fi link or the router itself."))
                .action(check);
        }
        Event::new(Kind::Internet, self.start, format!("Your internet dropped out for {dur}"))
            .detail(format!(
                "{span}. Your router was reachable the whole time, so the problem was beyond it: your internet connection or provider. If this happens often, this timeline is useful evidence for them."
            ))
            .action(check)
    }
}

fn probe() -> Probe {
    let online = net::internet_reachable(Duration::from_secs(3)) || ncsi_reachable();
    if online {
        return Probe { online, adapter_up: true, router_ok: true };
    }
    let Some(adapter) = net::primary(&net::adapters()) else {
        return Probe { online, adapter_up: false, router_ok: false };
    };
    let router_ok = match adapter.gateway {
        Some(IpAddr::V4(v4)) => net::ping(v4, 2, Duration::from_millis(200), 1000).received > 0,
        Some(ip) => net::tcp_alive(ip, &[80, 443, 53], Duration::from_secs(1)),
        None => false,
    } || adapter.gateway.map(|ip| net::tcp_alive(ip, &[80, 443, 53], Duration::from_secs(1))).unwrap_or(false);
    Probe { online, adapter_up: true, router_ok }
}

/// Fallback for networks that block well-known public addresses: Windows' own connectivity host.
fn ncsi_reachable() -> bool {
    ("www.msftconnecttest.com", 80)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .map(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok())
        .unwrap_or(false)
}

pub fn spawn(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("netwatch".into())
        .spawn(move || run(shared))
        .ok();
}

fn run(shared: Arc<Shared>) {
    let mut tracker = OutageTracker::default();
    loop {
        std::thread::sleep(Duration::from_secs(EVERY_SECS));
        let settings = shared.settings();
        if !settings.watch_internet {
            tracker = OutageTracker::default();
            continue;
        }
        if let Some(o) = tracker.feed(now(), probe()) {
            let ev = o.to_event();
            if settings.notify_internet && !ev.noise {
                notify_batch(&[("Your internet is back".into(), format!("{}. {}", ev.title, ev.detail))]);
            }
            shared.add_events(vec![ev]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP: Probe = Probe { online: true, adapter_up: true, router_ok: true };
    const ISP_DOWN: Probe = Probe { online: false, adapter_up: true, router_ok: true };
    const ROUTER_DOWN: Probe = Probe { online: false, adapter_up: true, router_ok: false };

    #[test]
    fn provider_outage_is_blamed_on_provider() {
        let mut t = OutageTracker::default();
        assert!(t.feed(0, UP).is_none());
        for i in 1..=4 {
            assert!(t.feed(i * 30, ISP_DOWN).is_none());
        }
        let o = t.feed(150, UP).unwrap();
        assert_eq!(o.start, 30);
        assert!(o.to_event().detail.contains("internet connection or provider"));
    }

    #[test]
    fn router_outage_is_blamed_on_router() {
        let mut t = OutageTracker::default();
        for i in 1..=3 {
            t.feed(i * 30, ROUTER_DOWN);
        }
        let o = t.feed(120, UP).unwrap();
        assert!(o.to_event().detail.contains("couldn't reach the router"));
    }

    #[test]
    fn single_blip_is_ignored() {
        let mut t = OutageTracker::default();
        t.feed(0, UP);
        t.feed(30, ISP_DOWN);
        assert!(t.feed(60, UP).is_none());
    }

    #[test]
    fn sleep_gap_resets() {
        let mut t = OutageTracker::default();
        t.feed(0, ISP_DOWN);
        t.feed(30, ISP_DOWN);
        assert!(t.feed(5000, UP).is_none());
    }
}
