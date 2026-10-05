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

#[test]
fn flatten_copy_keeps_the_look_without_the_annotation() {
    let dll = std::env::var("BACA_PDFIUM").unwrap_or_else(|_| "target/release/pdfium.dll".into());
    let Ok(file) = std::env::var("BACA_FORM_PDF") else { return };
    let pdfium = Pdfium::new(Pdfium::bind_to_library(&dll).expect("pdfium.dll"));
    let document = pdfium.load_pdf_from_file(&file, None).expect("pdf");
    {
        let mut page = document.pages().get(0).unwrap();
        let mut a = page.annotations_mut().create_highlight_annotation().unwrap();
        a.set_bounds(PdfRect::new(PdfPoints::new(600.0), PdfPoints::new(100.0), PdfPoints::new(620.0), PdfPoints::new(300.0))).unwrap();
        a.attachment_points_mut()
            .create_attachment_point_at_end(PdfQuadPoints::new_from_values(100.0, 620.0, 300.0, 620.0, 100.0, 600.0, 300.0, 600.0))
            .unwrap();
        a.set_stroke_color(PdfColor::new(255, 233, 77, 255)).unwrap();
    }
    // The page was shown before, which is when PDFium gives an annotation its appearance.
    let before = document.pages().get(0).unwrap().render_with_config(&PdfRenderConfig::new().set_target_width(595)).unwrap().as_image().unwrap().to_rgba8();
    let yellow = |img: &image::RgbaImage| img.pixels().filter(|p| p[0] > 200 && p[1] > 200 && p[2] < 160).count();
    println!("yellow pixels in the original render: {}", yellow(&before));
    let bytes = document.save_to_bytes().unwrap();
    std::fs::write(std::env::temp_dir().join("flat-before.pdf"), &bytes).unwrap();
    let copy = pdfium.load_pdf_from_byte_vec(bytes, None).unwrap();
    for mut page in copy.pages().iter() {
        for i in 0..page.annotations().len() {
            let mut annotations = page.annotations_mut();
            annotations.get(i).unwrap().set_is_printed(true).unwrap();
        }
        // Drawing the page once gives annotations that have no appearance yet one to flatten.
        let _ = page.render_with_config(&PdfRenderConfig::new().set_target_width(64)).unwrap();
        page.flatten().unwrap();
    }
    let flat_bytes = copy.save_to_bytes().unwrap();
    std::fs::write(std::env::temp_dir().join("flat-after.pdf"), &flat_bytes).unwrap();
    let flat = pdfium.load_pdf_from_byte_vec(flat_bytes, None).unwrap();
    let page = flat.pages().get(0).unwrap();
    let left = page.annotations().len();
    let image = page.render_with_config(&PdfRenderConfig::new().set_target_width(595)).unwrap().as_image().unwrap().to_rgba8();
    // The highlight sits at y 172..192 from the top of a 792 point page, x 100..300.
    let p = image.get_pixel(200, 182);
    println!("annotations left {left}, pixel {:?}, yellow pixels after flatten {}", p, yellow(&image));
    assert_eq!(left, 0);
    assert!(yellow(&image) > 500, "the yellow should be part of the page");
}
