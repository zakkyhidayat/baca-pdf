// SPDX-License-Identifier: GPL-3.0-or-later

//! Layout, zoom and the render queue for the document in the active tab.

use slint::{ComponentHandle, Image, Model, ModelRc, VecModel};

use crate::render::{Frac, Request};
use crate::store::{Spot, Zoom};
use crate::*;

/// pdf.js never zooms past 125% on its own.
const MAX_AUTO_SCALE: f32 = 1.25;

pub(crate) const SCROLL_HORIZONTAL: i32 = 1;
pub(crate) const SCROLL_WRAPPED: i32 = 2;
pub(crate) const SCROLL_PAGE: i32 = 3;

impl Viewer {
    pub(crate) fn viewport(&self, ui: &AppWindow) -> (f32, f32) {
        let b = ui.global::<Bridge>();
        (b.get_viewport_width(), b.get_viewport_height())
    }

    pub(crate) fn scroll(&self, ui: &AppWindow) -> (f32, f32) {
        let b = ui.global::<Bridge>();
        (-b.get_view_x(), -b.get_view_y())
    }

    /// Page size in points as displayed, after the user's rotation.
    pub(crate) fn page_size(&self, i: usize) -> (f32, f32) {
        let (w, h) = self.doc.as_ref().and_then(|d| d.sizes.get(i).copied()).unwrap_or((1.0, 1.0));
        if self.turns % 2 == 1 { (h, w) } else { (w, h) }
    }

    fn page_count(&self) -> usize {
        self.doc.as_ref().map(|d| d.sizes.len()).unwrap_or(0)
    }

    /// Pages shown side by side, as index ranges, for the current spread mode.
    pub(crate) fn groups(&self) -> Vec<std::ops::Range<usize>> {
        let n = self.page_count();
        let mut groups = Vec::new();
        let mut i = 0;
        if self.spread_mode == 2 && n > 0 {
            groups.push(0..1);
            i = 1;
        }
        while i < n {
            let len = if self.spread_mode == 0 { 1 } else { 2 };
            groups.push(i..(i + len).min(n));
            i += len;
        }
        groups
    }

    fn group_of(&self, page: usize) -> usize {
        self.groups().iter().position(|g| g.contains(&page)).unwrap_or(0)
    }

    /// Width in points of the widest group, and height of the tallest page.
    fn extents(&self) -> (f32, f32) {
        let mut widest: f32 = 1.0;
        let mut tallest: f32 = 1.0;
        for g in self.groups() {
            let w: f32 = g.clone().map(|i| self.page_size(i).0).sum::<f32>();
            widest = widest.max(w);
            for i in g {
                tallest = tallest.max(self.page_size(i).1);
            }
        }
        (widest, tallest)
    }

    /// The scale a zoom choice means at the current window size.
    pub(crate) fn scale_for(&self, ui: &AppWindow, zoom: Zoom) -> f32 {
        let (vw, vh) = self.viewport(ui);
        let (pw, ph) = self.extents();
        let spread_gap = if self.spread_mode == 0 { 0.0 } else { GAP };
        let width = (vw - 2.0 * MARGIN - spread_gap).max(80.0) / pw;
        let height = (vh - 2.0 * MARGIN).max(80.0) / ph;
        let scale = match zoom {
            Zoom::Actual => BASE_SCALE,
            Zoom::Custom(f) => f * BASE_SCALE,
            Zoom::Width => width,
            Zoom::Fit => width.min(height),
            Zoom::Auto => {
                let portrait = pw <= ph;
                let fitting = if portrait { width } else { width.min(height) };
                fitting.min(MAX_AUTO_SCALE * BASE_SCALE)
            }
        };
        scale.clamp(MIN_ZOOM * BASE_SCALE, MAX_ZOOM * BASE_SCALE)
    }

