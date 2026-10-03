// SPDX-License-Identifier: GPL-3.0-or-later

//! Draws a typed name in a handwriting font, as a picture with a transparent background.

use image::{DynamicImage, RgbaImage};
use windows::core::HSTRING;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GdiFlush, GetTextExtentPoint32W, SelectObject,
    SetBkMode, SetTextColor, TextOutW, ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DIB_RGB_COLORS, OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::Foundation::SIZE;

/// Handwriting faces that ship with Windows. The sign dialog previews them under the same names.
pub const FONTS: [&str; 3] = ["Segoe Script", "Ink Free", "Gabriola"];

/// The name as ink of the given color on a clear background, cut close to the strokes.
pub fn render(text: &str, font: u8, color: [u8; 3]) -> Result<DynamicImage, String> {
    let wide: Vec<u16> = text.trim().encode_utf16().collect();
    if wide.is_empty() {
        return Err("Type a name first.".into());
    }
    let face = HSTRING::from(FONTS[(font as usize).min(FONTS.len() - 1)]);
    unsafe {
        let dc = CreateCompatibleDC(None);
        let hfont = CreateFontW(
            -160,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            &face,
        );
        let old_font = SelectObject(dc, hfont.into());
        let mut size = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, &wide, &mut size);
        let pad = 40;
        let (w, h) = (size.cx + pad * 2, size.cy + pad * 2);
        if w <= 0 || h <= 0 || w > 12_000 || h > 2_000 {
            let _ = DeleteObject(SelectObject(dc, old_font));
            let _ = DeleteDC(dc);
            return Err("That name is too long to draw.".into());
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0);
        let Ok(bitmap) = bitmap else {
            let _ = DeleteObject(SelectObject(dc, old_font));
            let _ = DeleteDC(dc);
            return Err("The signature could not be drawn.".into());
        };
        let old_bitmap = SelectObject(dc, bitmap.into());
        let bytes = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        bytes.fill(0xFF);
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0));
        let _ = TextOutW(dc, pad, pad, &wide);
        let _ = GdiFlush();

        // Black text on white becomes opaque ink on nothing: the darker the pixel, the more ink.
        let (mut l, mut t, mut r, mut b) = (w, h, 0, 0);
        let mut out = RgbaImage::new(w as u32, h as u32);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let dark = 255 - ((bytes[i] as u32 + bytes[i + 1] as u32 + bytes[i + 2] as u32) / 3) as u8;
                if dark > 8 {
                    l = l.min(x);
                    r = r.max(x);
                    t = t.min(y);
                    b = b.max(y);
                }
                out.put_pixel(x as u32, y as u32, image::Rgba([color[0], color[1], color[2], dark]));
            }
        }
        let _ = DeleteObject(SelectObject(dc, old_bitmap));
        let _ = DeleteObject(SelectObject(dc, old_font));
        let _ = DeleteDC(dc);
        if r < l || b < t {
            return Err("Nothing could be drawn for that name.".into());
        }
        let margin = 6;
        let (x0, y0) = ((l - margin).max(0), (t - margin).max(0));
        let (x1, y1) = ((r + margin).min(w - 1), (b + margin).min(h - 1));
        let cut = image::imageops::crop_imm(&out, x0 as u32, y0 as u32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32).to_image();
        Ok(DynamicImage::ImageRgba8(cut))
    }
}
