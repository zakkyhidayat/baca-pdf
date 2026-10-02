// SPDX-License-Identifier: GPL-3.0-or-later

//! Puts the tab strip in the title bar. The native frame stays (shadow, rounded corners, resize
//! borders, Aero Snap); only the caption is taken over, and Windows is told where the drag area and
//! the maximize button are, so double-click, system menu and Snap Layouts keep working.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GetWindowLongW, GetWindowRect, IsWindowVisible, IsZoomed, SetWindowPos,
    ShowWindow, GWL_STYLE, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTMAXBUTTON, HTTOP, NCCALCSIZE_PARAMS, SM_CXPADDEDBORDER,
    SM_CYFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE, SW_RESTORE,
    WM_DPICHANGED, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_SETTINGCHANGE, WM_SIZE,
    WS_CAPTION,
};

/// Logical sizes shared with the Slint layout, stored as hundredths of a logical pixel.
static STRIP_HEIGHT: AtomicI32 = AtomicI32::new(4800);
static DRAG_FROM: AtomicI32 = AtomicI32::new(0);
pub const CAPTION_BUTTON_WIDTH: f32 = 46.0;

static HOVER_MAX: AtomicBool = AtomicBool::new(false);
static PRESSED_MAX: AtomicBool = AtomicBool::new(false);
static mut WINDOW: HWND = HWND(std::ptr::null_mut());

/// Called by the window whenever the tab strip changes size.
pub fn set_metrics(strip_height: f32, drag_from: f32) {
    STRIP_HEIGHT.store((strip_height * 100.0) as i32, Ordering::Relaxed);
    DRAG_FROM.store((drag_from * 100.0) as i32, Ordering::Relaxed);
}

pub fn installed() -> bool {
    hwnd().is_some()
}

pub fn hwnd() -> Option<HWND> {
    let h = unsafe { WINDOW };
    (!h.0.is_null()).then_some(h)
}

/// Finds this thread's visible top-level window, which is the Slint window.
fn find_window() -> Option<HWND> {
    unsafe extern "system" fn pick(h: HWND, out: LPARAM) -> BOOL {
        unsafe {
            if IsWindowVisible(h).as_bool() && GetWindowLongW(h, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0 {
                *(out.0 as *mut HWND) = h;
                return BOOL(0);
            }
        }
        BOOL(1)
    }
    let mut found = HWND(std::ptr::null_mut());
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(pick), LPARAM(&mut found as *mut HWND as isize));
    }
    (!found.0.is_null()).then_some(found)
}

/// `on_state` receives (maximized, hovering the maximize button, pressing it) whenever they change.
pub fn install(on_state: fn(bool, bool, bool)) -> bool {
    let Some(h) = find_window() else { return false };
    unsafe {
        STATE_CALLBACK = Some(on_state);
        if !SetWindowSubclass(h, Some(subclass), 1, 0).as_bool() {
            return false;
        }
        WINDOW = h;
        // Makes Windows ask WM_NCCALCSIZE again, now answered by `subclass`.
        let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
    }
    report();
    true
}

static mut STATE_CALLBACK: Option<fn(bool, bool, bool)> = None;
static mut THEME_CALLBACK: Option<fn()> = None;

/// `on_change` runs when Windows settings change, such as the light or dark app mode.
pub fn on_settings_change(on_change: fn()) {
    unsafe {
        THEME_CALLBACK = Some(on_change);
    }
}

fn report() {
    let Some(h) = hwnd() else { return };
    let maximized = unsafe { IsZoomed(h).as_bool() };
    if let Some(cb) = unsafe { STATE_CALLBACK } {
        cb(maximized, HOVER_MAX.load(Ordering::Relaxed), PRESSED_MAX.load(Ordering::Relaxed));
    }
}

