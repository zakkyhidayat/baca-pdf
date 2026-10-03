// SPDX-License-Identifier: GPL-3.0-or-later

//! The drawing tools (highlight, draw, erase, text), read aloud and translate, and the question
//! asked before changes are thrown away.

use std::path::PathBuf;

use slint::ComponentHandle;

use crate::render::{EditJob, FieldValue, StampKind};
use crate::tabs::Status;
use crate::*;

pub(crate) const HIGHLIGHT_COLORS: [[u8; 3]; 5] =
    [[255, 233, 77], [126, 231, 135], [102, 217, 239], [255, 143, 208], [255, 180, 84]];
pub(crate) const DRAW_COLORS: [[u8; 3]; 5] = [[232, 17, 35], [27, 27, 27], [0, 99, 177], [16, 137, 62], [255, 140, 0]];
/// Line widths in PDF points: thin, medium, thick.
pub(crate) const DRAW_WIDTHS: [f32; 3] = [1.5, 3.0, 6.0];
const TEXT_SIZES: [f32; 7] = [10.0, 12.0, 14.0, 18.0, 24.0, 32.0, 48.0];

/// A line being drawn, in content coordinates.
pub(crate) struct Stroke {
    page: usize,
    points: Vec<[f32; 2]>,
    width: f32,
    /// 0 for a freehand line, otherwise a shape (see `Bridge.draw-shape`) dragged out from `start`.
    shape: i32,
    start: [f32; 2],
}

