//! Messages between the background watcher (owns the tray) and the window process.
//! Each side has a hidden message window; WM_COPYDATA carries short text commands.

use crate::winutil::wide;
use std::ffi::c_void;
use std::sync::OnceLock;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const AGENT_CLASS: PCWSTR = w!("PlainlyTrayWindow");
pub const WINDOW_CLASS: PCWSTR = w!("PlainlyWindowMessages");

fn find(class: PCWSTR) -> Option<HWND> {
    unsafe { FindWindowW(class, PCWSTR::null()).ok().filter(|h| !h.0.is_null()) }
}

fn send(class: PCWSTR, msg: &str) -> bool {
    let Some(h) = find(class) else { return false };
    let bytes = msg.as_bytes();
    let cds = COPYDATASTRUCT { dwData: 0x504c, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut c_void };
    unsafe {
        let r = SendMessageTimeoutW(
            h,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            3000,
            None,
        );
        r.0 != 0
    }
}

/// Reads the text a WM_COPYDATA message carries.
pub unsafe fn read_copydata(lparam: LPARAM) -> Option<String> {
    let cds = (lparam.0 as *const COPYDATASTRUCT).as_ref()?;
    if cds.dwData != 0x504c || cds.lpData.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
    Some(String::from_utf8_lossy(bytes).to_string())
}

pub fn agent_running() -> bool {
    find(AGENT_CLASS).is_some()
}

pub fn send_to_agent(msg: &str) -> bool {
    send(AGENT_CLASS, msg)
}

pub fn send_to_window(msg: &str) -> bool {
    unsafe {
        let _ = AllowSetForegroundWindow(u32::MAX);
    }
    send(WINDOW_CLASS, msg)
}

pub fn start_agent() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--background").spawn();
    }
}

/// Brings the window forward (on `page` if given), starting the window process if it isn't open.
pub fn open_window(page: Option<&str>) {
    unsafe {
        let _ = AllowSetForegroundWindow(u32::MAX);
    }
    let msg = page.map(|p| format!("page:{p}")).unwrap_or_else(|| "open".into());
    if send(WINDOW_CLASS, &msg) {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--window");
        if let Some(p) = page {
            cmd.args(["--page", p]);
        }
        let _ = cmd.spawn();
    }
}

static WINDOW_HANDLER: OnceLock<Box<dyn Fn(String) + Send + Sync>> = OnceLock::new();

/// In the window process: listen for "open" / "page:x" from the background watcher.
pub fn listen_in_window(handler: impl Fn(String) + Send + Sync + 'static) {
    let _ = WINDOW_HANDLER.set(Box::new(handler));
    std::thread::Builder::new()
        .name("window-messages".into())
        .spawn(|| unsafe {
            let Ok(module) = GetModuleHandleW(None) else { return };
            let wc = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: module.into(), lpszClassName: WINDOW_CLASS, ..Default::default() };
            RegisterClassW(&wc);
            let title = wide("Plainly window messages");
            let Ok(_hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                WINDOW_CLASS,
                PCWSTR(title.as_ptr()),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                HWND::default(),
                HMENU::default(),
                module,
                None,
            ) else {
                return;
            };
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .ok();
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        if let (Some(text), Some(h)) = (read_copydata(lparam), WINDOW_HANDLER.get()) {
            h(text);
            return LRESULT(1);
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
