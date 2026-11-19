//! Watches overall load every few seconds and records "your PC was struggling" episodes with their cause.

use crate::journal::{now, Action, Event, Kind};
use crate::perf::{Sampler, Totals};
use crate::shared::{notify_batch, Shared};
use crate::slow::{self, Group, Resource};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const START_AFTER_SECS: i64 = 15;
const END_AFTER_SECS: i64 = 20;
const MIN_EPISODE_SECS: i64 = 60;

pub fn busy_resource(t: &Totals) -> Option<Resource> {
    let candidates = [
        (t.cpu / 85.0, Resource::Cpu),
        (t.mem_pct / 92.0, Resource::Memory),
        (t.disk.unwrap_or(0.0) / 90.0, Resource::Disk),
    ];
    let (ratio, res) = candidates.into_iter().fold((0.0f32, Resource::Cpu), |a, b| if b.0 > a.0 { b } else { a });
    (ratio >= 1.0).then_some(res)
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub why: String,
    pub cpu_secs: f64,
    pub mem_peak: u64,
    pub io_bytes: f64,
}

#[derive(Clone, Debug)]
pub struct Episode {
    pub start: i64,
    pub end: i64,
    pub peak_cpu: f32,
    pub peak_mem: f32,
    pub peak_disk: f32,
    pub counts: [u32; 3],
    pub usage: HashMap<String, Usage>,
}

#[derive(Default)]
pub struct EpisodeTracker {
    busy_since: Option<i64>,
    calm_since: Option<i64>,
    active: Option<Episode>,
}

impl EpisodeTracker {
    pub fn feed(&mut self, now: i64, t: &Totals, groups: &[Group], dt: f64) -> Option<Episode> {
        match busy_resource(t) {
            Some(res) => {
                self.calm_since = None;
                let since = *self.busy_since.get_or_insert(now);
                if self.active.is_none() && now - since >= START_AFTER_SECS {
                    self.active = Some(Episode {
                        start: since,
                        end: now,
                        peak_cpu: 0.0,
                        peak_mem: 0.0,
                        peak_disk: 0.0,
                        counts: [0; 3],
                        usage: HashMap::new(),
                    });
                }
                if let Some(ep) = self.active.as_mut() {
                    ep.end = now;
                    ep.peak_cpu = ep.peak_cpu.max(t.cpu);
                    ep.peak_mem = ep.peak_mem.max(t.mem_pct);
                    ep.peak_disk = ep.peak_disk.max(t.disk.unwrap_or(0.0));
                    ep.counts[res as usize] += 1;
                    for g in groups {
                        let u = ep.usage.entry(g.label.clone()).or_insert_with(|| Usage { why: g.why.clone(), ..Default::default() });
                        u.cpu_secs += g.cpu as f64 / 100.0 * dt;
                        u.mem_peak = u.mem_peak.max(g.mem);
                        u.io_bytes += g.io * dt;
                    }
                }
                None
            }
            None => {
                self.busy_since = None;
                if self.active.is_some() {
                    let calm = *self.calm_since.get_or_insert(now);
                    if now - calm >= END_AFTER_SECS {
                        self.calm_since = None;
                        let ep = self.active.take()?;
                        if ep.end - ep.start >= MIN_EPISODE_SECS {
                            return Some(ep);
                        }
                    }
                }
                None
            }
        }
    }

    pub fn is_tracking(&self) -> bool {
        self.busy_since.is_some() || self.active.is_some()
    }
}

pub fn duration_words(secs: i64) -> String {
    match secs {
        s if s < 90 => "about a minute".into(),
        s if s < 90 * 60 => format!("{} minutes", (s + 30) / 60),
        s => {
            let h = s as f64 / 3600.0;
            if h < 1.75 { "over an hour".into() } else { format!("about {:.0} hours", h) }
        }
    }
}

fn clock(t: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(t, 0).single().map(|d| d.format("%-I:%M %p").to_string()).unwrap_or_default()
}

impl Episode {
    pub fn main_resource(&self) -> Resource {
        let i = (0..3).max_by_key(|&i| self.counts[i]).unwrap_or(0);
        [Resource::Cpu, Resource::Memory, Resource::Disk][i]
    }

