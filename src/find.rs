// SPDX-License-Identifier: GPL-3.0-or-later

//! Find in the document, and selecting text on a page to copy it.

use crate::render::{Frac, SelectJob, SelectPiece};
use crate::*;

pub(crate) struct Hit {
    pub page: usize,
    pub rects: Vec<Frac>,
}

pub(crate) struct SearchState {
    pub generation: u64,
    pub query: String,
    pub hits: Vec<Hit>,
    pub current: Option<usize>,
    pub done: bool,
    /// Results before this page are not jumped to first; finding starts where you are reading.
    pub from_page: usize,
}

pub(crate) struct Selection {
    pub generation: u64,
    pub anchor_page: usize,
    pub anchor: [f32; 2],
    /// The highlighted rectangles of each page the selection touches.
    pub pages: Vec<(usize, Vec<Frac>)>,
    /// The stretches of pages it covers, for turning it into highlights.
    pub pieces: Vec<SelectPiece>,
    pub text: String,
}

impl Viewer {
    pub(crate) fn start_search(&mut self, ui: &AppWindow, query: String) {
        let from_page = self.current_spot(ui).map(|s| s.page).unwrap_or(0);
        self.begin_search(query, from_page);
        self.refresh(ui);
    }

    fn begin_search(&mut self, query: String, from_page: usize) {
        self.search_gen += 1;
        let Some(doc) = self.doc.as_ref().map(|d| d.id) else { return };
        self.renderer.search(doc, self.search_gen, query.clone(), self.find_opts, self.turns);
        self.search = (!query.is_empty()).then(|| SearchState {
            generation: self.search_gen,
            query,
            hits: Vec::new(),
            current: None,
            done: false,
            from_page,
        });
    }

    /// Hit rectangles depend on rotation, so a rotation reruns the search from the same place.
    pub(crate) fn restart_search(&mut self) {
        if let Some(s) = self.search.take() {
            self.begin_search(s.query, s.from_page);
        }
    }

    pub(crate) fn on_search_hits(&mut self, ui: &AppWindow, generation: u64, page: usize, hits: Vec<Vec<Frac>>) {
        let Some(search) = self.search.as_mut().filter(|s| s.generation == generation) else { return };
        let first_new = search.hits.len();
        search.hits.extend(hits.into_iter().map(|rects| Hit { page, rects }));
        if search.current.is_none() && page >= search.from_page {
            search.current = Some(first_new);
            self.reveal_hit(ui);
        }
        self.refresh(ui);
    }

    pub(crate) fn on_search_done(&mut self, ui: &AppWindow, generation: u64) {
        let Some(search) = self.search.as_mut().filter(|s| s.generation == generation) else { return };
        search.done = true;
        if search.current.is_none() && !search.hits.is_empty() {
            search.current = Some(0);
            self.reveal_hit(ui);
        }
        self.refresh(ui);
    }

    pub(crate) fn find_step(&mut self, ui: &AppWindow, step: i32) {
        let Some(search) = self.search.as_mut() else { return };
        let n = search.hits.len() as i32;
        if n == 0 {
            return;
        }
        let next = match search.current {
            Some(c) => (c as i32 + step).rem_euclid(n),
            None if step > 0 => 0,
            None => n - 1,
        };
        search.current = Some(next as usize);
        self.reveal_hit(ui);
        self.refresh(ui);
    }

    /// Scrolls the current hit into view, a third of the way down so the lines before it show too.
    fn reveal_hit(&mut self, ui: &AppWindow) {
        let Some(search) = &self.search else { return };
        let Some(hit) = search.current.and_then(|c| search.hits.get(c)) else { return };
        let Some(r) = hit.rects.first().copied() else { return };
        let Some([px, py, w, h]) = self.page_rect(hit.page) else { return };
        let (sx, sy) = self.scroll(ui);
        let (vw, vh) = self.viewport(ui);
        let (hx, hy) = (px + r[0] * w, py + r[1] * h);
        let ny = if hy < sy + 24.0 || hy + r[3] * h > sy + vh - 24.0 { hy - vh / 3.0 } else { sy };
        let nx = if hx < sx || hx + r[2] * w > sx + vw { hx - vw / 3.0 } else { sx };
        self.set_scroll(ui, nx, ny);
    }

    /// The count line under the find field, whether there are hits, and whether nothing was found.
    pub(crate) fn find_status(&self) -> (String, bool, bool) {
        match &self.search {
            None => (String::new(), false, false),
            Some(s) if s.hits.is_empty() && s.done => ("Phrase not found".into(), false, true),
            Some(s) if s.hits.is_empty() => ("Searching".into(), false, false),
            Some(s) => {
                let at = s.current.map(|c| c + 1).unwrap_or(0);
                let n = s.hits.len();
                let word = if n == 1 { "match" } else { "matches" };
                let more = if s.done { "" } else { "+" };
                (format!("{at} of {n}{more} {word}"), true, false)
            }
        }
    }

