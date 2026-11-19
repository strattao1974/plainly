//! The tray icon, its menu, and notifications. Uses the classic Shell_NotifyIcon API directly:
//! Windows 10/11 shows its balloons as proper notifications, credited to Plainly, without the
//! Start-menu shortcut registration that toast notifications need.

use crate::winutil::wide;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::OnceLock;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_LARGE_ICON, NIIF_RESPECT_QUIET_TIME, NIIF_USER, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_BALLOONUSERCLICK, NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};

const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCmd {
    Open,
    Timeline,
    Slow,
    Internet,
    /// From the window: look for changes now.
    Scan,
    /// From the window, on its first close: explain that Plainly keeps running here.
    Hint,
    Quit,
}

const WM_TRAY: u32 = WM_APP + 1;
const CLASS: PCWSTR = crate::ipc::AGENT_CLASS;
const ID_OPEN: usize = 1;
const ID_SLOW: usize = 2;
const ID_NET: usize = 3;
const ID_QUIT: usize = 9;

static HWND_TRAY: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static HANDLER: OnceLock<Box<dyn Fn(TrayCmd) + Send + Sync>> = OnceLock::new();

/// Adds the tray icon and runs its message loop on the calling thread until the process exits.
pub fn run(handler: impl Fn(TrayCmd) + Send + Sync + 'static) {
    let _ = HANDLER.set(Box::new(handler));
    unsafe { message_loop() }
}

fn dispatch(cmd: TrayCmd) {
    if let Some(h) = HANDLER.get() {
        h(cmd);
    }
}

fn command(text: &str) -> Option<TrayCmd> {
    Some(match text {
        "open" => TrayCmd::Open,
        "page:timeline" => TrayCmd::Timeline,
        "page:slow" => TrayCmd::Slow,
        "page:internet" => TrayCmd::Internet,
        "scan" => TrayCmd::Scan,
        "hint" => TrayCmd::Hint,
        _ => return None,
    })
}

unsafe fn message_loop() {
    let Ok(module) = GetModuleHandleW(None) else { return };
    let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: module.into(), lpszClassName: CLASS, ..Default::default() };
    RegisterClassW(&wc);
    let Ok(hwnd) = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        CLASS,
        w!("Plainly"),
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
    HWND_TRAY.store(hwnd.0 as isize, Ordering::SeqCst);
    TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::SeqCst);
    add_icon(hwnd);
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

fn copy_into(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

unsafe fn load_icon(big: bool) -> HICON {
    let (cx, cy) = if big {
        (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON))
    } else {
        (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON))
    };
    let Ok(module) = GetModuleHandleW(None) else { return HICON::default() };
    LoadImageW(module, PCWSTR(1 as *const u16), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR)
        .map(|h| HICON(h.0))
        .unwrap_or_default()
}

unsafe fn add_icon(hwnd: HWND) {
    let mut n = base(hwnd);
    n.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
    n.uCallbackMessage = WM_TRAY;
    n.hIcon = load_icon(false);
    copy_into(&mut n.szTip, "Plainly: your PC, explained");
    let _ = Shell_NotifyIconW(NIM_ADD, &n);
    n.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    let _ = Shell_NotifyIconW(NIM_SETVERSION, &n);
}

pub fn notify(title: &str, body: &str) {
    let h = HWND_TRAY.load(Ordering::SeqCst);
    if h == 0 {
        return;
    }
    unsafe {
        let mut n = base(HWND(h as *mut _));
        n.uFlags = NIF_INFO;
        copy_into(&mut n.szInfoTitle, title);
        copy_into(&mut n.szInfo, body);
        n.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON | NIIF_RESPECT_QUIET_TIME;
        n.hBalloonIcon = load_icon(true);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &n);
    }
}

pub fn remove() {
    let h = HWND_TRAY.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &base(HWND(h as *mut _)));
        }
    }
}

unsafe fn show_menu(hwnd: HWND, x: i32, y: i32) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let items: [(usize, &str); 3] = [(ID_OPEN, "Open Plainly"), (ID_SLOW, "Why is my PC slow?"), (ID_NET, "Check my internet")];
    for (id, text) in items {
        let t = wide(text);
        let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(t.as_ptr()));
    }
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, w!("Quit Plainly"));
    let _ = SetMenuDefaultItem(menu, ID_OPEN as u32, 0);
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, x, y, 0, hwnd, None).0 as usize;
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    match cmd {
        ID_OPEN => dispatch(TrayCmd::Open),
        ID_SLOW => dispatch(TrayCmd::Slow),
        ID_NET => dispatch(TrayCmd::Internet),
        ID_QUIT => dispatch(TrayCmd::Quit),
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_TRAY {
        let event = (lparam.0 as u32) & 0xFFFF;
        match event {
            e if e == NIN_SELECT || e == NIN_KEYSELECT => dispatch(TrayCmd::Open),
            e if e == NIN_BALLOONUSERCLICK => dispatch(TrayCmd::Timeline),
            WM_CONTEXTMENU => {
                let x = (wparam.0 & 0xFFFF) as i16 as i32;
                let y = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let (x, y) = if x == 0 && y == 0 {
                    let mut p = POINT::default();
                    let _ = GetCursorPos(&mut p);
                    (p.x, p.y)
                } else {
                    (x, y)
                };
                show_menu(hwnd, x, y);
            }
            _ => {}
        }
        return LRESULT(0);
    }
    if msg == WM_COPYDATA {
        if let Some(cmd) = crate::ipc::read_copydata(lparam).as_deref().and_then(command) {
            dispatch(cmd);
            return LRESULT(1);
        }
        return LRESULT(0);
    }
    let tc = TASKBAR_CREATED.load(Ordering::SeqCst);
    if tc != 0 && msg == tc {
        add_icon(hwnd);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