    pub fn to_event(&self) -> Event {
        let res = self.main_resource();
        let what = match res {
            Resource::Cpu => format!("the processor was flat out (peak {:.0}%)", self.peak_cpu),
            Resource::Memory => format!("memory was nearly full (peak {:.0}%)", self.peak_mem),
            Resource::Disk => format!("the disk was flat out (peak {:.0}%)", self.peak_disk),
        };
        let top = self.usage.iter().max_by(|a, b| {
            let score = |u: &Usage| match res {
                Resource::Cpu => u.cpu_secs,
                Resource::Memory => u.mem_peak as f64,
                Resource::Disk => u.io_bytes,
            };
            score(a.1).total_cmp(&score(b.1))
        });
        let mut detail = format!("From {} to {}, {what}.", clock(self.start), clock(self.end));
        if let Some((label, u)) = top {
            detail.push_str(&format!(" Main cause: {label}. {}", u.why));
        }
        Event::new(Kind::Slowdown, self.start, format!("Your PC was struggling for {}", duration_words(self.end - self.start)))
            .detail(detail)
            .action(Action::page("Why is it slow?", "slow"))
    }
}

pub fn spawn(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("pulse".into())
        .spawn(move || run(shared))
        .ok();
}

fn run(shared: Arc<Shared>) {
    let mut sampler = Sampler::new();
    let mut tracker = EpisodeTracker::default();
    let mut services: (Option<Instant>, HashMap<u32, Vec<String>>) = (None, HashMap::new());
    let mut last = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let dt = last.elapsed().as_secs_f64();
        last = Instant::now();
        let sample = sampler.sample(true);
        if !shared.settings().watch_slowdowns {
            continue;
        }
        let groups = if busy_resource(&sample.totals).is_some() && tracker.is_tracking() {
            if services.0.map(|t| t.elapsed() > Duration::from_secs(60)).unwrap_or(true) {
                services = (Some(Instant::now()), slow::services_by_pid());
            }
            slow::group(&sample.procs, &services.1)
        } else {
            Vec::new()
        };
        if let Some(ep) = tracker.feed(now(), &sample.totals, &groups, dt) {
            let ev = ep.to_event();
            if shared.settings().notify_slowdowns {
                notify_batch(&[(ev.title.clone(), ev.detail.clone())]);
            }
            shared.add_events(vec![ev]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(cpu: f32) -> Totals {
        Totals { cpu, mem_pct: 40.0, mem_used: 0, mem_total: 0, disk: Some(5.0) }
    }

    fn group(label: &str, cpu: f32) -> Group {
        Group { label: label.into(), why: "Because.".into(), tip: None, pids: vec![], cpu, mem: 0, io: 0.0, count: 1, can_end: false }
    }

    #[test]
    fn records_a_sustained_busy_spell_with_its_cause() {
        let mut t = EpisodeTracker::default();
        let mut out = None;
        let mut now = 1000;
        for _ in 0..30 {
            assert!(t.feed(now, &totals(97.0), &[group("Windows Update (installing)", 80.0), group("Chrome", 10.0)], 5.0).is_none());
            now += 5;
        }
        for _ in 0..6 {
            if let Some(e) = t.feed(now, &totals(10.0), &[], 5.0) {
                out = Some(e);
            }
            now += 5;
        }
        let ep = out.expect("episode should be recorded");
        let ev = ep.to_event();
        assert!(ev.title.starts_with("Your PC was struggling for"));
        assert!(ev.detail.contains("Main cause: Windows Update (installing)"));
        assert!(ev.detail.contains("peak 97%"));
    }

    #[test]
    fn short_spikes_are_ignored() {
        let mut t = EpisodeTracker::default();
        let mut now = 0;
        for _ in 0..3 {
            t.feed(now, &totals(99.0), &[], 5.0);
            now += 5;
        }
        for _ in 0..10 {
            assert!(t.feed(now, &totals(5.0), &[], 5.0).is_none());
            now += 5;
        }
    }

    #[test]
    fn duration_wording() {
        assert_eq!(duration_words(70), "about a minute");
        assert_eq!(duration_words(250), "4 minutes");
        assert_eq!(duration_words(3 * 3600), "about 3 hours");
    }
}