fn toggle_maximize() {
    let Some(h) = hwnd() else { return };
    unsafe {
        let _ = ShowWindow(h, if IsZoomed(h).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
    }
}

fn frame_thickness(h: HWND) -> i32 {
    unsafe {
        let dpi = GetDpiForWindow(h);
        GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
}

fn hit_test(h: HWND, lparam: LPARAM) -> Option<u32> {
    let mut p = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
    let mut window = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut window);
        let _ = ScreenToClient(h, &mut p);
    }
    let scale = unsafe { GetDpiForWindow(h) } as f32 / 96.0;
    let width = window.right - window.left;
    let strip = STRIP_HEIGHT.load(Ordering::Relaxed) as f32 / 100.0 * scale;
    let drag_from = DRAG_FROM.load(Ordering::Relaxed) as f32 / 100.0 * scale;
    let button = CAPTION_BUTTON_WIDTH * scale;
    let maximized = unsafe { IsZoomed(h).as_bool() };
    let (x, y) = (p.x as f32, p.y as f32);

    if !maximized && p.y >= 0 && p.y < frame_thickness(h) / 2 {
        return Some(HTTOP);
    }
    if y < 0.0 || y >= strip {
        return None;
    }
    let max_left = width as f32 - 2.0 * button;
    if x >= max_left && x < max_left + button {
        return Some(HTMAXBUTTON);
    }
    // Full screen, minimize and close are ordinary buttons drawn by the window.
    if x >= width as f32 - 4.0 * button {
        return Some(HTCLIENT);
    }
    if x >= drag_from {
        return Some(HTCAPTION);
    }
    Some(HTCLIENT)
}

unsafe extern "system" fn subclass(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_DPICHANGED => {
                // winit sizes the window as if it still had a caption, so every monitor change would
                // add the caption height. Windows already worked out the right rectangle.
                let result = DefSubclassProc(h, msg, wparam, lparam);
                let suggested = *(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    h,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                return result;
            }
            WM_NCCALCSIZE if wparam.0 != 0 => {
                let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
                let top = params.rgrc[0].top;
                let result = DefSubclassProc(h, msg, wparam, lparam);
                // Keep the side and bottom borders Windows just computed, drop the caption.
                params.rgrc[0].top = top;
                if IsZoomed(h).as_bool() {
                    // A maximized window hangs over the monitor edge by its frame.
                    params.rgrc[0].top += frame_thickness(h);
                }
                return result;
            }
            WM_NCHITTEST => {
                let base = DefSubclassProc(h, msg, wparam, lparam);
                // Full screen removes the caption style; nothing there may move the window.
                if GetWindowLongW(h, GWL_STYLE) as u32 & WS_CAPTION.0 != WS_CAPTION.0 {
                    return base;
                }
                // Resize borders stay with Windows; its own caption buttons no longer exist here.
                let resize = (HTLEFT..=HTBOTTOMRIGHT).contains(&(base.0 as u32));
                if resize {
                    return base;
                }
                if let Some(hit) = hit_test(h, lparam) {
                    return LRESULT(hit as isize);
                }
                return LRESULT(HTCLIENT as isize);
            }
            WM_NCMOUSEMOVE => {
                let over = wparam.0 == HTMAXBUTTON as usize;
                if HOVER_MAX.swap(over, Ordering::Relaxed) != over {
                    report();
                }
            }
            WM_NCMOUSELEAVE => {
                HOVER_MAX.store(false, Ordering::Relaxed);
                PRESSED_MAX.store(false, Ordering::Relaxed);
                report();
            }
            WM_NCLBUTTONDOWN if wparam.0 == HTMAXBUTTON as usize => {
                PRESSED_MAX.store(true, Ordering::Relaxed);
                report();
                return LRESULT(0);
            }
            WM_NCLBUTTONUP if wparam.0 == HTMAXBUTTON as usize => {
                let was_pressed = PRESSED_MAX.swap(false, Ordering::Relaxed);
                report();
                if was_pressed {
                    toggle_maximize();
                }
                return LRESULT(0);
            }
            WM_SETTINGCHANGE => {
                // Windows announces a light or dark switch as a change to "ImmersiveColorSet".
                if let Some(cb) = THEME_CALLBACK {
                    cb();
                }
            }
            WM_SIZE => {
                let result = DefSubclassProc(h, msg, wparam, lparam);
                report();
                return result;
            }
            _ => {}
        }
        DefSubclassProc(h, msg, wparam, lparam)
    }
}
