// SPDX-License-Identifier: GPL-3.0-or-later

//! The drawing tools (highlight, draw, erase, text), read aloud and translate, and the question
//! asked before changes are thrown away.

use std::path::PathBuf;

use slint::ComponentHandle;

use crate::render::EditJob;
use crate::tabs::Status;
use crate::*;

pub(crate) const HIGHLIGHT_COLORS: [[u8; 3]; 5] =
    [[255, 233, 77], [126, 231, 135], [102, 217, 239], [255, 143, 208], [255, 180, 84]];
pub(crate) const DRAW_COLORS: [[u8; 3]; 5] = [[232, 17, 35], [27, 27, 27], [0, 99, 177], [16, 137, 62], [255, 140, 0]];
/// Line widths in PDF points: thin, medium, thick.
pub(crate) const DRAW_WIDTHS: [f32; 3] = [1.5, 3.0, 6.0];
const TEXT_SIZE: f32 = 12.0;

/// A line being drawn, in content coordinates.
pub(crate) struct Stroke {
    page: usize,
    points: Vec<[f32; 2]>,
    width: f32,
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
    fn active_id(&self) -> Option<u64> {
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
    }

    /// Turns the text selected while the highlighter was on into highlights.
    pub(crate) fn annotate_selection(&mut self, ui: &AppWindow) {
        let Some(doc) = self.active_id() else { return };
        let Some(sel) = self.selection.as_ref().filter(|s| !s.pieces.is_empty()) else { return };
        let color = HIGHLIGHT_COLORS[(ui.global::<Bridge>().get_highlight_color().max(0) as usize).min(4)];
        self.renderer.edit(EditJob::Highlight { doc, pieces: sel.pieces.clone(), turns: self.turns, color, style: ui.global::<Bridge>().get_mark_style().clamp(0, 2) as u8 });
        self.clear_selection(ui);
    }

    pub(crate) fn stroke_begin(&mut self, ui: &AppWindow, x: f32, y: f32) {
        let Some(page) = self.page_near(x, y) else { return };
        let width_px = DRAW_WIDTHS[(ui.global::<Bridge>().get_draw_width().max(0) as usize).min(2)] * self.scale_of_page(ui, page);
        ui.global::<Bridge>().set_stroke_width(width_px.max(1.0));
        let stroke = Stroke { page, points: vec![[x, y]], width: width_px.max(1.0) };
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
        stroke.points.push([x, y]);
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

    pub(crate) fn text_at(&mut self, ui: &AppWindow, x: f32, y: f32) {
        let Some((page, point)) = self.fraction_at(x, y) else { return };
        self.text_target = Some(TextTarget { page, point });
        let b = ui.global::<Bridge>();
        b.set_text_x(x);
        b.set_text_y(y);
        b.set_text_open(true);
    }

    pub(crate) fn text_commit(&mut self, ui: &AppWindow, text: String) {
        ui.global::<Bridge>().set_text_open(false);
        let (Some(target), Some(doc)) = (self.text_target.take(), self.active_id()) else { return };
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let color = DRAW_COLORS[(ui.global::<Bridge>().get_draw_color().max(0) as usize).min(4)];
        self.renderer.edit(EditJob::Text { doc, page: target.page, turns: self.turns, point: target.point, text, size: TEXT_SIZE, color });
    }

    pub(crate) fn text_cancel(&mut self, ui: &AppWindow) {
        self.text_target = None;
        ui.global::<Bridge>().set_text_open(false);
    }

    /// A drawing, highlight, erase or text edit finished (or failed) on the PDFium thread.
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
        }
        if self.doc.as_ref().map(|d| d.id) == Some(doc) {
            for p in &pages {
                self.cache.remove(p);
                self.details.remove(p);
                self.thumbs.remove(p);
            }
            self.refresh(ui);
            self.refresh_thumbs(ui);
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

    /// A copy was written by the PDFium thread.
    pub(crate) fn on_saved(&mut self, ui: &AppWindow, doc: u64, path: PathBuf, token: u64, error: Option<String>, backup: Option<PathBuf>) {
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
