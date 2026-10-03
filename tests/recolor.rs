// SPDX-License-Identifier: GPL-3.0-or-later

//! Changing the color of a highlight that already exists must not crash PDFium, also when the
//! document is written afterwards. Skipped unless BACA_FORM_PDF names a PDF to try it on.

use pdfium_render::prelude::*;

#[test]
fn recolor_then_save() {
    let dll = std::env::var("BACA_PDFIUM").unwrap_or_else(|_| "target/release/pdfium.dll".into());
    let Ok(file) = std::env::var("BACA_FORM_PDF") else { return };
    let pdfium = Pdfium::new(Pdfium::bind_to_library(&dll).expect("pdfium.dll"));
    let document = pdfium.load_pdf_from_file(&file, None).expect("pdf");
    let mut page = document.pages().get(0).unwrap();
    {
        let mut a = page.annotations_mut().create_highlight_annotation().unwrap();
        a.set_bounds(PdfRect::new(PdfPoints::new(600.0), PdfPoints::new(100.0), PdfPoints::new(620.0), PdfPoints::new(300.0))).unwrap();
        a.attachment_points_mut()
            .create_attachment_point_at_end(PdfQuadPoints::new_from_values(100.0, 620.0, 300.0, 620.0, 100.0, 600.0, 300.0, 600.0))
            .unwrap();
        a.set_stroke_color(PdfColor::new(255, 233, 77, 255)).unwrap();
        let now = chrono::Utc::now();
        a.set_creator("zakky").unwrap();
        a.set_creation_date(now).unwrap();
        a.set_modification_date(now).unwrap();
    }
    println!("created");
    {
        let mut annotations = page.annotations_mut();
        let n = annotations.len();
        println!("annotations {n}");
        let mut a = annotations.get(n - 1).unwrap();
        a.set_stroke_color(PdfColor::new(120, 220, 120, 255)).unwrap();
        println!("recolored");
        let _ = a.set_modification_date(chrono::Utc::now());
        println!("dated");
        println!("creator {:?}", a.creator());
        println!("created {:?} modified {:?}", a.creation_date(), a.modification_date());
        println!("contents {:?}", a.contents());
    }
    let bitmap = page.render_with_config(&PdfRenderConfig::new().set_target_width(800).render_form_data(true)).unwrap();
    println!("rendered {}", bitmap.width());
    let text = page.text().unwrap();
    println!("text {}", text.all().len());
    for _ in 0..3 {
        let annotations = page.annotations();
        for i in 0..annotations.len() {
            let a = annotations.get(i).unwrap();
            println!("type {:?} creator {:?} bounds {:?}", a.annotation_type(), a.creator(), a.bounds().map(|b| b.left().value));
        }
    }
    drop(text);
    drop(bitmap);
    drop(page);
    let out = std::env::temp_dir().join("recolor-test.pdf");
    document.save_to_file(&out).unwrap();
    println!("saved to file");
    let bytes = document.save_to_bytes().unwrap();
    println!("saved {}", bytes.len());
}
