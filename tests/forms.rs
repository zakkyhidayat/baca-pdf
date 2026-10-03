// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks what the form fields code relies on: that a text value set through pdfium-render shows up
//! when the page is drawn again. Needs a form PDF and pdfium.dll; skipped when they are not around.

use pdfium_render::prelude::*;

#[path = "../src/forms.rs"]
mod forms;

#[test]
fn text_field_value_is_drawn() {
    let dll = std::env::var("BACA_PDFIUM").unwrap_or_else(|_| "target/release/pdfium.dll".into());
    let form = match std::env::var("BACA_FORM_PDF") {
        Ok(p) => p,
        Err(_) => return,
    };
    let pdfium = Pdfium::new(Pdfium::bind_to_library(&dll).expect("pdfium.dll"));
    let document = pdfium.load_pdf_from_file(&form, None).expect("form pdf");
    let config = PdfRenderConfig::new().set_target_width(800).render_form_data(true);
    let dark = |document: &PdfDocument| {
        let page = document.pages().get(0).unwrap();
        let bitmap = page.render_with_config(&config).unwrap();
        let image = bitmap.as_image().unwrap().to_rgba8();
        // The name field sits at 150..400 x 60..90 points on a 595 point wide page.
        let k = 800.0 / 595.0;
        let (x0, x1, y0, y1) = ((152.0 * k) as u32, (398.0 * k) as u32, (62.0 * k) as u32, (88.0 * k) as u32);
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = image.get_pixel(x, y);
                if p[0] < 100 && p[1] < 100 && p[2] < 100 {
                    n += 1;
                }
            }
        }
        n
    };
    let before = dark(&document);
    // The first annotation of the page is the name field.
    let bytes = forms::fill_text(&document, None, 0, 0, "Zakky Hidayat").expect("fill");
    let filled = pdfium.load_pdf_from_byte_vec(bytes, None).unwrap();
    let after = dark(&filled);
    println!("dark pixels before {before}, after {after}");
    assert!(after > before + 20, "the new value was not drawn");
}
