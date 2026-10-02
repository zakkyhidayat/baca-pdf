// SPDX-License-Identifier: GPL-3.0-or-later

//! One window for everything: a second launch, such as Explorer opening several selected files one
//! program each, hands its files to the window that is already open and quits.

use std::path::PathBuf;
use std::time::Duration;

use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, EnumWindows, GetPropW, SendMessageTimeoutW, ASFW_ANY, SMTO_ABORTIFHUNG, WM_COPYDATA,
};

/// Marks the data in a WM_COPYDATA message as a list of files to open.
pub const COPY_ID: usize = 0x4250_4446;
/// A window property that tells the running instance's window apart.
pub const PROP: PCWSTR = w!("BacaPDF.Primary");

unsafe extern "system" fn pick(h: HWND, found: LPARAM) -> BOOL {
    unsafe {
        if !GetPropW(h, PROP).is_invalid() {
            *(found.0 as *mut HWND) = h;
            return BOOL(0);
        }
    }
    BOOL(1)
}

fn find_running() -> Option<HWND> {
    let mut found = HWND(std::ptr::null_mut());
    unsafe {
        let _ = EnumWindows(Some(pick), LPARAM(&mut found as *mut HWND as isize));
    }
    (!found.0.is_null()).then_some(found)
}

fn absolute(path: &PathBuf) -> PathBuf {
    if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir().map(|d| d.join(path)).unwrap_or_else(|_| path.clone())
    }
}

/// Returns true when another window took the files and this process should quit. The first
/// launch claims the right to be the window and returns false. Tests that set `BACA_DATA_DIR`
/// get their own window unless they also set `BACA_SINGLE`.
pub fn hand_over(files: &[PathBuf]) -> bool {
    if std::env::var_os("BACA_DATA_DIR").is_some() && std::env::var_os("BACA_SINGLE").is_none() {
        return false;
    }
    unsafe {
        let mutex = CreateMutexW(None, true, w!("Local\\BacaPDF.SingleInstance"));
        let taken = GetLastError() == ERROR_ALREADY_EXISTS;
        match mutex {
            Ok(handle) if !taken => {
                // Held until this process ends.
                std::mem::forget(handle);
                return false;
            }
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    let text = files.iter().map(|f| absolute(f).display().to_string()).collect::<Vec<_>>().join("\n");
    let payload: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // The running window may still be starting; keep trying for a few seconds.
    for _ in 0..80 {
        if let Some(window) = find_running() {
            let data = COPYDATASTRUCT {
                dwData: COPY_ID,
                cbData: (payload.len() * 2) as u32,
                lpData: payload.as_ptr() as *mut _,
            };
            let mut answer = 0usize;
            unsafe {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                SendMessageTimeoutW(
                    window,
                    WM_COPYDATA,
                    WPARAM(0),
                    LPARAM(&data as *const COPYDATASTRUCT as isize),
                    SMTO_ABORTIFHUNG,
                    3000,
                    Some(&mut answer),
                );
            }
            if answer == 1 {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}
