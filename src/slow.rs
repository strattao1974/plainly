//! "Why is my PC slow?": gathers facts, then explains them in plain English.

use crate::journal::Action;
use crate::kb;
use crate::perf::{Proc, Sampler, Totals};
use crate::winutil::{exe_path_of_pid, file_description, fmt_bytes};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Group {
    pub label: String,
    pub why: String,
    pub tip: Option<String>,
    pub pids: Vec<u32>,
    pub cpu: f32,
    pub mem: u64,
    pub io: f64,
    pub count: usize,
    pub can_end: bool,
}

pub fn services_by_pid() -> HashMap<u32, Vec<String>> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Services::{
        CloseServiceHandle, EnumServicesStatusExW, OpenSCManagerW, ENUM_SERVICE_STATUS_PROCESSW, SC_ENUM_PROCESS_INFO,
        SC_MANAGER_ENUMERATE_SERVICE, SERVICE_ACTIVE, SERVICE_WIN32,
    };
    let mut map: HashMap<u32, Vec<String>> = HashMap::new();
    unsafe {
        let Ok(scm) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ENUMERATE_SERVICE) else { return map };
        let mut needed = 0u32;
        let mut returned = 0u32;
        let mut resume = 0u32;
        let _ = EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_ACTIVE, None, &mut needed, &mut returned, Some(&mut resume), PCWSTR::null());
        if needed > 0 {
            let mut buf = vec![0u64; needed as usize / 8 + 512];
            let bytes = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 8);
            resume = 0;
            if EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_ACTIVE, Some(bytes), &mut needed, &mut returned, Some(&mut resume), PCWSTR::null()).is_ok() {
                let arr = buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW;
                for i in 0..returned as usize {
                    let e = &*arr.add(i);
                    let name = e.lpServiceName.to_string().unwrap_or_default();
                    map.entry(e.ServiceStatusProcess.dwProcessId).or_default().push(name);
                }
            }
        }
        let _ = CloseServiceHandle(scm);
    }
    map
}

/// Groups processes the way people think about them: all of Chrome together, each Windows service host separately.
pub fn group(procs: &[Proc], services: &HashMap<u32, Vec<String>>) -> Vec<Group> {
    let mut groups: HashMap<String, Group> = HashMap::new();
    let me = std::process::id();
    for p in procs {
        let exe = p.name.to_ascii_lowercase();
        if exe.is_empty() || exe == "idle" || p.pid == me {
            continue;
        }
        let key = if exe == "svchost.exe" { format!("svchost:{}", p.pid) } else { exe.clone() };
        let g = groups.entry(key).or_insert_with(|| describe(&exe, p.pid, services));
        g.pids.push(p.pid);
        g.cpu += p.cpu;
        g.mem += p.mem;
        g.io += p.io_rate;
        g.count += 1;
    }
    groups.into_values().collect()
}

fn describe(exe: &str, pid: u32, services: &HashMap<u32, Vec<String>>) -> Group {
    let blank = |label: String, why: String, tip: Option<String>, can_end: bool| Group {
        label,
        why,
        tip,
        pids: Vec::new(),
        cpu: 0.0,
        mem: 0,
        io: 0.0,
        count: 0,
        can_end,
    };
    if exe == "svchost.exe" {
        let names = services.get(&pid).cloned().unwrap_or_default();
        let lower: Vec<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
        if let Some(known) = lower.iter().find_map(|n| kb::service(n)) {
            return blank(format!("Windows service: {}", known.label), known.why.to_string(), None, false);
        }
        let why = if names.is_empty() {
            "A group of Windows background services.".to_string()
        } else {
            format!("Windows background service ({}).", names.join(", "))
        };
        return blank("Windows background service".into(), why, None, false);
    }
    if let Some(k) = kb::process(exe) {
        return blank(k.name.into(), k.why.into(), k.tip.map(str::to_string), !kb::is_protected(exe));
    }
    let label = exe_path_of_pid(pid)
        .and_then(|p| file_description(&p))
        .unwrap_or_else(|| exe.trim_end_matches(".exe").to_string());
    blank(label, "An app running on your PC.".into(), None, !kb::is_protected(exe))
}

