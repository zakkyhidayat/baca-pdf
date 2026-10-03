// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

use windows::core::w;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, SetCurrentProcessExplicitAppUserModelID, FOS_FILEMUSTEXIST,
    FOS_FORCEFILESYSTEM, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CONVERTIBLESLATEMODE, SM_MAXIMUMTOUCHES, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
};

use crate::store::Placement;

pub fn set_app_id() {
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(w!("BacaPDF.Reader"));
    }
}

/// True on a touch screen that is currently in tablet posture.
pub fn prefers_touch() -> bool {
    unsafe { GetSystemMetrics(SM_MAXIMUMTOUCHES) > 0 && GetSystemMetrics(SM_CONVERTIBLESLATEMODE) == 0 }
}

pub fn pick_pdf(start: Option<&std::path::Path>) -> Option<PathBuf> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [
            COMDLG_FILTERSPEC { pszName: w!("PDF documents"), pszSpec: w!("*.pdf") },
            COMDLG_FILTERSPEC { pszName: w!("All files"), pszSpec: w!("*.*") },
        ];
        dialog.SetFileTypes(&filters).ok()?;
        dialog.SetTitle(w!("Open PDF")).ok()?;
        if let Some(folder) = start {
            let text = windows::core::HSTRING::from(folder.to_string_lossy().as_ref());
            let made: windows::core::Result<windows::Win32::UI::Shell::IShellItem> = windows::Win32::UI::Shell::SHCreateItemFromParsingName(&text, None);
            if let Ok(item) = made {
                let _ = dialog.SetFolder(&item);
            }
        }
        let options = dialog.GetOptions().ok()?;
        dialog.SetOptions(options | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM).ok()?;
        dialog.Show(Some(GetActiveWindow())).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as _));
        path.map(PathBuf::from)
    }
}

/// A question with Yes and No, in the system's own message box.
pub fn ask_yes_no(title: &str, text: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONQUESTION, MB_YESNO};
    unsafe { MessageBoxW(Some(GetActiveWindow()), &HSTRING::from(text), &HSTRING::from(title), MB_YESNO | MB_ICONQUESTION) == IDYES }
}

/// Border and title bar thickness around a client area, at the system DPI.
fn frame() -> (i32, i32, i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0), GetDpiForSystem());
    }
    (-r.left, -r.top, r.right, r.bottom)
}

fn work_area_near(x: i32, y: i32, w: i32, h: i32) -> RECT {
    unsafe {
        let monitor = MonitorFromRect(&RECT { left: x, top: y, right: x + w, bottom: y + h }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        info.rcWork
    }
}

/// Moves and shrinks a window so all of it, title bar included, sits inside one monitor's work area.
pub fn keep_on_screen(p: Placement) -> Placement {
    let (fl, ft, fr, fb) = frame();
    let outer_w = p.width as i32 + fl + fr;
    let outer_h = p.height as i32 + ft + fb;
    let work = work_area_near(p.x, p.y, outer_w, outer_h);
    let w = outer_w.min(work.right - work.left);
    let h = outer_h.min(work.bottom - work.top);
    let x = p.x.clamp(work.left, work.right - w);
    let y = p.y.clamp(work.top, work.bottom - h);
    Placement { x, y, width: (w - fl - fr).max(1) as u32, height: (h - ft - fb).max(1) as u32, maximized: p.maximized }
}

/// The first-run window: 1100 x 820 at the system scale, centred, and never larger than the screen.
pub fn default_placement() -> Placement {
    let scale = unsafe { GetDpiForSystem() } as f32 / 96.0;
    let work = work_area_near(0, 0, 1, 1);
    let (fl, ft, fr, fb) = frame();
    let (ww, wh) = (work.right - work.left, work.bottom - work.top);
    let width = ((1100.0 * scale) as i32).min(ww * 92 / 100 - fl - fr);
    let height = ((820.0 * scale) as i32).min(wh * 92 / 100 - ft - fb);
    let x = work.left + (ww - width - fl - fr) / 2;
    let y = work.top + (wh - height - ft - fb) / 2;
    Placement { x, y, width: width as u32, height: height as u32, maximized: false }
}
/// Puts plain text on the Windows clipboard.
pub fn copy_text(text: &str) -> bool {
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let ok = (|| -> Option<()> {
            let memory: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).ok()?;
            let target = GlobalLock(memory) as *mut u16;
            if target.is_null() {
                let _ = GlobalFree(Some(memory));
                return None;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
            let _ = GlobalUnlock(memory);
            // The clipboard owns the memory once this succeeds.
            if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(memory.0))).is_err() {
                let _ = GlobalFree(Some(memory));
                return None;
            }
            Some(())
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
    }
}
/// True when Windows apps are set to dark mode.
pub fn system_dark() -> bool {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    ok.is_ok() && value == 0
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Opens a web address or a file with its default program.
pub fn shell_open(target: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let target = wide(target);
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(target.as_ptr()), None, None, SW_SHOWNORMAL);
    }
}

