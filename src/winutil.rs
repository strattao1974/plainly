use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HWND};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};
use windows::Win32::UI::Shell::{SHLoadIndirectString, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW, SW_SHOWNORMAL,
};
use winreg::enums::*;
use winreg::RegKey;

pub const RUN_VALUE: &str = "Plainly";

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    base.join("Plainly")
}

pub fn shell_open(target: &str) {
    let t = wide(target);
    unsafe {
        ShellExecuteW(HWND::default(), w!("open"), PCWSTR(t.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn run(exe: &str, args: &str) {
    let e = wide(exe);
    let a = wide(args);
    unsafe {
        ShellExecuteW(HWND::default(), w!("open"), PCWSTR(e.as_ptr()), PCWSTR(a.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn reveal(path: &str) {
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
}

static INDIRECT_CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// Resolves "@dll,-123" / "@{Package?ms-resource://...}" strings Windows stores for display names.
pub fn indirect_string(s: &str) -> Option<String> {
    if !s.starts_with('@') {
        return Some(s.to_string());
    }
    let mut cache = INDIRECT_CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(hit) = cache.get(s) {
        return hit.clone();
    }
    let src = wide(s);
    let mut out = vec![0u16; 1024];
    let resolved = unsafe { SHLoadIndirectString(PCWSTR(src.as_ptr()), &mut out, None) }
        .ok()
        .map(|_| from_wide(&out))
        .filter(|r| !r.is_empty());
    cache.insert(s.to_string(), resolved.clone());
    resolved
}

static DESC_CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// The "File description" from an exe's version info, e.g. "Google Chrome".
pub fn file_description(path: &str) -> Option<String> {
    let mut cache = DESC_CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(hit) = cache.get(path) {
        return hit.clone();
    }
    let result = unsafe { read_file_description(path) };
    cache.insert(path.to_string(), result.clone());
    result
}

unsafe fn read_file_description(path: &str) -> Option<String> {
    let p = wide(path);
    let size = GetFileVersionInfoSizeW(PCWSTR(p.as_ptr()), None);
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    GetFileVersionInfoW(PCWSTR(p.as_ptr()), 0, size, data.as_mut_ptr() as *mut c_void).ok()?;

    let mut ptr: *mut c_void = std::ptr::null_mut();
    let mut len = 0u32;
    let mut lang = (0x0409u16, 0x04b0u16);
    if VerQueryValueW(data.as_ptr() as *const c_void, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len).as_bool()
        && len >= 4
    {
        let pair = ptr as *const u16;
        lang = (*pair, *pair.add(1));
    }
    let sub = wide(&format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription", lang.0, lang.1));
    if !VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(sub.as_ptr()), &mut ptr, &mut len).as_bool() || len == 0 {
        return None;
    }
    let slice = std::slice::from_raw_parts(ptr as *const u16, len as usize);
    let s = from_wide(slice).trim().to_string();
    (!s.is_empty()).then_some(s)
}

pub fn exe_path_of_pid(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut size = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

pub fn terminate(pid: u32) -> bool {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) else { return false };
        let ok = TerminateProcess(h, 1).is_ok();
        let _ = CloseHandle(h);
        ok
    }
}

pub fn claim_single_instance(name: &str) -> bool {
    let n = wide(name);
    unsafe {
        let r = CreateMutexW(None, true, PCWSTR(n.as_ptr()));
        // Leak the handle on purpose: the mutex must live as long as the process.
        r.is_ok() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

pub fn show_window(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let h = HWND(hwnd as *mut c_void);
    unsafe {
        let _ = ShowWindow(h, SW_SHOW);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let _ = SetForegroundWindow(h);
    }
}

pub fn uptime_secs() -> u64 {
    unsafe { GetTickCount64() / 1000 }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub fn starts_with_windows() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(RUN_KEY)
        .and_then(|k| k.get_value::<String, _>(RUN_VALUE))
        .is_ok()
}

pub fn set_starts_with_windows(on: bool) {
    let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY) else { return };
    if on {
        if let Ok(exe) = std::env::current_exe() {
            let _ = key.set_value(RUN_VALUE, &format!("\"{}\" --background", exe.display()));
        }
    } else {
        let _ = key.delete_value(RUN_VALUE);
    }
}

/// Turns a startup entry off the same way Task Manager does, so Windows' own UI agrees with us.
pub fn set_startup_approved(hive: &str, approved_key: &str, name: &str, enabled: bool) -> bool {
    let root = if hive == "HKLM" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER };
    let path = format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\{approved_key}");
    let Ok((key, _)) = RegKey::predef(root).create_subkey(path) else { return false };
    let mut bytes = vec![if enabled { 2u8 } else { 3u8 }, 0, 0, 0];
    let ft = (crate::journal::now() as u64 + 11_644_473_600) * 10_000_000;
    bytes.extend_from_slice(&ft.to_le_bytes());
    key.set_raw_value(name, &winreg::RegValue { bytes, vtype: REG_BINARY }).is_ok()
}

/// Pulls the program path out of a command line like `"C:\x\y.exe" --flag` or `C:\x\y.exe -a`.
pub fn exe_from_command(cmd: &str) -> Option<String> {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        return rest.split('"').next().map(str::to_string).filter(|s| !s.is_empty());
    }
    let lower = cmd.to_ascii_lowercase();
    for ext in [".exe", ".lnk", ".bat", ".cmd", ".vbs", ".ps1"] {
        if let Some(i) = lower.find(ext) {
            return Some(cmd[..i + ext.len()].to_string());
        }
    }
    cmd.split_whitespace().next().map(str::to_string)
}

pub fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{b:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_from_command_handles_quotes_and_args() {
        assert_eq!(exe_from_command(r#""C:\Program Files\App\app.exe" --min"#).as_deref(), Some(r"C:\Program Files\App\app.exe"));
        assert_eq!(exe_from_command(r"C:\Tools\thing.exe -silent").as_deref(), Some(r"C:\Tools\thing.exe"));
        assert_eq!(exe_from_command(r"C:\Program Files\Odd\run.exe /x").as_deref(), Some(r"C:\Program Files\Odd\run.exe"));
        assert_eq!(exe_from_command("  ").as_deref(), None);
    }

    #[test]
    fn fmt_bytes_is_readable() {
        assert_eq!(fmt_bytes(1_500_000_000), "1.5 GB");
        assert_eq!(fmt_bytes(450_000_000), "450 MB");
        assert_eq!(fmt_bytes(2_000), "2 KB");
    }
}