#[derive(Clone, Debug, Default)]
pub struct SlowFacts {
    pub totals: Totals,
    pub groups: Vec<Group>,
    pub uptime_days: f32,
    pub reboot_pending: bool,
    /// (drive, percent free, bytes free)
    pub drives: Vec<(String, f32, u64)>,
    pub on_battery: bool,
    pub battery_saver: bool,
    pub power_saver_plan: bool,
    pub startup_count: usize,
}

pub fn gather(startup_count: usize) -> SlowFacts {
    let mut sampler = Sampler::new();
    sampler.sample(true);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let sample = sampler.sample(true);
    let services = services_by_pid();
    let (on_battery, battery_saver) = power_status();
    SlowFacts {
        totals: sample.totals,
        groups: group(&sample.procs, &services),
        uptime_days: crate::winutil::uptime_secs() as f32 / 86_400.0,
        reboot_pending: crate::collect::reboot_pending(),
        drives: crate::collect::drives()
            .into_iter()
            .map(|d| (d.root, d.free as f32 / d.total as f32 * 100.0, d.free))
            .collect(),
        on_battery,
        battery_saver,
        power_saver_plan: power_saver_plan(),
        startup_count,
    }
}

fn power_status() -> (bool, bool) {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe {
        if GetSystemPowerStatus(&mut s).is_err() {
            return (false, false);
        }
    }
    let has_battery = s.BatteryFlag != 128 && s.BatteryFlag != 255;
    (has_battery && s.ACLineStatus == 0, s.SystemStatusFlag == 1)
}

