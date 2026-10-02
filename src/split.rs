// SPDX-License-Identifier: GPL-3.0-or-later

//! Split view: two documents side by side.
//!
//! The viewer keeps one live view, the one of the pane that has the toolbar, the side panel and the
//! keyboard. The other pane's view waits in a `PaneStore`; clicking it swaps the two. Everything the
//! rest of the program does to "the" view keeps working, because it always meets the front pane.

use std::collections::HashMap;
use std::rc::Rc;

use slint::{ComponentHandle, Image, VecModel};

use crate::store::Zoom;
use crate::tabs::Status;
use crate::*;

/// The view state of the pane that is not in front.
pub(crate) struct PaneStore {
    /// The tab whose document this pane shows.
    pub tab: u64,
    doc: Option<Doc>,
    zoom: Zoom,
    scale: f32,
    page_group: usize,
    rects: Vec<Option<[f32; 4]>>,
    content_w: f32,
    content_h: f32,
    last_viewport: (f32, f32),
    visible: (usize, usize),
    cache: HashMap<usize, (u32, Image)>,
    details: HashMap<usize, Detail>,
    model: Rc<VecModel<PageView>>,
    turns: u8,
    search: Option<find::SearchState>,
    selection: Option<find::Selection>,
    back: Vec<usize>,
    forward: Vec<usize>,
}

impl PaneStore {
    fn empty(model: Rc<VecModel<PageView>>, tab: u64) -> Self {
        Self {
            tab,
            doc: None,
            zoom: Zoom::Auto,
            scale: BASE_SCALE,
            page_group: 0,
            rects: Vec::new(),
            content_w: 0.0,
            content_h: 0.0,
            last_viewport: (0.0, 0.0),
            visible: (0, 0),
            cache: HashMap::new(),
            details: HashMap::new(),
            model,
            turns: 0,
            search: None,
            selection: None,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }
}

pub(crate) struct Split {
    pub pane: PaneStore,
}

/// Width taken by the line between the panes.
const DIVIDER: f32 = 7.0;

impl Viewer {
    fn swap_view(&mut self, o: &mut PaneStore) {
        use std::mem::swap;
        swap(&mut self.doc, &mut o.doc);
        swap(&mut self.zoom, &mut o.zoom);
        swap(&mut self.scale, &mut o.scale);
        swap(&mut self.page_group, &mut o.page_group);
        swap(&mut self.rects, &mut o.rects);
        swap(&mut self.content_w, &mut o.content_w);
        swap(&mut self.content_h, &mut o.content_h);
        swap(&mut self.last_viewport, &mut o.last_viewport);
        swap(&mut self.visible, &mut o.visible);
        swap(&mut self.cache, &mut o.cache);
        swap(&mut self.details, &mut o.details);
        swap(&mut self.model, &mut o.model);
        swap(&mut self.turns, &mut o.turns);
        swap(&mut self.search, &mut o.search);
        swap(&mut self.selection, &mut o.selection);
        swap(&mut self.back, &mut o.back);
        swap(&mut self.forward, &mut o.forward);
    }

    /// Whether a tab is one of the two shown in split view.
    pub(crate) fn in_split(&self, id: u64) -> bool {
        match &self.split {
            Some(s) => s.pane.tab == id || self.active.is_some_and(|a| self.tabs[a].id == id),
            None => false,
        }
    }

    /// Runs something against the pane that is not in front, as if it were the live view.
    pub(crate) fn in_other_pane(&mut self, ui: &AppWindow, f: impl FnOnce(&mut Self, &AppWindow)) {
        let Some(mut split) = self.split.take() else { return };
        self.swap_view(&mut split.pane);
        self.focus_side ^= 1;
        f(self, ui);
        self.swap_view(&mut split.pane);
        self.focus_side ^= 1;
        self.split = Some(split);
        self.publish_pane(ui);
    }