/// Opens Explorer with the file selected.
pub fn show_in_folder(path: &std::path::Path) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let args = wide(&format!("/select,\"{}\"", path.display()));
    unsafe {
        ShellExecuteW(None, w!("open"), w!("explorer.exe"), PCWSTR(args.as_ptr()), None, SW_SHOWNORMAL);
    }
}

/// Asks where to save a copy, starting from `suggested`.
pub fn pick_save_path(suggested: &str) -> Option<PathBuf> {
    // Lets automated tests choose the file without the dialog.
    if let Some(path) = std::env::var_os("BACA_TEST_SAVE_TO") {
        return Some(PathBuf::from(path));
    }
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, FOS_OVERWRITEPROMPT};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [COMDLG_FILTERSPEC { pszName: w!("PDF documents"), pszSpec: w!("*.pdf") }];
        dialog.SetFileTypes(&filters).ok()?;
        dialog.SetDefaultExtension(w!("pdf")).ok()?;
        dialog.SetTitle(w!("Save a copy")).ok()?;
        let name = wide(suggested);
        dialog.SetFileName(PCWSTR(name.as_ptr())).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog.SetOptions(options | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM).ok()?;
        dialog.Show(Some(GetActiveWindow())).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as _));
        path.map(PathBuf::from)
    }
}

/// The Windows print dialog. Returns the printer's device context and the 0-based pages to print.
pub fn print_dialog(page_count: usize, current: usize) -> Option<(isize, Vec<usize>)> {
    use windows::Win32::UI::Controls::Dialogs::{
        PrintDlgW, PD_ALLPAGES, PD_CURRENTPAGE, PD_NOSELECTION, PD_PAGENUMS, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE,
        PRINTDLGW,
    };
    let mut dlg = PRINTDLGW {
        lStructSize: std::mem::size_of::<PRINTDLGW>() as u32,
        hwndOwner: unsafe { GetActiveWindow() },
        Flags: PD_RETURNDC | PD_USEDEVMODECOPIESANDCOLLATE | PD_NOSELECTION | PD_ALLPAGES,
        nFromPage: 1,
        nToPage: page_count.min(u16::MAX as usize) as u16,
        nMinPage: 1,
        nMaxPage: page_count.min(u16::MAX as usize) as u16,
        nCopies: 1,
        ..Default::default()
    };
    unsafe {
        if !PrintDlgW(&mut dlg).as_bool() || dlg.hDC.is_invalid() {
            return None;
        }
    }
    let pages: Vec<usize> = if dlg.Flags.contains(PD_PAGENUMS) {
        let from = (dlg.nFromPage as usize).max(1);
        let to = (dlg.nToPage as usize).clamp(from, page_count);
        (from - 1..to).collect()
    } else if dlg.Flags.contains(PD_CURRENTPAGE) {
        vec![current]
    } else {
        (0..page_count).collect()
    };
    Some((dlg.hDC.0 as isize, pages))
}

