// SPDX-License-Identifier: GPL-3.0-or-later

//! Filling in a form text field the way PDFium's own form filler does it: click into the field, type,
//! leave it. That is what makes PDFium draw the new value; setting the value in the file alone leaves
//! the old appearance on the page. The work is done on a copy of the document made in memory, and the
//! changed copy is returned as bytes for the caller to open in place of the original.
//!
//! pdfium-render keeps the form functions to itself, so they are taken straight from the library that
//! is already loaded in the process.

use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_int, c_ulong};

use pdfium_render::prelude::*;
use windows::core::{s, w};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

type Handle = *mut c_void;

/// Shaped like PDFium's FPDF_FILEWRITE, with one extra field after it for the bytes being collected.
#[repr(C)]
struct FileWrite {
    version: c_int,
    write_block: unsafe extern "C" fn(*mut FileWrite, *const c_void, c_ulong) -> c_int,
    out: *mut Vec<u8>,
}

unsafe extern "C" fn write_block(this: *mut FileWrite, data: *const c_void, size: c_ulong) -> c_int {
    unsafe {
        let out = &mut *(*this).out;
        out.extend_from_slice(std::slice::from_raw_parts(data as *const u8, size as usize));
    }
    1
}

struct Lib {
    load_mem_document: unsafe extern "C" fn(*const c_void, c_int, *const c_char) -> Handle,
    close_document: unsafe extern "C" fn(Handle),
    init_form: unsafe extern "C" fn(Handle, *mut c_void) -> Handle,
    exit_form: unsafe extern "C" fn(Handle),
    load_page: unsafe extern "C" fn(Handle, c_int) -> Handle,
    close_page: unsafe extern "C" fn(Handle),
    after_load_page: unsafe extern "C" fn(Handle, Handle),
    before_close_page: unsafe extern "C" fn(Handle, Handle),
    mouse: unsafe extern "C" fn(Handle, Handle, c_int, f64, f64) -> c_int,
    left_down: unsafe extern "C" fn(Handle, Handle, c_int, f64, f64) -> c_int,
    left_up: unsafe extern "C" fn(Handle, Handle, c_int, f64, f64) -> c_int,
    select_all: unsafe extern "C" fn(Handle, Handle) -> c_int,
    replace: unsafe extern "C" fn(Handle, Handle, *const u16),
    kill_focus: unsafe extern "C" fn(Handle) -> c_int,
    set_index: unsafe extern "C" fn(Handle, Handle, c_int, c_int) -> c_int,
    save_as_copy: unsafe extern "C" fn(Handle, *mut FileWrite, c_ulong) -> c_int,
}

// The function types come from the fields of `Lib`, so the transmutes need no annotation.
#[allow(clippy::missing_transmute_annotations)]
fn lib() -> Result<Lib, String> {
    unsafe {
        let module = GetModuleHandleW(w!("pdfium.dll")).map_err(|_| "pdfium.dll is not loaded.".to_string())?;
        macro_rules! get {
            ($name:expr) => {
                match GetProcAddress(module, $name) {
                    Some(f) => std::mem::transmute(f),
                    None => return Err("This pdfium.dll cannot fill in forms.".to_string()),
                }
            };
        }
        Ok(Lib {
            load_mem_document: get!(s!("FPDF_LoadMemDocument")),
            close_document: get!(s!("FPDF_CloseDocument")),
            init_form: get!(s!("FPDFDOC_InitFormFillEnvironment")),
            exit_form: get!(s!("FPDFDOC_ExitFormFillEnvironment")),
            load_page: get!(s!("FPDF_LoadPage")),
            close_page: get!(s!("FPDF_ClosePage")),
            after_load_page: get!(s!("FORM_OnAfterLoadPage")),
            before_close_page: get!(s!("FORM_OnBeforeClosePage")),
            mouse: get!(s!("FORM_OnMouseMove")),
            left_down: get!(s!("FORM_OnLButtonDown")),
            left_up: get!(s!("FORM_OnLButtonUp")),
            select_all: get!(s!("FORM_SelectAllText")),
            replace: get!(s!("FORM_ReplaceSelection")),
            kill_focus: get!(s!("FORM_ForceToKillFocus")),
            set_index: get!(s!("FORM_SetIndexSelected")),
            save_as_copy: get!(s!("FPDF_SaveAsCopy")),
        })
    }
}