    /// Puts what the toolbar shows back in line with the front pane.
    pub(crate) fn publish_pane(&mut self, ui: &AppWindow) {
        let b = ui.global::<Bridge>();
        b.set_page_count(self.page_count() as i32);
        if self.doc.is_some() && !self.rects.is_empty() {
            b.set_current_page(self.current_page(ui) as i32 + 1);
        }
        b.set_zoom_percent((self.scale / BASE_SCALE * 100.0).round() as i32);
        b.set_zoom_mode(self.zoom.index());
        b.set_can_zoom_in(self.scale < MAX_ZOOM * BASE_SCALE - 0.001);
        b.set_can_zoom_out(self.scale > MIN_ZOOM * BASE_SCALE + 0.001);
        b.set_has_selection(self.selection.as_ref().is_some_and(|s| !s.text.is_empty()));
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
        self.sync_bookmark_state(ui);
    }

    /// Lays out the pages of both panes; each asks for the bitmaps it needs.
    pub(crate) fn refresh_all(&mut self, ui: &AppWindow) {
        self.refresh(ui);
        if self.split.is_some() {
            self.in_other_pane(ui, |s, ui| s.refresh(ui));
        }
    }

    fn load_view(&mut self, ui: &AppWindow, index: usize) {
        let tab = &mut self.tabs[index];
        self.doc = tab.doc.take();
        self.turns = tab.turns;
        let (zoom, spot) = (tab.zoom, tab.spot);
        self.show_doc(ui, zoom, spot);
    }

    /// Shows `left` and `right` side by side. `focus_right` says which pane starts in front.
    pub(crate) fn make_split(&mut self, ui: &AppWindow, left: usize, right: usize, focus_right: bool) {
        if left == right || left >= self.tabs.len() || right >= self.tabs.len() {
            return;
        }
        if self.split.is_some() {
            self.exit_split(ui);
        }
        if self.presenting.is_some() {
            self.toggle_presentation(ui);
        }
        if !matches!(self.tabs[left].status, Status::Ready) || !matches!(self.tabs[right].status, Status::Ready) {
            self.notify(ui, "Wait until both documents have opened, then try again.");
            return;
        }
        // Both documents wait in their tabs, then each is loaded into a pane.
        self.park_active(ui);
        let (left_id, right_id) = (self.tabs[left].id, self.tabs[right].id);
        let b = ui.global::<Bridge>();
        // The panes are not laid out yet, so start from half of the width there is.
        let window_w = ui.window().size().width as f32 / ui.window().scale_factor();
        let full = if b.get_viewport_width() > 100.0 { b.get_viewport_width() } else { window_w - 220.0 };
        let half = ((full - DIVIDER) / 2.0).max(200.0);
        let height = b.get_viewport_height().max(300.0);
        b.set_viewport_width(half);
        b.set_viewport_height(height);
        b.set_viewport_width_b(half);
        b.set_viewport_height_b(height);
        b.set_split_ratio(0.5);

        self.focus_side = 0;
        self.active = Some(left);
        self.load_view(ui, left);
        let mut store = PaneStore::empty(self.pane_models[1].clone(), left_id);
        self.swap_view(&mut store);
        self.focus_side = 1;
        self.active = self.tabs.iter().position(|t| t.id == right_id);
        if let Some(r) = self.active {
            self.load_view(ui, r);
        }
        self.split = Some(Split { pane: store });
        b.set_split_active(true);
        b.set_focus_side(1);
        if !focus_right {
            self.swap_focus(ui);
        } else {
            self.after_focus_change(ui);
        }
    }

    /// Brings the other pane to the front.
    pub(crate) fn swap_focus(&mut self, ui: &AppWindow) {
        let Some(mut split) = self.split.take() else { return };
        let Some(a) = self.active else {
            self.split = Some(split);
            return;
        };
        self.swap_view(&mut split.pane);
        self.focus_side ^= 1;
        let front_id = self.tabs[a].id;
        let new_front = split.pane.tab;
        split.pane.tab = front_id;
        self.active = self.tabs.iter().position(|t| t.id == new_front);
        self.split = Some(split);
        self.after_focus_change(ui);
    }