fn power_saver_plan() -> bool {
    use windows::core::GUID;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::System::Power::PowerGetActiveScheme;
    const POWER_SAVER: GUID = GUID::from_u128(0xa1841308_3541_4fab_bc81_f71556f20b4a);
    unsafe {
        let mut ptr: *mut GUID = std::ptr::null_mut();
        if PowerGetActiveScheme(None, &mut ptr).is_err() || ptr.is_null() {
            return false;
        }
        let is_saver = *ptr == POWER_SAVER;
        let _ = LocalFree(HLOCAL(ptr as *mut _));
        is_saver
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Calm,
    Busy,
    Strained,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Cpu,
    Memory,
    Disk,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub text: String,
    pub action: Option<Action>,
}

#[derive(Clone, Debug)]
pub struct SlowReport {
    pub level: Level,
    pub resource: Resource,
    pub headline: String,
    pub summary: String,
    pub culprits: Vec<Group>,
    pub findings: Vec<Finding>,
    pub totals: Totals,
}

fn pressure(t: &Totals) -> (Level, Resource) {
    let cpu = t.cpu / 80.0;
    let mem = t.mem_pct / 90.0;
    let disk = t.disk.unwrap_or(0.0) / 85.0;
    let (worst, res) = [(cpu, Resource::Cpu), (mem, Resource::Memory), (disk, Resource::Disk)]
        .into_iter()
        .fold((0.0f32, Resource::Cpu), |a, b| if b.0 > a.0 { b } else { a });
    let level = if worst >= 1.0 {
        Level::Strained
    } else if t.cpu >= 50.0 || t.mem_pct >= 80.0 || t.disk.unwrap_or(0.0) >= 60.0 {
        Level::Busy
    } else {
        Level::Calm
    };
    let res = if level == Level::Busy {
        [(t.cpu / 50.0, Resource::Cpu), (t.mem_pct / 80.0, Resource::Memory), (t.disk.unwrap_or(0.0) / 60.0, Resource::Disk)]
            .into_iter()
            .fold((0.0f32, Resource::Cpu), |a, b| if b.0 > a.0 { b } else { a })
            .1
    } else {
        res
    };
    (level, res)
}

pub fn rank(groups: &[Group], res: Resource) -> Vec<Group> {
    let mut g: Vec<Group> = groups
        .iter()
        .filter(|g| match res {
            Resource::Cpu => g.cpu >= 1.0,
            Resource::Memory => g.mem >= 150_000_000,
            Resource::Disk => g.io >= 500_000.0,
        })
        .cloned()
        .collect();
    match res {
        Resource::Cpu => g.sort_by(|a, b| b.cpu.total_cmp(&a.cpu)),
        Resource::Memory => g.sort_by(|a, b| b.mem.cmp(&a.mem)),
        Resource::Disk => g.sort_by(|a, b| b.io.total_cmp(&a.io)),
    }
    g.truncate(6);
    g
}

pub fn build(f: &SlowFacts) -> SlowReport {
    let (level, resource) = pressure(&f.totals);
    // On a calm PC the top processor users are trivial; the big memory users are what's worth knowing.
    let resource = if level == Level::Calm { Resource::Memory } else { resource };
    let mut culprits = rank(&f.groups, resource);
    if level == Level::Calm {
        culprits.truncate(4);
    }
    let findings = findings(f, level);

    let (headline, summary) = match level {
        Level::Calm => (
            "Your PC isn't busy right now".to_string(),
            if findings.is_empty() {
                "Nothing is hogging it and nothing looks wrong. If it felt slow a moment ago, the Timeline shows any slowdowns Plainly caught.".to_string()
            } else {
                format!(
                    "Nothing is hogging it at the moment, but {} below can make it feel slow.",
                    if findings.len() == 1 { "one thing".to_string() } else { format!("{} things", findings.len()) }
                )
            },
        ),
        _ => {
            let head = match resource {
                Resource::Cpu => format!("The processor is working hard ({:.0}% busy)", f.totals.cpu),
                Resource::Memory => format!("Memory is nearly full ({:.0}% used)", f.totals.mem_pct),
                Resource::Disk => format!("The disk is flat out ({:.0}% busy)", f.totals.disk.unwrap_or(0.0)),
            };
            let summary = match culprits.first() {
                Some(top) => {
                    let amount = match resource {
                        Resource::Cpu => format!("{:.0}% of the processor", top.cpu),
                        Resource::Memory => format!("{} of memory", fmt_bytes(top.mem)),
                        Resource::Disk => format!("{}/s of disk activity", fmt_bytes(top.io as u64)),
                    };
                    format!("The biggest cause is {}, using {amount}. {}", top.label, top.why)
                }
                None => "No single program stands out. It's lots of small things adding up.".to_string(),
            };
            (head, summary)
        }
    };
    SlowReport { level, resource, headline, summary, culprits, findings, totals: f.totals.clone() }
}

fn findings(f: &SlowFacts, level: Level) -> Vec<Finding> {
    let mut out = Vec::new();
    if f.reboot_pending {
        out.push(Finding {
            text: "Updates are waiting for a restart to finish installing. Until then Windows can feel sluggish.".into(),
            action: Some(Action::open("Windows Update", "ms-settings:windowsupdate")),
        });
    }
    if f.uptime_days >= 7.0 {
        out.push(Finding {
            text: format!(
                "Your PC hasn't fully restarted in {:.0} days. On most PCs, Shut down doesn't count (Windows hibernates instead), so choose Restart now and then. It clears out a lot of clutter.",
                f.uptime_days.floor()
            ),
            action: None,
        });
    }
    for (drive, pct, free) in &f.drives {
        if *pct < 10.0 {
            out.push(Finding {
                text: format!("Drive {drive} is nearly full: only {} left ({pct:.0}% free). Windows slows down when it's short on space.", fmt_bytes(*free)),
                action: Some(Action::open("Storage settings", "ms-settings:storagesense")),
            });
        }
    }
    if f.battery_saver {
        out.push(Finding {
            text: "Battery saver is on. It deliberately slows the PC down to make the battery last.".into(),
            action: Some(Action::open("Battery settings", "ms-settings:batterysaver")),
        });
    } else if f.on_battery && level != Level::Calm {
        out.push(Finding { text: "You're running on battery. Many laptops slow themselves down when unplugged.".into(), action: None });
    }
    if f.power_saver_plan {
        out.push(Finding {
            text: "The power plan is set to Power saver, which limits how fast the PC can run.".into(),
            action: Some(Action::run("Power options", "control.exe", "powercfg.cpl")),
        });
    }
    if f.startup_count >= 10 {
        out.push(Finding {
            text: format!("{} apps start by themselves when you sign in. That slows startup and keeps memory busy all day.", f.startup_count),
            action: Some(Action::open("Startup apps", "ms-settings:startupapps")),
        });
    }
    let gb = f.totals.mem_total as f64 / 1e9;
    if gb > 0.0 && gb < 7.0 && f.totals.mem_pct >= 80.0 {
        out.push(Finding {
            text: format!("This PC has {gb:.0} GB of memory. Modern browsers and apps can fill that quickly; closing tabs helps most."),
            action: None,
        });
    }
    out
}

impl SlowReport {
    pub fn to_text(&self) -> String {
        let mut s = format!("{}\n{}\n\n", self.headline, self.summary);
        s.push_str(&format!(
            "Processor {:.0}% · Memory {:.0}% ({} of {}) · Disk {}\n\n",
            self.totals.cpu,
            self.totals.mem_pct,
            fmt_bytes(self.totals.mem_used),
            fmt_bytes(self.totals.mem_total),
            self.totals.disk.map(|d| format!("{d:.0}%")).unwrap_or_else(|| "n/a".into())
        ));
        if !self.culprits.is_empty() {
            s.push_str("What's using the PC:\n");
            for c in &self.culprits {
                s.push_str(&format!(
                    "- {}: processor {:.0}%, memory {}, disk {}/s. {}\n",
                    c.label,
                    c.cpu,
                    fmt_bytes(c.mem),
                    fmt_bytes(c.io as u64),
                    c.why
                ));
            }
        }
        if !self.findings.is_empty() {
            s.push_str("\nOther things worth knowing:\n");
            for f in &self.findings {
                s.push_str(&format!("- {}\n", f.text));
            }
        }
        s.push_str("\n(Report from Plainly)");
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(label: &str, cpu: f32, mem: u64) -> Group {
        Group { label: label.into(), why: format!("{label} why."), tip: None, pids: vec![1], cpu, mem, io: 0.0, count: 1, can_end: true }
    }

    fn facts(cpu: f32, mem: f32) -> SlowFacts {
        SlowFacts {
            totals: Totals { cpu, mem_pct: mem, mem_used: 8_000_000_000, mem_total: 16_000_000_000, disk: Some(5.0) },
            groups: vec![g("Microsoft Defender", 61.0, 200_000_000), g("Google Chrome", 12.0, 3_000_000_000)],
            ..Default::default()
        }
    }

    #[test]
    fn busy_cpu_names_the_top_culprit() {
        let r = build(&facts(92.0, 50.0));
        assert_eq!(r.level, Level::Strained);
        assert_eq!(r.resource, Resource::Cpu);
        assert!(r.headline.contains("92%"));
        assert!(r.summary.starts_with("The biggest cause is Microsoft Defender"));
        assert_eq!(r.culprits[0].label, "Microsoft Defender");
    }

    #[test]
    fn full_memory_ranks_by_memory() {
        let r = build(&facts(20.0, 95.0));
        assert_eq!(r.resource, Resource::Memory);
        assert_eq!(r.culprits[0].label, "Google Chrome");
    }

    #[test]
    fn calm_pc_with_old_uptime_suggests_restart() {
        let mut f = facts(5.0, 40.0);
        f.uptime_days = 23.4;
        let r = build(&f);
        assert_eq!(r.level, Level::Calm);
        assert!(r.findings.iter().any(|x| x.text.contains("23 days")));
        assert!(r.summary.contains("one thing"));
    }

    #[test]
    fn nearly_full_drive_is_flagged() {
        let mut f = facts(5.0, 40.0);
        f.drives = vec![("C:".into(), 4.0, 9_000_000_000)];
        let r = build(&f);
        assert!(r.findings.iter().any(|x| x.text.contains("Drive C: is nearly full")));
    }

    #[test]
    fn report_text_includes_culprits() {
        let r = build(&facts(92.0, 50.0));
        let t = r.to_text();
        assert!(t.contains("Microsoft Defender: processor 61%"));
    }
}