/// The outline of a shape dragged from `a` to `b`, as the points of one line.
fn shape_points(shape: i32, a: [f32; 2], b: [f32; 2]) -> Vec<[f32; 2]> {
    match shape {
        1 => vec![a, [b[0], a[1]], b, [a[0], b[1]], a],
        2 => {
            let (cx, cy) = ((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0);
            let (rx, ry) = ((b[0] - a[0]).abs() / 2.0, (b[1] - a[1]).abs() / 2.0);
            (0..=48)
                .map(|i| {
                    let t = i as f32 / 48.0 * std::f32::consts::TAU;
                    [cx + rx * t.cos(), cy + ry * t.sin()]
                })
                .collect()
        }
        4 => {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = dx.hypot(dy).max(1.0);
            let head = (len * 0.4).min(16.0);
            let back = dy.atan2(dx) + std::f32::consts::PI;
            let wing = |turn: f32| [b[0] + head * (back + turn).cos(), b[1] + head * (back + turn).sin()];
            vec![a, b, wing(0.45), b, wing(-0.45)]
        }
        _ => vec![a, b],
    }
}

/// A drawing, shape or note picked on the page: its place as fractions of the page (left, top, right, bottom).
#[derive(Clone)]
pub(crate) struct Selected {
    page: usize,
    index: usize,
    kind: &'static str,
    rect: [f32; 4],
    text: String,
}

/// A press on the picked annotation: 1 moves it, 2 to 5 pull the top left, top right, bottom left or bottom right corner.
pub(crate) struct AnnotDrag {
    mode: i32,
    start: [f32; 2],
    orig: [f32; 4],
    moved: bool,
}

/// Whether a point (as fractions of the page) is on an annotation. Big drawings only answer near their edge,
/// so a box drawn around a paragraph does not stop the text inside from being selected.
fn annot_hit(kind: &str, r: &[f32; 4], f: [f32; 2], page: [f32; 2], inside_ok: bool) -> bool {
    let (sx, sy) = (8.0 / page[0], 8.0 / page[1]);
    let outer = f[0] >= r[0] - sx && f[0] <= r[2] + sx && f[1] >= r[1] - sy && f[1] <= r[3] + sy;
    if !outer {
        return false;
    }
    let small = (r[2] - r[0]) * page[0] < 48.0 || (r[3] - r[1]) * page[1] < 48.0;
    if matches!(kind, "Note" | "Image" | "Text") || small || inside_ok {
        return true;
    }
    let inner = f[0] > r[0] + sx && f[0] < r[2] - sx && f[1] > r[1] + sy && f[1] < r[3] - sy;
    !inner
}

/// Where the text being typed will land.
pub(crate) struct TextTarget {
    page: usize,
    point: [f32; 2],
}

/// What to do once the user has decided about unsaved changes.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Pending {
    CloseTab(u64),
    CloseOthers(u64),
    CloseWindow,
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Stroke {
    /// Draws the line as a small element around it; a path the size of the whole page is too heavy.
    fn show(&self, ui: &AppWindow) {
        let pad = self.width + 2.0;
        let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &self.points {
            l = l.min(p[0]);
            r = r.max(p[0]);
            t = t.min(p[1]);
            b = b.max(p[1]);
        }
        let (l, t) = (l - pad, t - pad);
        let mut path = String::new();
        for (i, p) in self.points.iter().enumerate() {
            let cmd = if i == 0 { 'M' } else { 'L' };
            path.push_str(&format!("{cmd} {:.1} {:.1} ", p[0] - l, p[1] - t));
        }
        if self.points.len() == 1 {
            path.push_str(&format!("L {:.1} {:.1}", self.points[0][0] - l + 0.1, self.points[0][1] - t));
        }
        let bridge = ui.global::<Bridge>();
        bridge.set_stroke_x(l);
        bridge.set_stroke_y(t);
        bridge.set_stroke_w(r - l + 2.0 * pad);
        bridge.set_stroke_h(b - t + pad);
        bridge.set_stroke_path(path.into());
    }
}

impl Viewer {
    pub(crate) fn active_id(&self) -> Option<u64> {
        self.active.map(|a| self.tabs[a].id)
    }

    /// A tool was picked or dropped: whatever the old one had in progress goes away.
    pub(crate) fn tool_changed(&mut self, ui: &AppWindow) {
        self.stroke = None;
        self.text_target = None;
        let b = ui.global::<Bridge>();
        b.set_stroke_path("".into());
        b.set_text_open(false);
        self.clear_selection(ui);
        self.deselect(ui);
        self.note_edit_close(ui);
    }

    /// Turns the text selected while the highlighter was on into highlights.
    pub(crate) fn annotate_selection(&mut self, ui: &AppWindow) {
        let Some(doc) = self.active_id() else { return };
        let Some(sel) = self.selection.as_ref().filter(|s| !s.pieces.is_empty()) else { return };
        let style = ui.global::<Bridge>().get_mark_style().clamp(0, 2) as u8;
        // Underlines and strikethroughs are plain black lines; only highlights take a color.
        let color = if style == 0 { HIGHLIGHT_COLORS[(ui.global::<Bridge>().get_highlight_color().max(0) as usize).min(4)] } else { [0, 0, 0] };
        self.renderer.edit(EditJob::Highlight { doc, pieces: sel.pieces.clone(), turns: self.turns, color, style });
        self.clear_selection(ui);
    }

    pub(crate) fn stroke_begin(&mut self, ui: &AppWindow, x: f32, y: f32) {
        let Some(page) = self.page_near(x, y) else { return };
        let width_px = DRAW_WIDTHS[(ui.global::<Bridge>().get_draw_width().max(0) as usize).min(2)] * self.scale_of_page(ui, page);
        ui.global::<Bridge>().set_stroke_width(width_px.max(1.0));
        let shape = ui.global::<Bridge>().get_draw_shape();
        let stroke = Stroke { page, points: vec![[x, y]], width: width_px.max(1.0), shape, start: [x, y] };
        stroke.show(ui);
        self.stroke = Some(stroke);
    }

    pub(crate) fn stroke_point(&mut self, ui: &AppWindow, x: f32, y: f32) {
        let Some(stroke) = self.stroke.as_mut() else { return };
        if let Some(last) = stroke.points.last() {
            if (last[0] - x).hypot(last[1] - y) < 1.5 {
                return;
            }
        }
        if stroke.shape > 0 {
            stroke.points = shape_points(stroke.shape, stroke.start, [x, y]);
        } else {
            stroke.points.push([x, y]);
        }
        stroke.show(ui);
    }

    pub(crate) fn stroke_end(&mut self, ui: &AppWindow) {
        let Some(stroke) = self.stroke.take() else { return };
        ui.global::<Bridge>().set_stroke_path("".into());
        let (Some(doc), Some([px, py, w, h])) = (self.active_id(), self.page_rect(stroke.page)) else { return };
        let points = stroke
            .points
            .iter()
            .map(|p| [((p[0] - px) / w).clamp(0.0, 1.0), ((p[1] - py) / h).clamp(0.0, 1.0)])
            .collect();
        let b = ui.global::<Bridge>();
        let color = DRAW_COLORS[(b.get_draw_color().max(0) as usize).min(4)];
        let width = DRAW_WIDTHS[(b.get_draw_width().max(0) as usize).min(2)];
        self.renderer.edit(EditJob::Ink { doc, page: stroke.page, turns: self.turns, points, color, width });
    }

    fn fraction_at(&self, x: f32, y: f32) -> Option<(usize, [f32; 2])> {
        let page = self.page_near(x, y)?;
        let [px, py, w, h] = self.page_rect(page)?;
        if x < px || y < py || x > px + w || y > py + h {
            return None;
        }
        Some((page, [(x - px) / w, (y - py) / h]))
    }

    pub(crate) fn erase_at(&mut self, x: f32, y: f32) {
        let Some(doc) = self.active_id() else { return };
        let Some((page, point)) = self.fraction_at(x, y) else { return };
        self.renderer.edit(EditJob::Erase { doc, page, turns: self.turns, point });
    }

    /// Page menu: type text at the spot that was right-clicked.
    pub(crate) fn text_here(&mut self, ui: &AppWindow) {
        let Some((x, y)) = self.menu_point else { return };
        self.text_at(ui, x, y);
    }

    /// Page menu: choose a picture and put it at the spot that was right-clicked.
    pub(crate) fn image_here(&mut self, ui: &AppWindow) {
        let Some((x, y)) = self.menu_point else { return };
        let Some((page, point)) = self.fraction_at(x, y) else { return };
        let Some(doc) = self.active_id() else { return };
        let Some(path) = platform::pick_image() else { return };
        self.renderer.edit(EditJob::Stamp { doc, page, turns: self.turns, point, kind: StampKind::Image(path) });
        self.notify(ui, "Picture added. Drag it to move it, pull a corner to resize it.");
    }

    /// Page menu: opens the sign dialog for the spot that was right-clicked.
    pub(crate) fn sign_here(&mut self, ui: &AppWindow) {
        let Some((x, y)) = self.menu_point else { return };
        let Some((page, point)) = self.fraction_at(x, y) else { return };
        self.text_target = Some(TextTarget { page, point });
        let b = ui.global::<Bridge>();
        if b.get_sign_name().is_empty() {
            let name = std::env::var("USERNAME").unwrap_or_default();
            b.set_sign_name(name.into());
        }
        b.set_sign_open(true);
    }

    pub(crate) fn sign_place(&mut self, ui: &AppWindow, text: String) {
        let b = ui.global::<Bridge>();
        let (Some(target), Some(doc)) = (self.text_target.take(), self.active_id()) else { return };
        if text.trim().is_empty() {
            self.text_target = Some(target);
            return;
        }
        b.set_sign_open(false);
        let color = [[27, 27, 27], [0, 99, 177], [26, 35, 126]][(b.get_sign_color().max(0) as usize).min(2)];
        let font = b.get_sign_font().clamp(0, 2) as u8;
        self.renderer.edit(EditJob::Stamp { doc, page: target.page, turns: self.turns, point: target.point, kind: StampKind::Signature { text, font, color } });
        self.notify(ui, "Signature added. Drag it to move it, pull a corner to resize it.");
    }

    pub(crate) fn text_at(&mut self, ui: &AppWindow, x: f32, y: f32) {
        let Some((page, point)) = self.fraction_at(x, y) else { return };
        self.text_target = Some(TextTarget { page, point });
        let b = ui.global::<Bridge>();
        b.set_text_note(false);
        b.set_text_x(x);
        b.set_text_y(y);
        b.set_text_open(true);
    }

    pub(crate) fn text_commit(&mut self, ui: &AppWindow, text: String) {
        ui.global::<Bridge>().set_text_open(false);
        let (Some(target), Some(doc)) = (self.text_target.take(), self.active_id()) else { return };
        let text = text.trim().to_string();
        let note = std::mem::take(&mut self.note_mode);
        if text.is_empty() {
            return;
        }
        if note {
            self.renderer.edit(EditJob::Note { doc, page: target.page, turns: self.turns, point: target.point, text });
            return;
        }
        let b = ui.global::<Bridge>();
        let color = DRAW_COLORS[(b.get_text_color().max(0) as usize).min(4)];
        let size = TEXT_SIZES[(b.get_text_size().max(0) as usize).min(TEXT_SIZES.len() - 1)];
        self.renderer.edit(EditJob::Stamp { doc, page: target.page, turns: self.turns, point: target.point, kind: StampKind::Text { text, size, color } });
    }

    /// Page menu: type a note at the spot that was right-clicked.
    pub(crate) fn note_here(&mut self, ui: &AppWindow) {
        let Some((x, y)) = self.menu_point else { return };
        let Some((page, point)) = self.fraction_at(x, y) else { return };
        self.note_mode = true;
        self.text_target = Some(TextTarget { page, point });
        let b = ui.global::<Bridge>();
        b.set_text_note(true);
        b.set_text_x(x);
        b.set_text_y(y);
        b.set_text_open(true);
    }

    pub(crate) fn text_cancel(&mut self, ui: &AppWindow) {
        self.note_mode = false;
        self.text_target = None;
        ui.global::<Bridge>().set_text_open(false);
    }

    /// A drawing, highlight, erase or text edit finished (or failed) on the PDFium thread.
    /// Asks the PDFium thread for the list of annotations the side panel shows.
    pub(crate) fn request_annotations(&self) {
        if let Some(doc) = self.active_id() {
            self.renderer.edit(EditJob::ListFields { doc });
            self.renderer.edit(EditJob::ListAnnotations { doc });
        }
    }

    fn content_rect(&self, page: usize, r: [f32; 4]) -> Option<[f32; 4]> {
        let [px, py, w, h] = self.page_rect(page)?;
        Some([px + r[0] * w, py + r[1] * h, (r[2] - r[0]) * w, (r[3] - r[1]) * h])
    }

    pub(crate) fn show_selection(&self, ui: &AppWindow) {
        let b = ui.global::<Bridge>();
        let shown = self.selected.as_ref().and_then(|s| self.content_rect(s.page, s.rect).map(|c| (s, c)));
        match shown {
            Some((s, [x, y, w, h])) => {
                b.set_sel_x(x);
                b.set_sel_y(y);
                b.set_sel_w(w);
                b.set_sel_h(h);
                b.set_sel_resizable(s.kind != "Note");
                b.set_sel_on(true);
            }
            None => b.set_sel_on(false),
        }
    }

    pub(crate) fn deselect(&mut self, ui: &AppWindow) {
        self.selected = None;
        self.annot_drag = None;
        self.show_selection(ui);
    }

    pub(crate) fn delete_selected(&mut self, ui: &AppWindow) {
        if let (Some(s), Some(doc)) = (self.selected.clone(), self.active_id()) {
            self.renderer.edit(EditJob::DeleteAnnotation { doc, page: s.page, index: s.index });
        }
        self.deselect(ui);
    }

    /// A press on the page with no tool: picks a drawing, shape or note, or one of the corners of the picked one.
    /// Returns what the press will do (see `AnnotDrag`), or 0 when it hit nothing.
    pub(crate) fn annot_press(&mut self, ui: &AppWindow, x: f32, y: f32) -> i32 {
        if self.turns % 4 != 0 || self.annots.is_empty() {
            return 0;
        }
        if let Some(s) = &self.selected {
            if s.kind != "Note" {
                if let Some([cx, cy, cw, ch]) = self.content_rect(s.page, s.rect) {
                    let corners = [(cx, cy), (cx + cw, cy), (cx, cy + ch), (cx + cw, cy + ch)];
                    for (i, (hx, hy)) in corners.iter().enumerate() {
                        if (x - hx).abs() <= 10.0 && (y - hy).abs() <= 10.0 {
                            self.annot_drag = Some(AnnotDrag { mode: 2 + i as i32, start: [x, y], orig: s.rect, moved: false });
                            return 2 + i as i32;
                        }
                    }
                }
            }
        }
        let Some((page, f)) = self.fraction_at(x, y) else {
            self.deselect(ui);
            return 0;
        };
        let Some([_, _, pw, ph]) = self.page_rect(page) else { return 0 };
        let current = self.selected.as_ref().map(|s| (s.page, s.index));
        let hit = self.annots.iter().rev().find(|(p, i, kind, r, _)| {
            *p == page && matches!(*kind, "Drawing" | "Shape" | "Note" | "Image" | "Text") && annot_hit(kind, r, f, [pw, ph], current == Some((*p, *i)))
        });
        match hit {
            Some((p, i, kind, r, text)) => {
                self.selected = Some(Selected { page: *p, index: *i, kind, rect: *r, text: text.clone() });
                self.annot_drag = Some(AnnotDrag { mode: 1, start: [x, y], orig: *r, moved: false });
                self.show_selection(ui);
                1
            }
            None => {
                self.deselect(ui);
                0
            }
        }
    }

    pub(crate) fn annot_drag(&mut self, ui: &AppWindow, x: f32, y: f32) {
        let Some(sel) = self.selected.as_ref() else { return };
        let Some([_, _, pw, ph]) = self.page_rect(sel.page) else { return };
        let Some(d) = self.annot_drag.as_mut() else { return };
        if !d.moved && (x - d.start[0]).abs() < 4.0 && (y - d.start[1]).abs() < 4.0 {
            return;
        }
        d.moved = true;
        let (dx, dy) = ((x - d.start[0]) / pw, (y - d.start[1]) / ph);
        let o = d.orig;
        let min = 0.01;
        let rect = match d.mode {
            1 => {
                let (w, h) = (o[2] - o[0], o[3] - o[1]);
                let l = (o[0] + dx).clamp(0.0, (1.0 - w).max(0.0));
                let t = (o[1] + dy).clamp(0.0, (1.0 - h).max(0.0));
                [l, t, l + w, t + h]
            }
            2 => [(o[0] + dx).min(o[2] - min), (o[1] + dy).min(o[3] - min), o[2], o[3]],
            3 => [o[0], (o[1] + dy).min(o[3] - min), (o[2] + dx).max(o[0] + min), o[3]],
            4 => [(o[0] + dx).min(o[2] - min), o[1], o[2], (o[3] + dy).max(o[1] + min)],
            _ => [o[0], o[1], (o[2] + dx).max(o[0] + min), (o[3] + dy).max(o[1] + min)],
        };
        let rect = rect.map(|v| v.clamp(0.0, 1.0));
        if let Some(sel) = self.selected.as_mut() {
            sel.rect = rect;
        }
        self.show_selection(ui);
    }

    /// The press ended: a drag is saved, and a click on a note opens it.
    pub(crate) fn annot_release(&mut self, ui: &AppWindow) {
        let Some(d) = self.annot_drag.take() else { return };
        let Some(sel) = self.selected.clone() else { return };
        if d.moved {
            if let Some(doc) = self.active_id() {
                self.renderer.edit(EditJob::SetBounds { doc, page: sel.page, index: sel.index, rect: sel.rect });
            }
        } else if sel.kind == "Note" {
            self.open_note_editor(ui, &sel);
        }
    }

    fn open_note_editor(&mut self, ui: &AppWindow, sel: &Selected) {
        let Some([cx, cy, cw, ch]) = self.content_rect(sel.page, sel.rect) else { return };
        let _ = cw;
        self.note_edit = Some((sel.page, sel.index));
        let b = ui.global::<Bridge>();
        b.set_note_edit_text(sel.text.clone().into());
        b.set_note_edit_info(self.annot_info.get(&(sel.page, sel.index)).cloned().unwrap_or_default().into());
        b.set_note_edit_x(cx);
        b.set_note_edit_y(cy + ch + 6.0);
        b.set_note_edit_top(cy);
        b.set_note_edit_open(true);
    }

    /// Auto-scroll runs on vertical scrolling and single pages, so switching it on switches the view too.
    pub(crate) fn toggle_auto_scroll(&mut self, ui: &AppWindow) {
        let b = ui.global::<Bridge>();
        if b.get_auto_scroll() {
            b.set_auto_scroll(false);
            return;
        }
        if self.scroll_mode != 0 || self.spread_mode != 0 {
            self.set_layout_modes(ui, 0, 0);
            self.save_settings(ui);
            self.notify(ui, "Auto-scroll uses vertical scrolling and single pages, so the view was switched.");
        }
        b.set_auto_scroll(true);
    }

    pub(crate) fn note_edit_close(&mut self, ui: &AppWindow) {
        self.note_edit = None;
        ui.global::<Bridge>().set_note_edit_open(false);
    }

    pub(crate) fn note_edit_save(&mut self, ui: &AppWindow, text: String) {
        let (Some((page, index)), Some(doc)) = (self.note_edit, self.active_id()) else { return };
        let text = text.trim().to_string();
        if !text.is_empty() {
            self.renderer.edit(EditJob::SetNote { doc, page, index, text });
        }
        self.note_edit_close(ui);
    }

    pub(crate) fn note_edit_delete(&mut self, ui: &AppWindow) {
        if let (Some((page, index)), Some(doc)) = (self.note_edit, self.active_id()) {
            self.renderer.edit(EditJob::DeleteAnnotation { doc, page, index });
        }
        self.note_edit_close(ui);
    }

    pub(crate) fn undo_redo(&mut self, ui: &AppWindow, undo: bool) {
        let Some(doc) = self.active_id() else { return };
        self.deselect(ui);
        self.note_edit_close(ui);
        self.renderer.edit(if undo { EditJob::Undo { doc } } else { EditJob::Redo { doc } });
    }

    pub(crate) fn show_history(&mut self, ui: &AppWindow, undo: Option<String>, redo: Option<String>) {
        let b = ui.global::<Bridge>();
        b.set_can_undo(undo.is_some());
        b.set_can_redo(redo.is_some());
        b.set_undo_label(undo.unwrap_or_default().into());
        b.set_redo_label(redo.unwrap_or_default().into());
    }

    pub(crate) fn show_fields(&mut self, rows: Vec<crate::render::FieldRow>) {
        self.fields = rows;
    }

    /// A click on a form field: a checkbox or radio button changes, a text field opens for typing.
    /// Returns true when the click was on a field.
    pub(crate) fn click_field(&mut self, ui: &AppWindow, page: usize, f: [f32; 2]) -> bool {
        if self.field_edit.is_some() {
            let text = ui.global::<Bridge>().get_field_edit_text().to_string();
            self.field_commit(ui, text);
        }
        let Some(doc) = self.active_id() else { return false };
        let hit = self.fields.iter().rev().find(|r| r.page == page && f[0] >= r.rect[0] && f[0] <= r.rect[2] && f[1] >= r.rect[1] && f[1] <= r.rect[3]);
        let Some(row) = hit else { return false };
        match row.kind {
            0 => {
                let Some([x, y, w, h]) = self.content_rect(row.page, row.rect) else { return true };
                self.field_edit = Some((row.page, row.index));
                let b = ui.global::<Bridge>();
                b.set_field_edit_text(row.value.clone().into());
                b.set_field_edit_x(x);
                b.set_field_edit_y(y);
                b.set_field_edit_w(w);
                b.set_field_edit_h(h);
                b.set_field_edit_multiline(row.multiline);
                b.set_field_edit_open(true);
            }
            kind => {
                let value = if kind == 1 { FieldValue::Checked(!row.checked) } else { FieldValue::Checked(true) };
                self.renderer.edit(EditJob::SetField { doc, page: row.page, index: row.index, value });
            }
        }
        true
    }

    pub(crate) fn field_commit(&mut self, ui: &AppWindow, text: String) {
        let (Some((page, index)), Some(doc)) = (self.field_edit, self.active_id()) else { return };
        let changed = self.fields.iter().find(|r| r.page == page && r.index == index).map(|r| r.value != text).unwrap_or(true);
        if changed {
            self.renderer.edit(EditJob::SetField { doc, page, index, value: FieldValue::Text(text) });
        }
        self.field_close(ui);
    }

    pub(crate) fn field_close(&mut self, ui: &AppWindow) {
        self.field_edit = None;
        ui.global::<Bridge>().set_field_edit_open(false);
    }

    pub(crate) fn show_annotations(&mut self, ui: &AppWindow, rows: Vec<crate::render::AnnotRow>) {
        self.annots = rows.iter().map(|r| (r.page, r.index, r.kind, r.rect, r.preview.clone())).collect();
        self.annot_info = rows.iter().map(|r| ((r.page, r.index), r.info.clone())).collect();
        if self.annot_drag.is_none() {
            let found = self.selected.as_ref().and_then(|s| self.annots.iter().find(|a| a.0 == s.page && a.1 == s.index && a.2 == s.kind).cloned());
            match (self.selected.as_mut(), found) {
                (Some(s), Some(a)) => {
                    s.rect = a.3;
                    s.text = a.4;
                }
                _ => self.selected = None,
            }
        }
        let rows: Vec<AnnotationRow> = rows
            .into_iter()
            .map(|r| AnnotationRow {
                page: r.page as i32,
                index: r.index as i32,
                kind: r.kind.into(),
                preview: r.preview.into(),
                color: slint::Color::from_rgb_u8(r.color[0], r.color[1], r.color[2]),
                fy: r.fy,
                info: r.info.into(),
            })
            .collect();
        self.annot_model.set_vec(rows);
        self.show_selection(ui);
    }

    pub(crate) fn on_annotated(&mut self, ui: &AppWindow, doc: u64, pages: Vec<usize>, message: Option<String>) {
        if let Some(message) = message {
            store::log_error(&format!("Edit failed: {message}"));
            self.notify(ui, &message);
            return;
        }
        if pages.is_empty() {
            return;
        }
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == doc) {
            tab.dirty = true;
            self.recovery_due.insert(doc);
        }
        if self.doc.as_ref().map(|d| d.id) == Some(doc) {
            for p in &pages {
                self.cache.remove(p);
                self.details.remove(p);
                self.thumbs.remove(p);
            }
            self.refresh(ui);
            self.refresh_thumbs(ui);
            self.request_annotations();
        }
        self.sync_ui(ui);
    }

    // ---- read aloud and translate ----

    pub(crate) fn read_aloud(&mut self, ui: &AppWindow) {
        let b = ui.global::<Bridge>();
        if self.speaking() {
            self.stop_speaking(ui);
            return;
        }
        if let Some(sel) = self.selection.as_ref().filter(|s| !s.text.is_empty()) {
            let text = sel.text.clone();
            self.start_speaking(ui, &text);
            return;
        }
        let (Some(doc), page) = (self.active_id(), b.get_current_page().max(1) as usize - 1) else { return };
        self.renderer.edit(EditJob::PageText { doc, page });
    }

    pub(crate) fn on_page_text(&mut self, ui: &AppWindow, text: String) {
        if text.trim().is_empty() {
            self.notify(ui, "There is no text on this page to read.");
        } else {
            self.start_speaking(ui, &text);
        }
    }

    fn speaking(&self) -> bool {
        self.speaker.as_ref().is_some_and(|s| s.busy())
    }

    fn start_speaking(&mut self, ui: &AppWindow, text: &str) {
        if self.speaker.is_none() {
            self.speaker = platform::Speaker::new();
        }
        let Some(speaker) = &self.speaker else {
            self.notify(ui, "Windows speech is not available.");
            return;
        };
        let text: String = text.chars().take(30_000).collect();
        speaker.speak(&text);
        ui.global::<Bridge>().set_speaking(true);
    }

    pub(crate) fn stop_speaking(&mut self, ui: &AppWindow) {
        if let Some(s) = &self.speaker {
            s.stop();
        }
        ui.global::<Bridge>().set_speaking(false);
    }

    /// Called now and then to notice when the voice has finished.
    pub(crate) fn poll_speech(&mut self, ui: &AppWindow) {
        if ui.global::<Bridge>().get_speaking() && !self.speaking() {
            ui.global::<Bridge>().set_speaking(false);
        }
    }

    /// Opens the selected text in a translation page in the browser.
    pub(crate) fn translate(&mut self, ui: &AppWindow) {
        let Some(text) = self.selection.as_ref().map(|s| s.text.clone()).filter(|t| !t.is_empty()) else {
            self.notify(ui, "Select some text first, then choose Translate.");
            return;
        };
        let text: String = text.chars().take(1_200).collect();
        let language = platform::system_language();
        platform::shell_open(&format!(
            "https://translate.google.com/?sl=auto&tl={language}&op=translate&text={}",
            percent_encode(&text)
        ));
        self.notify(ui, "Opened the translation in your browser.");
    }

    // ---- unsaved changes ----

    pub(crate) fn any_dirty(&self, except: Option<u64>) -> Option<u64> {
        self.tabs.iter().find(|t| t.dirty && Some(t.id) != except).map(|t| t.id)
    }

    /// Closes a tab, asking first when it has changes that are not in any file.
    pub(crate) fn request_close_tab(&mut self, ui: &AppWindow, index: usize) {
        let Some(tab) = self.tabs.get(index) else { return };
        if tab.dirty {
            let id = tab.id;
            self.ask_about_changes(ui, Pending::CloseTab(id));
        } else {
            self.close_tab(ui, index);
        }
    }

    pub(crate) fn request_close_others(&mut self, ui: &AppWindow, keep: usize) {
        let Some(tab) = self.tabs.get(keep) else { return };
        let id = tab.id;
        if self.any_dirty(Some(id)).is_some() {
            self.ask_about_changes(ui, Pending::CloseOthers(id));
        } else {
            self.close_other_tabs(ui, keep);
        }
    }

    /// True when the window may close now.
    pub(crate) fn request_close_window(&mut self, ui: &AppWindow) -> bool {
        if self.any_dirty(None).is_none() {
            return true;
        }
        self.ask_about_changes(ui, Pending::CloseWindow);
        false
    }

    fn pending_tab(&self, pending: Pending) -> Option<u64> {
        match pending {
            Pending::CloseTab(id) => self.tabs.iter().find(|t| t.id == id && t.dirty).map(|t| t.id),
            Pending::CloseOthers(keep) => self.any_dirty(Some(keep)),
            Pending::CloseWindow => self.any_dirty(None),
        }
    }

    fn ask_about_changes(&mut self, ui: &AppWindow, pending: Pending) {
        self.pending = Some(pending);
        self.continue_pending(ui);
    }

    /// Asks about the next tab with changes, or does the closing once none are left.
    fn continue_pending(&mut self, ui: &AppWindow) {
        let Some(pending) = self.pending else { return };
        let b = ui.global::<Bridge>();
        match self.pending_tab(pending) {
            Some(id) => {
                let name = self.tabs.iter().find(|t| t.id == id).map(|t| t.title.clone()).unwrap_or_default();
                self.confirm_tab = Some(id);
                b.set_confirm_text(format!("{name} has changes that are not saved in a file.").into());
                b.set_confirm_open(true);
            }
            None => {
                self.pending = None;
                self.confirm_tab = None;
                b.set_confirm_open(false);
                match pending {
                    Pending::CloseTab(id) => {
                        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                            self.close_tab(ui, i);
                        }
                    }
                    Pending::CloseOthers(keep) => {
                        if let Some(i) = self.tabs.iter().position(|t| t.id == keep) {
                            self.close_other_tabs(ui, i);
                        }
                    }
                    Pending::CloseWindow => {
                        self.save_all(ui);
                        let _ = slint::quit_event_loop();
                    }
                }
            }
        }
    }

    /// 0 saves a copy, 1 discards the changes, 2 cancels.
    pub(crate) fn confirm_choice(&mut self, ui: &AppWindow, choice: i32) {
        let b = ui.global::<Bridge>();
        let Some(id) = self.confirm_tab else {
            b.set_confirm_open(false);
            return;
        };
        match choice {
            0 => {
                let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
                let stem = tab.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
                let source = tab.path.clone();
                let Some(target) = platform::pick_save_path(&format!("{stem} (copy).pdf")) else {
                    // The save dialog was cancelled: leave everything as it was.
                    self.pending = None;
                    self.confirm_tab = None;
                    b.set_confirm_open(false);
                    return;
                };
                if store::same_file(&target, &source) {
                    self.notify(ui, "Choose a different name: a copy cannot replace the file itself.");
                    return;
                }
                self.save_continue = Some(self.next_token());
                let token = self.save_continue.unwrap_or_default();
                self.renderer.edit(EditJob::Save { doc: id, path: target, token });
                b.set_confirm_open(false);
            }
            3 => {
                let Some(path) = self.tabs.iter().find(|t| t.id == id).map(|t| t.path.clone()) else { return };
                let token = self.next_token();
                self.save_continue = Some(token);
                self.in_place.insert(token);
                self.renderer.edit(EditJob::SaveOriginal { doc: id, path, token });
                b.set_confirm_open(false);
            }
            1 => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                    tab.dirty = false;
                    store::clear_recovery(&tab.path);
                    self.recovery_due.remove(&id);
                }
                self.sync_ui(ui);
                self.continue_pending(ui);
            }
            _ => {
                self.pending = None;
                self.confirm_tab = None;
                b.set_confirm_open(false);
            }
        }
    }

    pub(crate) fn next_token(&mut self) -> u64 {
        self.token += 1;
        self.token
    }

    /// Writes the recovery copy of every document that changed since the last one.
    pub(crate) fn write_recoveries(&mut self) {
        let due: Vec<u64> = self.recovery_due.drain().collect();
        for id in due {
            let Some(original) = self.tabs.iter().find(|t| t.id == id && t.dirty).map(|t| t.path.clone()) else { continue };
            let Some(copy) = store::recovery_path(&original) else { continue };
            let token = self.next_token();
            self.recovery_tokens.insert(token);
            self.renderer.edit(EditJob::Save { doc: id, path: copy, token });
        }
    }

    /// At start: offers the changes that were open when the program last stopped without saving them.
    pub(crate) fn offer_recovery(&mut self, ui: &AppWindow) {
        let found = store::pending_recoveries();
        if found.is_empty() {
            return;
        }
        let names: Vec<String> = found.iter().map(|(original, _)| tabs::title_of(original)).collect();
        let text = format!("The program stopped last time before these files were saved:\n\n{}\n\nRecover the changes? They are saved as a new file next to the original; the original is not touched.", names.join("\n"));
        let recover = platform::ask_yes_no("Baca PDF", &text);
        for (original, copy) in found {
            if !recover {
                store::clear_recovery(&original);
                continue;
            }
            let stem = original.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
            let target = original.with_file_name(format!("{stem} (recovered).pdf"));
            match std::fs::copy(&copy, &target) {
                Ok(_) => {
                    store::clear_recovery(&original);
                    self.open_path(ui, target);
                }
                Err(e) => store::log_error(&format!("Recovery could not be written next to {}: {e}", original.display())),
            }
        }
    }

    /// A copy was written by the PDFium thread.
    pub(crate) fn on_saved(&mut self, ui: &AppWindow, doc: u64, path: PathBuf, token: u64, error: Option<String>, backup: Option<PathBuf>) {
        if self.recovery_tokens.remove(&token) {
            if let Some(message) = error {
                store::log_error(&format!("Recovery copy failed: {message}"));
            }
            return;
        }
        if self.flat_tokens.remove(&token) {
            match error {
                Some(message) => {
                    store::log_error(&format!("Flattened copy failed: {message}"));
                    self.notify(ui, &format!("The flattened copy could not be saved: {message}"));
                }
                None => self.notify(ui, &format!("Saved a flattened copy as {}", tabs::title_of(&path))),
            }
            return;
        }
        let in_place = self.in_place.remove(&token);
        let continuing = self.save_continue == Some(token);
        if continuing {
            self.save_continue = None;
        }
        if let Some(message) = error {
            store::log_error(&format!("Save failed: {message}"));
            self.notify(ui, &if in_place { message } else { format!("The copy could not be saved: {message}") });
            if continuing {
                self.pending = None;
                self.confirm_tab = None;
            }
            return;
        }
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == doc && !matches!(t.status, Status::Failed(_))) {
            tab.dirty = false;
            store::clear_recovery(&tab.path);
            self.recovery_due.remove(&doc);
        }
        if in_place {
            let kept = backup.map(|b| format!(" The earlier version is kept as {}.", tabs::title_of(&b))).unwrap_or_default();
            self.notify(ui, &format!("Saved {}.{kept}", tabs::title_of(&path)));
        } else {
            self.notify(ui, &format!("Saved a copy as {}", tabs::title_of(&path)));
        }
        self.sync_ui(ui);
        if continuing {
            self.continue_pending(ui);
        }
    }
}