    pub(crate) fn focus_pane(&mut self, ui: &AppWindow, side: i32) {
        if self.split.is_some() && side != self.focus_side {
            self.swap_focus(ui);
        }
    }

    fn after_focus_change(&mut self, ui: &AppWindow) {
        ui.global::<Bridge>().set_focus_side(self.focus_side);
        self.tool_changed(ui);
        self.sync_ui(ui);
        self.publish_pane(ui);
        self.sync_outline();
        self.thumbs.clear();
        self.refresh_thumbs(ui);
        self.save_session(ui);
    }

    /// Back to one pane, showing the one that was in front.
    pub(crate) fn exit_split(&mut self, ui: &AppWindow) {
        let Some(mut split) = self.split.take() else { return };
        let Some(front) = self.active else { return };
        let keep_id = self.tabs[front].id;
        // Park the front pane's document in its tab, then the other one's.
        self.park_active(ui);
        self.swap_view(&mut split.pane);
        self.focus_side ^= 1;
        self.active = self.tabs.iter().position(|t| t.id == split.pane.tab);
        self.park_active(ui);
        self.focus_side = 0;
        self.model = self.pane_models[0].clone();
        self.pane_models[0].set_vec(Vec::new());
        self.pane_models[1].set_vec(Vec::new());
        let b = ui.global::<Bridge>();
        b.set_split_active(false);
        b.set_focus_side(0);
        self.active = None;
        let again = self.tabs.iter().position(|t| t.id == keep_id);
        self.activate(ui, again);
    }

    /// Ctrl+backslash: split the front tab with the next one, or leave split view.
    pub(crate) fn toggle_split(&mut self, ui: &AppWindow) {
        if self.split.is_some() {
            self.exit_split(ui);
            return;
        }
        let Some(a) = self.active else { return };
        if self.tabs.len() < 2 {
            self.notify(ui, "Open a second document to use split view.");
            return;
        }
        let next = (a + 1) % self.tabs.len();
        self.make_split(ui, a, next, true);
    }

    /// Ctrl+click on a tab: that tab opens beside the front one.
    pub(crate) fn split_with(&mut self, ui: &AppWindow, index: usize) {
        let Some(a) = self.active else {
            self.notify(ui, "Open a document first, then Ctrl+click another tab.");
            return;
        };
        if let Some(split) = &self.split {
            if self.tabs.get(index).is_some_and(|t| t.id == split.pane.tab) {
                self.swap_focus(ui);
                return;
            }
        }
        if index == a {
            return;
        }
        self.make_split(ui, a, index, true);
    }

    /// A tab dropped on another: they open side by side, the dropped one in front on the right.
    pub(crate) fn split_tabs(&mut self, ui: &AppWindow, dragged: usize, target: usize) {
        self.make_split(ui, target, dragged, true);
    }

    /// Rendered bitmaps for the pane that is not in front.
    pub(crate) fn route_rendered(&mut self, ui: &AppWindow, req: render::Request, pixels: slint::SharedPixelBuffer<slint::Rgba8Pixel>) {
        if self.doc.as_ref().is_some_and(|d| d.id == req.doc) {
            return self.on_rendered(ui, req, pixels);
        }
        if req.thumb {
            return;
        }
        let other = self.split.as_ref().and_then(|s| s.pane.doc.as_ref()).map(|d| d.id);
        if other == Some(req.doc) {
            self.in_other_pane(ui, |s, ui| s.on_rendered(ui, req, pixels));
        }
    }

    /// An edit finished in the document of either pane.
    pub(crate) fn route_annotated(&mut self, ui: &AppWindow, doc: u64, pages: Vec<usize>, message: Option<String>) {
        let other = self.split.as_ref().and_then(|s| s.pane.doc.as_ref()).map(|d| d.id);
        if other == Some(doc) {
            self.in_other_pane(ui, |s, ui| s.on_annotated(ui, doc, pages, message));
        } else {
            self.on_annotated(ui, doc, pages, message);
        }
    }
}
