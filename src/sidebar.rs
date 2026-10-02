// SPDX-License-Identifier: GPL-3.0-or-later

//! The side panel: page thumbnails and the document's own table of contents.

use slint::{ComponentHandle, Model};

use crate::*;

/// Logical size of the box a thumbnail is fitted into, and the height of one row.
const THUMB_W: f32 = 132.0;
const THUMB_H: f32 = 168.0;
const THUMB_ROW: f32 = THUMB_H + 36.0;
const THUMB_CACHE: usize = 80;

impl Viewer {
    pub(crate) fn sync_outline(&mut self) {
        let rows: Vec<OutlineRow> = self
            .doc
            .as_ref()
            .map(|d| {
                d.outline
                    .iter()
                    .map(|o| OutlineRow {
                        title: if o.title.is_empty() { "Untitled".into() } else { o.title.as_str().into() },
                        depth: o.depth.min(6) as i32,
                        page: o.page.map(|p| p as i32 + 1).unwrap_or(0),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.outline_model.set_vec(rows);
    }

    /// Shows the thumbnails in the scrolled part of the panel and asks for the missing ones.
    pub(crate) fn refresh_thumbs(&mut self, ui: &AppWindow) {
        let Some(doc) = &self.doc else { return };
        let count = doc.sizes.len();
        ui.global::<Bridge>().set_thumb_content_height(count as f32 * THUMB_ROW + 8.0);
        let tab = ui.global::<Bridge>().get_sidebar_tab();
        if !ui.global::<Bridge>().get_sidebar_open() || tab == 1 {
            self.renderer.want_thumbs(Vec::new());
            return;
        }
        if tab == 2 {
            self.refresh_marked_thumbs(ui);
            return;
        }
        let sy = -ui.global::<Bridge>().get_thumb_view_y();
        let vh = ui.global::<Bridge>().get_thumb_viewport_height().max(THUMB_ROW);
        let first = (sy / THUMB_ROW).floor().max(0.0) as usize;
        let last = (((sy + vh) / THUMB_ROW).ceil() as usize).min(count.saturating_sub(1));
        let lo = first.saturating_sub(2);
        let hi = (last + 2).min(count - 1);
        let current = ui.global::<Bridge>().get_current_page() as usize;
        let sf = ui.window().scale_factor();

        let mut wanted = Vec::new();
        let rows: Vec<ThumbView> = (lo..=hi)
            .map(|i| {
                let (pw, ph) = self.page_size(i);
                let k = (THUMB_W / pw).min(THUMB_H / ph);
                let (w, h) = (pw * k, ph * k);
                let px_w = (w * sf).round().max(1.0) as u32;
                if self.thumbs.get(&i).map(|(pw, _)| *pw) != Some(px_w) && (first..=last).contains(&i) {
                    wanted.push(self.request(i, px_w, (h * sf).round().max(1.0) as u32, None, true));
                }
                ThumbView {
                    page: i as i32,
                    y: i as f32 * THUMB_ROW + 8.0 + (THUMB_H - h) / 2.0,
                    width: w,
                    height: h,
                    image: self.thumbs.get(&i).map(|(_, img)| img.clone()).unwrap_or_default(),
                    current: i + 1 == current,
                }
            })
            .collect();
        self.renderer.want_thumbs(wanted);

        if self.thumbs.len() > THUMB_CACHE {
            let mid = (first + last) / 2;
            let mut keys: Vec<usize> = self.thumbs.keys().copied().collect();
            keys.sort_by_key(|&k| std::cmp::Reverse(k.abs_diff(mid)));
            for k in keys.into_iter().take(self.thumbs.len() - THUMB_CACHE) {
                self.thumbs.remove(&k);
            }
        }

        let same = self.thumb_model.row_count() == rows.len()
            && rows.iter().enumerate().all(|(r, row)| self.thumb_model.row_data(r).is_some_and(|old| old.page == row.page));
        if same {
            for (r, row) in rows.into_iter().enumerate() {
                if self.thumb_model.row_data(r).as_ref() != Some(&row) {
                    self.thumb_model.set_row_data(r, row);
                }
            }
        } else {
            self.thumb_model.set_vec(rows);
        }
    }

    /// Thumbnails of the bookmarked pages only, one under the other.
    fn refresh_marked_thumbs(&mut self, ui: &AppWindow) {
        let Some(doc) = &self.doc else { return };
        let count = doc.sizes.len();
        let marked: Vec<usize> = self
            .active_path()
            .map(|p| self.bookmarks_for(&p))
            .unwrap_or_default()
            .into_iter()
            .filter(|&p| p >= 1 && p <= count)
            .map(|p| p - 1)
            .take(60)
            .collect();
        let current = ui.global::<Bridge>().get_current_page() as usize;
        let sf = ui.window().scale_factor();
        let mut wanted = Vec::new();
        let rows: Vec<ThumbView> = marked
            .iter()
            .enumerate()
            .map(|(n, &i)| {
                let (pw, ph) = self.page_size(i);
                let k = (THUMB_W / pw).min(THUMB_H / ph);
                let (w, h) = (pw * k, ph * k);
                let px_w = (w * sf).round().max(1.0) as u32;
                if self.thumbs.get(&i).map(|(pw, _)| *pw) != Some(px_w) {
                    wanted.push(self.request(i, px_w, (h * sf).round().max(1.0) as u32, None, true));
                }
                ThumbView {
                    page: i as i32,
                    y: n as f32 * THUMB_ROW + 8.0 + (THUMB_H - h) / 2.0,
                    width: w,
                    height: h,
                    image: self.thumbs.get(&i).map(|(_, img)| img.clone()).unwrap_or_default(),
                    current: i + 1 == current,
                }
            })
            .collect();
        self.renderer.want_thumbs(wanted);
        ui.global::<Bridge>().set_mark_content_height(marked.len() as f32 * THUMB_ROW + 8.0);
        self.mark_model.set_vec(rows);
    }

    /// Scrolls the panel so the thumbnail of `page` is visible.
    pub(crate) fn reveal_thumb(&self, ui: &AppWindow, page: usize) {
        let sy = -ui.global::<Bridge>().get_thumb_view_y();
        let vh = ui.global::<Bridge>().get_thumb_viewport_height();
        let top = page as f32 * THUMB_ROW;
        if top < sy || top + THUMB_ROW > sy + vh {
            let max = (ui.global::<Bridge>().get_thumb_content_height() - vh).max(0.0);
            ui.global::<Bridge>().set_thumb_view_y(-(top - vh / 3.0).clamp(0.0, max));
        }
    }
}