    /// `phase` 0 starts a selection at the pointer, 1 extends it; points are in content coordinates.
    pub(crate) fn select_at(&mut self, ui: &AppWindow, phase: i32, x: f32, y: f32) {
        let Some(doc) = self.doc.as_ref().map(|d| d.id) else { return };
        let Some(page) = self.page_near(x, y) else { return };
        let Some([px, py, w, h]) = self.page_rect(page) else { return };
        let point = [((x - px) / w).clamp(0.0, 1.0), ((y - py) / h).clamp(0.0, 1.0)];
        match phase {
            0 => {
                self.select_gen += 1;
                self.selection = Some(Selection {
                    generation: self.select_gen,
                    anchor_page: page,
                    anchor: point,
                    pages: Vec::new(),
                    pieces: Vec::new(),
                    text: String::new(),
                });
                self.refresh(ui);
            }
            1 => {
                let Some(sel) = &self.selection else { return };
                let (first, last) = if page >= sel.anchor_page { (sel.anchor_page, page) } else { (page, sel.anchor_page) };
                let (start, end) = if page >= sel.anchor_page { (sel.anchor, point) } else { (point, sel.anchor) };
                let pieces = if first == last {
                    vec![SelectPiece { page, from: Some(sel.anchor), to: Some(point), expand: 0 }]
                } else {
                    // The pages in between are taken whole; very long drags stop at a limit.
                    let last = last.min(first + 300);
                    let mut pieces = vec![SelectPiece { page: first, from: Some(start), to: None, expand: 0 }];
                    pieces.extend((first + 1..last).map(|p| SelectPiece { page: p, from: None, to: None, expand: 0 }));
                    pieces.push(SelectPiece { page: last, from: None, to: Some(end), expand: 0 });
                    pieces
                };
                let generation = sel.generation;
                if let Some(sel) = self.selection.as_mut() {
                    sel.pieces = pieces.clone();
                }
                self.renderer.select(SelectJob { doc, generation, turns: self.turns, pieces });
            }
            // A double click selects the word under the pointer, a triple click the line.
            2 | 3 => {
                self.select_gen += 1;
                let pieces = vec![SelectPiece { page, from: Some(point), to: Some(point), expand: phase as u8 - 1 }];
                self.selection = Some(Selection {
                    generation: self.select_gen,
                    anchor_page: page,
                    anchor: point,
                    pages: Vec::new(),
                    pieces: pieces.clone(),
                    text: String::new(),
                });
                self.renderer.select(SelectJob { doc, generation: self.select_gen, turns: self.turns, pieces });
                self.refresh(ui);
                if ui.global::<Bridge>().get_tool() == 1 {
                    self.annotate_selection(ui);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn select_page(&mut self, ui: &AppWindow) {
        let Some(doc) = self.doc.as_ref().map(|d| d.id) else { return };
        let Some(page) = self.current_spot(ui).map(|s| s.page) else { return };
        self.select_gen += 1;
        self.selection = Some(Selection {
            generation: self.select_gen,
            anchor_page: page,
            anchor: [0.0, 0.0],
            pages: Vec::new(),
            pieces: vec![SelectPiece { page, from: None, to: None, expand: 0 }],
            text: String::new(),
        });
        let pieces = vec![SelectPiece { page, from: None, to: None, expand: 0 }];
        self.renderer.select(SelectJob { doc, generation: self.select_gen, turns: self.turns, pieces });
    }

    pub(crate) fn on_selected(&mut self, ui: &AppWindow, generation: u64, pages: Vec<(usize, Vec<Frac>)>, text: String) {
        let Some(sel) = self.selection.as_mut().filter(|s| s.generation == generation) else { return };
        sel.pages = pages;
        sel.text = text;
        self.refresh(ui);
    }

    pub(crate) fn clear_selection(&mut self, ui: &AppWindow) {
        if self.selection.take().is_some() {
            self.refresh(ui);
        }
    }

    /// Returns false when nothing is selected.
    pub(crate) fn copy_selection(&self) -> bool {
        let Some(sel) = self.selection.as_ref().filter(|s| !s.text.is_empty()) else { return false };
        // Lets automated tests check the copied text without touching the user's clipboard.
        if let Some(path) = std::env::var_os("BACA_TEST_COPY_TO") {
            return std::fs::write(path, &sel.text).is_ok();
        }
        platform::copy_text(&sel.text)
    }
}