    /// Page Fit sizes each page (or spread) to the window on its own, so a landscape page and a
    /// portrait page both fill the view. Every other zoom uses one scale for the whole document.
    fn scale_of_group(&self, ui: &AppWindow, g: &std::ops::Range<usize>) -> f32 {
        if !matches!(self.zoom, Zoom::Fit) || self.scroll_mode == SCROLL_WRAPPED {
            return self.scale;
        }
        let (vw, vh) = self.viewport(ui);
        let gap = if g.len() > 1 { GAP * (g.len() as f32 - 1.0) } else { 0.0 };
        let w: f32 = g.clone().map(|i| self.page_size(i).0).sum::<f32>().max(1.0);
        let h: f32 = g.clone().map(|i| self.page_size(i).1).fold(1.0, f32::max);
        let fit = ((vw - 2.0 * MARGIN - gap).max(80.0) / w).min((vh - 2.0 * MARGIN).max(80.0) / h);
        fit.clamp(MIN_ZOOM * BASE_SCALE, MAX_ZOOM * BASE_SCALE)
    }

    fn scale_of_page(&self, ui: &AppWindow, i: usize) -> f32 {
        if !matches!(self.zoom, Zoom::Fit) {
            return self.scale;
        }
        let groups = self.groups();
        match groups.iter().find(|g| g.contains(&i)) {
            Some(g) => self.scale_of_group(ui, g),
            None => self.scale,
        }
    }

    /// Places every page for the current zoom, scroll mode and spread mode.
    pub(crate) fn relayout(&mut self, ui: &AppWindow) {
        let count = self.page_count();
        if count == 0 {
            return;
        }
        let (vw, vh) = self.viewport(ui);
        let groups = self.groups();
        let mut sizes: Vec<(f32, f32)> = vec![(0.0, 0.0); count];
        for g in &groups {
            let scale = self.scale_of_group(ui, g);
            for i in g.clone() {
                let (w, h) = self.page_size(i);
                sizes[i] = (w * scale, h * scale);
            }
        }
        let spans: Vec<(f32, f32)> = groups
            .iter()
            .map(|g| {
                let w: f32 = g.clone().map(|i| sizes[i].0).sum::<f32>() + GAP * (g.len() as f32 - 1.0);
                let h = g.clone().map(|i| sizes[i].1).fold(0.0, f32::max);
                (w, h)
            })
            .collect();

        // Rows of groups: one per row, all in one row, as many as fit the width, or just the current one.
        let mut rows: Vec<Vec<usize>> = Vec::new();
        match self.scroll_mode {
            SCROLL_HORIZONTAL => rows.push((0..groups.len()).collect()),
            SCROLL_WRAPPED => {
                let room = (vw - 2.0 * MARGIN).max(1.0);
                let mut row: Vec<usize> = Vec::new();
                let mut used = 0.0;
                for (gi, &(w, _)) in spans.iter().enumerate() {
                    if !row.is_empty() && used + GAP + w > room {
                        rows.push(std::mem::take(&mut row));
                        used = 0.0;
                    }
                    used += if row.is_empty() { w } else { GAP + w };
                    row.push(gi);
                }
                if !row.is_empty() {
                    rows.push(row);
                }
            }
            SCROLL_PAGE => rows.push(vec![self.page_group.min(groups.len() - 1)]),
            _ => rows.extend((0..groups.len()).map(|gi| vec![gi])),
        }

        let row_size = |row: &Vec<usize>| -> (f32, f32) {
            let w: f32 = row.iter().map(|&gi| spans[gi].0).sum::<f32>() + GAP * (row.len() as f32 - 1.0);
            let h = row.iter().map(|&gi| spans[gi].1).fold(0.0, f32::max);
            (w, h)
        };
        let widest_row = rows.iter().map(|r| row_size(r).0).fold(0.0, f32::max);
        let total_h: f32 = rows.iter().map(|r| row_size(r).1).sum::<f32>() + GAP * (rows.len() as f32 - 1.0);
        self.content_w = widest_row + 2.0 * MARGIN;
        self.content_h = total_h + 2.0 * MARGIN;
        let area_w = self.content_w.max(vw);
        let area_h = self.content_h.max(vh);

        self.rects = vec![None; count];
        // Short content sits in the middle of the window, as pdf.js does in horizontal and page modes.
        let centered = self.scroll_mode == SCROLL_HORIZONTAL || self.scroll_mode == SCROLL_PAGE;
        let mut y = if centered { ((area_h - total_h) / 2.0).max(MARGIN) } else { MARGIN };
        for row in &rows {
            let (rw, rh) = row_size(row);
            let mut x = ((area_w - rw) / 2.0).max(MARGIN);
            for &gi in row {
                for i in groups[gi].clone() {
                    let (w, h) = sizes[i];
                    self.rects[i] = Some([x, y + (rh - h) / 2.0, w, h]);
                    x += w + GAP;
                }
            }
            y += rh + GAP;
        }

        let b = ui.global::<Bridge>();
        b.set_content_width(self.content_w);
        b.set_content_height(self.content_h);
        b.set_zoom_percent((self.scale / BASE_SCALE * 100.0).round() as i32);
        b.set_zoom_mode(self.zoom.index());
        b.set_can_zoom_in(self.scale < MAX_ZOOM * BASE_SCALE - 0.001);
        b.set_can_zoom_out(self.scale > MIN_ZOOM * BASE_SCALE + 0.001);
        b.set_scroll_mode(self.scroll_mode);
        b.set_spread_mode(self.spread_mode);
    }