/// Puts `text` into the form field that is annotation `annotation` of page `page`. Returns the
/// document with the change in it.
pub fn fill_text(document: &PdfDocument, password: Option<&str>, page: usize, annotation: usize, text: &str) -> Result<Vec<u8>, String> {
    fill(document, password, page, annotation, Some(text), None)
}

/// Picks entry number `choice` (from 0) of the drop-down or list that is annotation `annotation`.
pub fn fill_choice(document: &PdfDocument, password: Option<&str>, page: usize, annotation: usize, choice: i32) -> Result<Vec<u8>, String> {
    fill(document, password, page, annotation, None, Some(choice))
}

fn fill(document: &PdfDocument, password: Option<&str>, page: usize, annotation: usize, text: Option<&str>, choice: Option<i32>) -> Result<Vec<u8>, String> {
    // The middle of the field, in page coordinates.
    let (x, y) = {
        let p = document.pages().get(page as _).map_err(|e| e.to_string())?;
        let annotations = p.annotations();
        let b = annotations.get(annotation).map_err(|e| e.to_string())?.bounds().map_err(|e| e.to_string())?;
        (((b.left().value + b.right().value) / 2.0) as f64, ((b.top().value + b.bottom().value) / 2.0) as f64)
    };
    let bytes = document.save_to_bytes().map_err(|e| e.to_string())?;
    let f = lib()?;
    let wide: Vec<u16> = text.unwrap_or("").encode_utf16().chain(std::iter::once(0)).collect();
    let password = password.and_then(|p| CString::new(p).ok());
    let mut saved: Vec<u8> = Vec::new();
    unsafe {
        let raw = (f.load_mem_document)(bytes.as_ptr() as *const c_void, bytes.len() as c_int, password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()));
        if raw.is_null() {
            return Err("The form could not be opened to fill it in.".into());
        }
        // The form filler needs this structure to exist; every callback in it may stay empty. Its
        // first field is the version.
        let mut info = vec![0u64; 64];
        (info.as_mut_ptr() as *mut c_int).write(2);
        let form = (f.init_form)(raw, info.as_mut_ptr() as *mut c_void);
        if form.is_null() {
            (f.close_document)(raw);
            return Err("The form could not be opened to fill it in.".into());
        }
        let pg = (f.load_page)(raw, page as c_int);
        if pg.is_null() {
            (f.exit_form)(form);
            (f.close_document)(raw);
            return Err("The page could not be opened to fill it in.".into());
        }
        (f.after_load_page)(pg, form);
        (f.mouse)(form, pg, 0, x, y);
        (f.left_down)(form, pg, 0, x, y);
        (f.left_up)(form, pg, 0, x, y);
        if let Some(index) = choice {
            (f.set_index)(form, pg, index, 1);
        } else {
            (f.select_all)(form, pg);
            (f.replace)(form, pg, wide.as_ptr());
        }
        (f.kill_focus)(form);
        (f.before_close_page)(pg, form);
        (f.close_page)(pg);
        let mut writer = FileWrite { version: 1, write_block, out: &mut saved };
        let ok = (f.save_as_copy)(raw, &mut writer, 0);
        (f.exit_form)(form);
        (f.close_document)(raw);
        // `info` is read by PDFium until the form environment is gone.
        drop(info);
        if ok == 0 || saved.is_empty() {
            return Err("The filled-in form could not be saved.".into());
        }
    }
    Ok(saved)
}