/// The Windows text-to-speech voice.
pub struct Speaker {
    voice: windows::Win32::Media::Speech::ISpVoice,
}

impl Speaker {
    pub fn new() -> Option<Self> {
        use windows::Win32::Media::Speech::{ISpVoice, SpVoice};
        use windows::Win32::System::Com::CLSCTX_ALL;
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let voice: ISpVoice = CoCreateInstance(&SpVoice, None, CLSCTX_ALL).ok()?;
            Some(Self { voice })
        }
    }

    fn flags(purge: bool) -> u32 {
        use windows::Win32::Media::Speech::{SPF_ASYNC, SPF_IS_NOT_XML, SPF_PURGEBEFORESPEAK};
        let mut flags = (SPF_ASYNC.0 | SPF_IS_NOT_XML.0) as u32;
        if purge {
            flags |= SPF_PURGEBEFORESPEAK.0 as u32;
        }
        flags
    }

    pub fn speak(&self, text: &str) {
        use windows::core::PCWSTR;
        let text = wide(text);
        unsafe {
            let _ = self.voice.Speak(PCWSTR(text.as_ptr()), Self::flags(true), None);
        }
    }

    pub fn stop(&self) {
        use windows::core::PCWSTR;
        let empty = wide("");
        unsafe {
            let _ = self.voice.Speak(PCWSTR(empty.as_ptr()), Self::flags(true), None);
        }
    }

    /// Whether the voice is still talking.
    pub fn busy(&self) -> bool {
        use windows::core::PWSTR;
        use windows::Win32::Media::Speech::SPVOICESTATUS;
        let mut status = SPVOICESTATUS::default();
        let mut bookmark = PWSTR::null();
        // 2 is SPRS_IS_SPEAKING.
        unsafe { self.voice.GetStatus(&mut status, &mut bookmark).is_ok() && status.dwRunningState & 2 != 0 }
    }
}

/// The two-letter language Windows is set to, such as "en" or "id".
pub fn system_language() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buffer = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    if n <= 0 {
        return "en".into();
    }
    let name = String::from_utf16_lossy(&buffer[..(n as usize).saturating_sub(1)]);
    let code = name.split('-').next().unwrap_or("en");
    if code.is_empty() { "en".into() } else { code.to_ascii_lowercase() }
}

fn set_value(key: &str, name: &str, value: &str) {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let key = HSTRING::from(key);
    let name = HSTRING::from(name);
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &key, if name.is_empty() { windows::core::PCWSTR::null() } else { windows::core::PCWSTR(name.as_ptr()) }, REG_SZ.0, Some(data.as_ptr() as *const _), (data.len() * 2) as u32);
    }
}

/// Lists the program among the ones Windows offers for .pdf files, for this user only. Making it the
/// default is still the user's choice in Windows Settings.
pub fn register_file_type() {
    if std::env::var_os("BACA_DATA_DIR").is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(file) = exe.file_name().map(|f| f.to_string_lossy().into_owned()) else { return };
    let exe = exe.to_string_lossy().into_owned();
    let command = format!("\"{exe}\" \"%1\"");
    let class = "Software\\Classes\\BacaPDF.Document";
    set_value(class, "", "PDF document");
    set_value(&format!("{class}\\DefaultIcon"), "", &format!("\"{exe}\",0"));
    set_value(&format!("{class}\\shell\\open\\command"), "", &command);
    set_value("Software\\Classes\\.pdf\\OpenWithProgids", "BacaPDF.Document", "");
    let app = format!("Software\\Classes\\Applications\\{file}");
    set_value(&app, "FriendlyAppName", "Baca PDF");
    set_value(&format!("{app}\\shell\\open\\command"), "", &command);
    set_value(&format!("{app}\\SupportedTypes"), ".pdf", "");
}