    pub(crate) fn page_rect(&self, i: usize) -> Option<[f32; 4]> {
        self.rects.get(i).copied().flatten()
    }

    pub(crate) fn set_scroll(&self, ui: &AppWindow, sx: f32, sy: f32) {
        let (vw, vh) = self.viewport(ui);
        let sx = sx.clamp(0.0, (self.content_w - vw).max(0.0));
        let sy = sy.clamp(0.0, (self.content_h - vh).max(0.0));
        let b = ui.global::<Bridge>();
        b.set_view_x(-sx);
        b.set_view_y(-sy);
    }

    /// The page under a content point, or the nearest one.
    pub(crate) fn page_near(&self, x: f32, y: f32) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for (i, r) in self.rects.iter().enumerate() {
            let Some([rx, ry, rw, rh]) = *r else { continue };
            let dx = (rx - x).max(x - (rx + rw)).max(0.0);
            let dy = (ry - y).max(y - (ry + rh)).max(0.0);
            let d = dx * dx + dy * dy;
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, i));
                if d == 0.0 {
                    break;
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// Pages whose rectangle meets the viewport, in page order.
    fn visible_pages(&self, ui: &AppWindow) -> Vec<usize> {
        let (sx, sy) = self.scroll(ui);
        let (vw, vh) = self.viewport(ui);
        self.rects
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let [x, y, w, h] = (*r)?;
                (x < sx + vw && x + w > sx && y < sy + vh && y + h > sy).then_some(i)
            })
            .collect()
    }

    fn current_page(&self, ui: &AppWindow) -> usize {
        let (sx, sy) = self.scroll(ui);
        let (vw, vh) = self.viewport(ui);
        let (px, py) = if self.scroll_mode == SCROLL_HORIZONTAL { (sx + vw / 3.0, sy + vh / 2.0) } else { (sx + vw / 2.0, sy + vh / 3.0) };
        self.page_near(px, py).unwrap_or(0)
    }

    /// The page being read and how far into it the viewport starts.
    pub(crate) fn current_spot(&self, ui: &AppWindow) -> Option<Spot> {
        self.doc.as_ref()?;
        let page = self.current_page(ui);
        let [_, y, _, h] = self.page_rect(page)?;
        let (_, sy) = self.scroll(ui);
        Some(Spot { page, fy: ((sy - y) / h).clamp(0.0, 1.0) })
    }

    /// Scrolls so that page `i` starts at the top, or so that the fraction `fy` of it does.
    pub(crate) fn go_to(&mut self, ui: &AppWindow, i: usize, fy: f32) {
        let count = self.page_count();
        if count == 0 {
            return;
        }
        let i = i.min(count - 1);
        if self.scroll_mode == SCROLL_PAGE && self.group_of(i) != self.page_group {
            self.page_group = self.group_of(i);
            self.relayout(ui);
        }
        let Some([x, y, _, h]) = self.page_rect(i) else { return };
        let (sx, _) = self.scroll(ui);
        let ty = if fy <= 0.0 { y - GAP / 2.0 } else { y + fy * h };
        let tx = if self.scroll_mode == SCROLL_HORIZONTAL { x - MARGIN } else { sx };
        self.set_scroll(ui, tx, ty);
        self.refresh(ui);
    }

    /// Moves to the next or previous page, or pair of pages in a spread.
    pub(crate) fn page_step(&mut self, ui: &AppWindow, step: i32) {
        let groups = self.groups();
        if groups.is_empty() {
            return;
        }
        let at = if self.scroll_mode == SCROLL_PAGE { self.page_group } else { self.group_of(self.current_page(ui)) };
        let next = (at as i32 + step).clamp(0, groups.len() as i32 - 1) as usize;
        self.go_to(ui, groups[next].start, 0.0);
    }

    /// In one-page-at-a-time mode the wheel turns the page once the current one is scrolled through.
    pub(crate) fn scroll_wheel(&mut self, ui: &AppWindow, delta: f32) -> bool {
        if self.scroll_mode != SCROLL_PAGE || self.doc.is_none() {
            return false;
        }
        let (_, sy) = self.scroll(ui);
        let (_, vh) = self.viewport(ui);
        let bottom = (self.content_h - vh).max(0.0);
        if delta < 0.0 && sy >= bottom - 1.0 && self.page_group + 1 < self.groups().len() {
            self.page_step(ui, 1);
            return true;
        }
        if delta > 0.0 && sy <= 1.0 && self.page_group > 0 {
            self.page_step(ui, -1);
            // Going back lands at the bottom of the previous page, where reading left it.
            self.set_scroll(ui, 0.0, f32::MAX);
            self.refresh(ui);
            return true;
        }
        false
    }

    /// Lays out a freshly installed document at the given zoom and reading position.
    pub(crate) fn show_doc(&mut self, ui: &AppWindow, zoom: Zoom, spot: Spot) {
        let Some(doc) = &self.doc else { return };
        let count = doc.sizes.len();
        ui.global::<Bridge>().set_page_count(count as i32);
        let page = spot.page.min(count - 1);
        self.back.clear();
        self.forward.clear();
        self.zoom = zoom;
        self.page_group = self.group_of(page);
        self.scale = self.scale_for(ui, zoom);
        self.last_viewport = self.viewport(ui);
        self.relayout(ui);
        let (vw, _) = self.viewport(ui);
        self.set_scroll(ui, ((self.content_w - vw) / 2.0).max(0.0), 0.0);
        if page > 0 || spot.fy > 0.0 {
            self.go_to(ui, page, spot.fy);
        }
        self.sync_outline();
        self.sync_bookmark_state(ui);
        let b = ui.global::<Bridge>();
        let query = b.get_find_text().to_string();
        if b.get_find_open() && !query.is_empty() {
            self.start_search(ui, query);
        }
        self.refresh(ui);
    }

    /// Drops every bitmap and mark of the document leaving the screen.
    pub(crate) fn clear_view(&mut self) {
        self.cache.clear();
        self.details.clear();
        self.thumbs.clear();
        self.search = None;
        self.selection = None;
        self.rects.clear();
        self.model.set_vec(Vec::new());
        self.thumb_model.set_vec(Vec::new());
        self.mark_model.set_vec(Vec::new());
        self.outline_model.set_vec(Vec::new());
        self.renderer.want(Vec::new());
        self.renderer.want_thumbs(Vec::new());
    }

    /// Bitmaps no longer match after a rotation or a page color change.
    pub(crate) fn drop_bitmaps(&mut self) {
        self.cache.clear();
        self.details.clear();
        self.thumbs.clear();
    }

    /// Changes the scale while keeping the document point under (ax, ay) in place.
    pub(crate) fn zoom_to(&mut self, ui: &AppWindow, scale: f32, ax: f32, ay: f32) {
        if self.doc.is_none() {
            return;
        }
        let scale = scale.clamp(MIN_ZOOM * BASE_SCALE, MAX_ZOOM * BASE_SCALE);
        let (sx, sy) = self.scroll(ui);
        let anchor = self.page_near(sx + ax, sy + ay).and_then(|i| Some((i, self.page_rect(i)?)));
        self.scale = scale;
        self.relayout(ui);
        if let Some((i, [x0, y0, w0, h0])) = anchor {
            let fx = (sx + ax - x0) / w0;
            let fy = (sy + ay - y0) / h0;
            if let Some([x1, y1, w1, h1]) = self.page_rect(i) {
                self.set_scroll(ui, x1 + fx * w1 - ax, y1 + fy * h1 - ay);
            }
        }
        self.refresh(ui);
    }

    /// Applies a zoom choice from the list, keeping the middle of the window in place.
    pub(crate) fn set_zoom(&mut self, ui: &AppWindow, zoom: Zoom) {
        self.zoom = zoom;
        let scale = self.scale_for(ui, zoom);
        let (vw, vh) = self.viewport(ui);
        let anchor_y = if matches!(zoom, Zoom::Custom(_) | Zoom::Actual) { vh / 2.0 } else { 0.0 };
        self.zoom_to(ui, scale, vw / 2.0, anchor_y);
    }

    /// Turns every page a quarter, keeping the current page in view.
    pub(crate) fn rotate(&mut self, ui: &AppWindow, step: i32) {
        if self.doc.is_none() {
            return;
        }
        let page = self.current_spot(ui).map(|s| s.page).unwrap_or(0);
        self.turns = (self.turns as i32 + step).rem_euclid(4) as u8;
        self.drop_bitmaps();
        self.selection = None;
        self.restart_search();
        self.scale = self.scale_for(ui, self.zoom);
        self.relayout(ui);
        self.go_to(ui, page, 0.0);
    }

    pub(crate) fn set_layout_modes(&mut self, ui: &AppWindow, scroll: i32, spread: i32) {
        let page = self.current_spot(ui).map(|s| s.page).unwrap_or(0);
        self.scroll_mode = scroll;
        self.spread_mode = spread;
        if self.doc.is_none() {
            return;
        }
        self.page_group = self.group_of(page);
        self.scale = self.scale_for(ui, self.zoom);
        self.relayout(ui);
        self.go_to(ui, page, 0.0);
    }

    /// Device pixels of the whole page at the current zoom.
    fn full_px(&self, ui: &AppWindow, i: usize) -> (u32, u32) {
        let (w, h) = self.page_size(i);
        let sf = ui.window().scale_factor();
        let scale = self.scale_of_page(ui, i);
        ((w * scale * sf).round().max(1.0) as u32, (h * scale * sf).round().max(1.0) as u32)
    }

    fn base_px(&self, ui: &AppWindow, i: usize) -> (u32, u32) {
        let (fw, fh) = self.full_px(ui, i);
        let k = (BASE_PIXELS / (fw as f32 * fh as f32)).sqrt().min(1.0);
        ((fw as f32 * k).round().max(1.0) as u32, (fh as f32 * k).round().max(1.0) as u32)
    }

    pub(crate) fn request(&self, page: usize, width: u32, height: u32, tile: Option<[u32; 4]>, thumb: bool) -> Request {
        let doc = self.doc.as_ref().map(|d| d.id).unwrap_or(0);
        Request { doc, page, width, height, tile, turns: self.turns, tone: self.tone, thumb }
    }

    /// The pixel rectangle of page `i` inside the viewport, grown by `grow` logical px.
    fn visible_px(&self, ui: &AppWindow, i: usize, grow: f32) -> Option<[u32; 4]> {
        let [px, py, pw, ph] = self.page_rect(i)?;
        let (sx, sy) = self.scroll(ui);
        let (vw, vh) = self.viewport(ui);
        let x0 = (sx - grow - px).max(0.0);
        let y0 = (sy - grow - py).max(0.0);
        let x1 = (sx + vw + grow - px).min(pw);
        let y1 = (sy + vh + grow - py).min(ph);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (fw, fh) = self.full_px(ui, i);
        let k = fw as f32 / pw;
        let l = (x0 * k).floor() as u32;
        let t = (y0 * k).floor() as u32;
        let r = ((x1 * k).ceil() as u32).min(fw);
        let b = ((y1 * k).ceil() as u32).min(fh);
        Some([l, t, r.saturating_sub(l).max(1), b.saturating_sub(t).max(1)])
    }

    fn tile_needed(&self, ui: &AppWindow, i: usize) -> Option<Request> {
        let (fw, fh) = self.full_px(ui, i);
        if self.base_px(ui, i).0 >= fw {
            return None;
        }
        let need = self.visible_px(ui, i, 0.0)?;
        if let Some(d) = self.details.get(&i) {
            let [x, y, w, h] = d.px;
            let covered = d.width == fw
                && x <= need[0]
                && y <= need[1]
                && x + w >= need[0] + need[2]
                && y + h >= need[1] + need[3];
            if covered {
                return None;
            }
        }
        let tile = self.visible_px(ui, i, TILE_MARGIN)?;
        Some(self.request(i, fw, fh, Some(tile), false))
    }

    fn marks_for(&self, page: usize, w: f32, h: f32) -> ModelRc<Mark> {
        let mut marks = Vec::new();
        let mut push = |r: &Frac, kind: i32| {
            marks.push(Mark { x: r[0] * w, y: r[1] * h, width: r[2] * w, height: r[3] * h, kind });
        };
        if let Some(search) = &self.search {
            for (n, hit) in search.hits.iter().enumerate().filter(|(_, hit)| hit.page == page) {
                let current = search.current == Some(n);
                if current || self.highlight_all {
                    hit.rects.iter().for_each(|r| push(r, if current { 1 } else { 0 }));
                }
            }
        }
        if let Some(sel) = &self.selection {
            for (_, rects) in sel.pages.iter().filter(|(p, _)| *p == page) {
                rects.iter().for_each(|r| push(r, 2));
            }
        }
        if marks.is_empty() {
            ModelRc::default()
        } else {
            ModelRc::new(VecModel::from(marks))
        }
    }

    /// Rebuilds the visible page list and the render queue from the scroll position.
    pub(crate) fn refresh(&mut self, ui: &AppWindow) {
        if self.doc.is_none() || self.rects.is_empty() {
            return;
        }
        let count = self.page_count();
        let viewport = self.viewport(ui);
        let resized = (viewport.0 - self.last_viewport.0).abs() > 0.5 || (viewport.1 - self.last_viewport.1).abs() > 0.5;
        self.last_viewport = viewport;
        if resized && !self.pinching {
            let refit = !matches!(self.zoom, Zoom::Custom(_) | Zoom::Actual);
            if refit || self.scroll_mode == SCROLL_WRAPPED {
                let scale = self.scale_for(ui, self.zoom);
                self.zoom_to(ui, scale, 0.0, 0.0);
                return;
            }
        }
        let (_, vh) = viewport;
        let (_, sy) = self.scroll(ui);

        let visible = self.visible_pages(ui);
        let (first, last) = match (visible.first(), visible.last()) {
            (Some(&a), Some(&b)) => (a, b),
            _ => {
                let p = self.current_page(ui);
                (p, p)
            }
        };
        self.visible = (first, last);
        let lo = first.saturating_sub(1);
        let hi = (last + 1).min(count - 1);

        let current = self.current_page(ui) + 1;
        let b = ui.global::<Bridge>();
        if b.get_current_page() != current as i32 {
            b.set_current_page(current as i32);
            self.sync_bookmark_state(ui);
        }

        // Visible pages first, nearest the middle of the viewport.
        let middle = sy + vh / 2.0;
        let mut order = visible.clone();
        order.sort_by(|&a, &b| {
            let da = self.page_rect(a).map(|r| (r[1] + r[3] / 2.0 - middle).abs()).unwrap_or(f32::MAX);
            let db = self.page_rect(b).map(|r| (r[1] + r[3] / 2.0 - middle).abs()).unwrap_or(f32::MAX);
            da.total_cmp(&db)
        });

        let mut wanted = Vec::new();
        for &i in &order {
            if !self.cache.contains_key(&i) {
                let (bw, bh) = self.base_px(ui, i);
                wanted.push(self.request(i, (bw / 4).max(16), (bh / 4).max(16), None, false));
            }
        }
        if !self.pinching {
            for &i in &order {
                if let Some(req) = self.tile_needed(ui, i) {
                    wanted.push(req);
                }
            }
            let prefetch = [lo, hi, (hi + 1).min(count - 1)];
            for i in order.iter().copied().chain(prefetch) {
                if self.page_rect(i).is_none() {
                    continue;
                }
                let (width, height) = self.base_px(ui, i);
                let have = self.cache.get(&i).map(|(w, _)| *w);
                let req = self.request(i, width, height, None, false);
                if have != Some(width) && !wanted.contains(&req) {
                    wanted.push(req);
                }
            }
        }
        self.renderer.want(wanted);

        let keep_lo = lo.saturating_sub(1);
        let keep_hi = hi + 2;
        let keep_pages = CACHE_PAGES.max(visible.len() + 4);
        self.cache.retain(|&i, _| i >= keep_lo && i <= keep_hi);
        while self.cache.len() > keep_pages {
            let far = *self.cache.keys().max_by_key(|&&i| i.abs_diff(current - 1)).unwrap();
            self.cache.remove(&far);
        }
        let stale: Vec<usize> = self
            .details
            .keys()
            .copied()
            .filter(|&i| !visible.contains(&i) || self.base_px(ui, i).0 >= self.full_px(ui, i).0)
            .collect();
        for i in stale {
            self.details.remove(&i);
        }

        let rows: Vec<PageView> = (lo..=hi)
            .filter_map(|i| {
                let [x, y, width, height] = self.page_rect(i)?;
                let image = self.cache.get(&i).map(|(_, img)| img.clone()).unwrap_or_default();
                let marks = self.marks_for(i, width, height);
                let mut row = PageView { page: i as i32, x, y, width, height, image, marks, ..Default::default() };
                if let Some(d) = self.details.get(&i) {
                    row.detail = d.image.clone();
                    row.dx = d.frac[0] * width;
                    row.dy = d.frac[1] * height;
                    row.dw = d.frac[2] * width;
                    row.dh = d.frac[3] * height;
                }
                Some(row)
            })
            .collect();
        let same_pages = self.model.row_count() == rows.len()
            && rows.iter().enumerate().all(|(r, row)| self.model.row_data(r).is_some_and(|old| old.page == row.page));
        if same_pages {
            for (r, row) in rows.into_iter().enumerate() {
                self.model.set_row_data(r, row);
            }
        } else {
            self.model.set_vec(rows);
        }
        let (status, has_hits, not_found) = self.find_status();
        b.set_find_status(status.into());
        b.set_find_has_hits(has_hits);
        b.set_find_not_found(not_found);
        let (current, total, more) = match &self.search {
            Some(s) => (s.current.map(|c| c + 1).unwrap_or(0), s.hits.len(), !s.done),
            None => (0, 0, false),
        };
        b.set_find_current(current as i32);
        b.set_find_total(total as i32);
        b.set_find_more(more);
        b.set_has_selection(self.selection.as_ref().is_some_and(|s| !s.text.is_empty()));
        self.refresh_thumbs(ui);
    }

    pub(crate) fn on_rendered(&mut self, ui: &AppWindow, req: Request, pixels: slint::SharedPixelBuffer<slint::Rgba8Pixel>) {
        let Some(d) = &self.doc else { return };
        if req.doc != d.id || req.turns != self.turns || req.tone != self.tone {
            return;
        }
        if req.thumb {
            self.thumbs.insert(req.page, (req.width, Image::from_rgba8(pixels)));
            self.refresh_thumbs(ui);
            return;
        }
        if let Some(px) = req.tile {
            let (first, last) = self.visible;
            if req.width != self.full_px(ui, req.page).0 || req.page < first || req.page > last {
                return;
            }
            let (fw, fh) = (req.width as f32, req.height as f32);
            let frac = [px[0] as f32 / fw, px[1] as f32 / fh, px[2] as f32 / fw, px[3] as f32 / fh];
            let image = Image::from_rgba8(pixels);
            self.details.insert(req.page, Detail { width: req.width, px, frac, image });
        } else {
            let target = self.base_px(ui, req.page).0;
            let have = self.cache.get(&req.page).map(|(w, _)| *w);
            // Keep the sharper bitmap unless this one matches the current zoom.
            if req.width != target && have.is_some_and(|w| w >= req.width) {
                return;
            }
            self.cache.insert(req.page, (req.width, Image::from_rgba8(pixels)));
        }
        self.refresh(ui);
    }
}
