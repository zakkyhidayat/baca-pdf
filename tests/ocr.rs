// SPDX-License-Identifier: GPL-3.0-or-later

//! The recognizer reads a drawn page. Skipped unless BACA_FORM_PDF names a PDF with text on page one.

use pdfium_render::prelude::*;

#[allow(dead_code)]
#[path = "../src/ocr.rs"]
mod ocr;

#[test]
fn reads_a_drawn_page() {
    let dll = std::env::var("BACA_PDFIUM").unwrap_or_else(|_| "target/release/pdfium.dll".into());
    let Ok(file) = std::env::var("BACA_FORM_PDF") else { return };
    let pdfium = Pdfium::new(Pdfium::bind_to_library(&dll).expect("pdfium.dll"));
    let document = pdfium.load_pdf_from_file(&file, None).expect("pdf");
    let page = document.pages().get(0).unwrap();
    let config = PdfRenderConfig::new().set_target_width(1800).set_format(PdfBitmapFormat::BGRA);
    let bitmap = page.render_with_config(&config).unwrap();
    let (w, h) = (bitmap.width() as u32, bitmap.height() as u32);
    let words = ocr::recognize(bitmap.as_raw_bytes().as_slice(), w, h).expect("ocr");
    println!("{} words, first: {:?}", words.len(), words.iter().take(8).map(|w| w.text.clone()).collect::<Vec<_>>());
    assert!(words.len() > 50);
}
