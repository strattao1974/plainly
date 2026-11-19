//! Cheap whole-machine sampling: total CPU/memory/disk plus per-process usage, without admin rights.
//! Per-process numbers come from one NtQuerySystemInformation call, which (unlike opening each
//! process) also works for system and protected processes.

use std::collections::HashMap;
use std::ffi::c_void;
use std::time::Instant;
use windows::core::w;
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT_COUNTERVALUE,
    PDH_FMT_DOUBLE,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

#[derive(Clone, Debug, Default)]
pub struct Totals {
    pub cpu: f32,
    pub mem_pct: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    /// Share of the whole machine's processor (0-100), not of one core.
    pub cpu: f32,
    pub mem: u64,
    pub io_rate: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub totals: Totals,
    pub procs: Vec<Proc>,
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    max: u16,
    buffer: *const u16,
}

// Full x64 layout of SYSTEM_PROCESS_INFORMATION; the public headers hide most of it as "Reserved".
#[repr(C)]
#[allow(dead_code)]
struct Spi {
    next: u32,
    threads: u32,
    ws_private: i64,
    hard_faults: u32,
    threads_hwm: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    pid: usize,
    parent_pid: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual: usize,
    virtual_size: usize,
    page_faults: u32,
    peak_ws: usize,
    ws: usize,
    quota_peak_paged: usize,
    quota_paged: usize,
    quota_peak_nonpaged: usize,
    quota_nonpaged: usize,
    pagefile: usize,
    peak_pagefile: usize,
    private_pages: usize,
    read_ops: i64,
    write_ops: i64,
    other_ops: i64,
    read_bytes: i64,
    write_bytes: i64,
    other_bytes: i64,
}

struct RawProc {
    pid: u32,
    name: String,
    create_time: i64,
    cpu_time: i64,
    io_bytes: i64,
    mem: u64,
}

fn read_processes(buf: &mut Vec<u64>) -> Vec<RawProc> {
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC0000004u32 as i32;
    unsafe {
        loop {
            let mut needed = 0u32;
            let status = NtQuerySystemInformation(
                SYSTEM_INFORMATION_CLASS(5),
                buf.as_mut_ptr() as *mut c_void,
                (buf.len() * 8) as u32,
                &mut needed,
            );
            if status.0 == STATUS_INFO_LENGTH_MISMATCH {
                buf.resize(needed as usize / 8 + 8192, 0);
                continue;
            }
            if status.0 < 0 {
                return Vec::new();
            }
            break;
        }
        let mut out = Vec::new();
        let base = buf.as_ptr() as *const u8;
        let mut offset = 0usize;
        loop {
            let spi = std::ptr::read_unaligned(base.add(offset) as *const Spi);
            if spi.pid != 0 {
                let name = if spi.image_name.buffer.is_null() {
                    if spi.pid == 4 { "System".to_string() } else { String::new() }
                } else {
                    let s = std::slice::from_raw_parts(spi.image_name.buffer, spi.image_name.length as usize / 2);
                    String::from_utf16_lossy(s)
                };
                out.push(RawProc {
                    pid: spi.pid as u32,
                    name,
                    create_time: spi.create_time,
                    cpu_time: spi.user_time + spi.kernel_time,
                    io_bytes: spi.read_bytes + spi.write_bytes,
                    mem: spi.ws_private.max(0) as u64,
                });
            }
            if spi.next == 0 {
                break;
            }
            offset += spi.next as usize;
        }
        out
    }
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn system_times() -> Option<(u64, u64)> {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()? };
    // Kernel time already includes idle time.
    Some((ft(idle), ft(kernel) + ft(user)))
}

pub fn memory() -> (f32, u64, u64) {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe {
        if GlobalMemoryStatusEx(&mut m).is_err() {
            return (0.0, 0, 0);
        }
    }
    let used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys);
    (m.dwMemoryLoad as f32, used, m.ullTotalPhys)
}

struct DiskCounter {
    query: isize,
    counter: isize,
}

