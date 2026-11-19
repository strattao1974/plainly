//! HTTPS via Windows' own WinHTTP: no TLS library to ship, and it honours the system proxy.

use crate::winutil::wide;
use std::ffi::c_void;
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

pub struct Response {
    pub status: u32,
    pub bytes: u64,
    pub elapsed: Duration,
}

struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// GETs `https://host/path`, reading (and counting, not keeping) the body for at most `max_time`.
pub fn get(host: &str, path: &str, max_time: Duration) -> Result<Response, String> {
    let host_w = wide(host);
    let path_w = wide(path);
    let start = Instant::now();
    unsafe {
        let session = Handle(WinHttpOpen(
            w!("Plainly/1.0"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err("couldn't start a web connection".into());
        }
        let _ = WinHttpSetTimeouts(session.0, 5000, 5000, 5000, 8000);
        let connect = Handle(WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if connect.0.is_null() {
            return Err("couldn't connect".into());
        }
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            PCWSTR(path_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err("couldn't create the request".into());
        }
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|e| describe(e.code().0))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| describe(e.code().0))?;

        let mut status = 0u32;
        let mut size = 4u32;
        let _ = WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut size,
            std::ptr::null_mut(),
        );

        let mut total = 0u64;
        let mut chunk = vec![0u8; 64 * 1024];
        while start.elapsed() < max_time {
            let mut read = 0u32;
            if WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read).is_err() || read == 0 {
                break;
            }
            total += read as u64;
        }
        Ok(Response { status, bytes: total, elapsed: start.elapsed() })
    }
}

fn describe(hresult: i32) -> String {
    let code = (hresult as u32) & 0xFFFF;
    match code {
        12002 => "timed out".into(),
        12007 => "the website's name couldn't be looked up".into(),
        12029 => "couldn't connect".into(),
        12030 => "the connection was cut off".into(),
        12175 | 12157 | 12169 | 12038 | 12037 | 12044 | 12045 => {
            "the secure connection failed (often a wrong PC clock, or security software getting in the way)".into()
        }
        _ => format!("error {code}"),
    }
}