impl DiskCounter {
    fn new() -> Option<Self> {
        unsafe {
            let mut query = 0isize;
            if PdhOpenQueryW(windows::core::PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut counter = 0isize;
            if PdhAddEnglishCounterW(query, w!("\\PhysicalDisk(_Total)\\% Idle Time"), 0, &mut counter) != 0 {
                return None;
            }
            PdhCollectQueryData(query);
            Some(DiskCounter { query, counter })
        }
    }

    fn busy(&self) -> Option<f32> {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let mut v = PDH_FMT_COUNTERVALUE::default();
            if PdhGetFormattedCounterValue(self.counter, PDH_FMT_DOUBLE, None, &mut v) != 0 {
                return None;
            }
            let idle = v.Anonymous.doubleValue as f32;
            Some((100.0 - idle).clamp(0.0, 100.0))
        }
    }
}

pub struct Sampler {
    buf: Vec<u64>,
    prev_sys: Option<(u64, u64)>,
    prev_proc_sys: Option<u64>,
    prev_procs: HashMap<(u32, i64), (i64, i64)>,
    prev_proc_at: Option<Instant>,
    disk: Option<DiskCounter>,
}

impl Sampler {
    pub fn new() -> Self {
        let mut s = Sampler {
            buf: vec![0u64; 64 * 1024],
            prev_sys: system_times(),
            prev_proc_sys: None,
            prev_procs: HashMap::new(),
            prev_proc_at: None,
            disk: DiskCounter::new(),
        };
        s.prev_proc_sys = s.prev_sys.map(|x| x.1);
        s
    }

    /// Process numbers are averages since the previous sample that included processes.
    pub fn sample(&mut self, with_procs: bool) -> Sample {
        let sys = system_times();
        let cpu = match (self.prev_sys, sys) {
            (Some((pi, pt)), Some((ni, nt))) if nt > pt => (1.0 - (ni - pi) as f64 / (nt - pt) as f64) as f32 * 100.0,
            _ => 0.0,
        };
        self.prev_sys = sys;
        let (mem_pct, mem_used, mem_total) = memory();
        let disk = self.disk.as_ref().and_then(|d| d.busy());
        let totals = Totals { cpu: cpu.clamp(0.0, 100.0), mem_pct, mem_used, mem_total, disk };

        let mut procs = Vec::new();
        if with_procs {
            let raw = read_processes(&mut self.buf);
            let now = Instant::now();
            let sys_total = sys.map(|s| s.1);
            let dsys = match (self.prev_proc_sys, sys_total) {
                (Some(p), Some(n)) if n > p => Some((n - p) as f64),
                _ => None,
            };
            let dt = self.prev_proc_at.map(|t| now.duration_since(t).as_secs_f64()).filter(|d| *d > 0.05);
            let mut next = HashMap::with_capacity(raw.len());
            for p in raw {
                let key = (p.pid, p.create_time);
                if let (Some(&(cpu0, io0)), Some(dsys), Some(dt)) = (self.prev_procs.get(&key), dsys, dt) {
                    procs.push(Proc {
                        pid: p.pid,
                        name: p.name.clone(),
                        cpu: (((p.cpu_time - cpu0).max(0) as f64 / dsys) * 100.0) as f32,
                        mem: p.mem,
                        io_rate: (p.io_bytes - io0).max(0) as f64 / dt,
                    });
                }
                next.insert(key, (p.cpu_time, p.io_bytes));
            }
            self.prev_procs = next;
            self.prev_proc_sys = sys_total;
            self.prev_proc_at = Some(now);
        }
        Sample { totals, procs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_layout_matches_windows() {
        assert_eq!(std::mem::size_of::<Spi>(), 0x100);
    }

    #[test]
    fn sampler_sees_this_process() {
        let mut s = Sampler::new();
        s.sample(true);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let sample = s.sample(true);
        let me = std::process::id();
        assert!(sample.procs.iter().any(|p| p.pid == me));
        assert!(sample.totals.mem_total > 0);
    }
}
