// SPDX-License-Identifier: GPL-3.0-or-later

//! Open tabs, the Home lists (recent files, favorites, bookmarks), and saving them between runs.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::render::Event;
use crate::store::{self, Placement, Session, SessionTab, Spot, Zoom};
use crate::*;

pub(crate) enum Status {
    /// Restored from the last session and not read yet; loading waits until the tab is shown.
    Unloaded,
    Loading,
    Ready,
    Failed(String),
}

pub(crate) struct Tab {
    pub id: u64,
    pub path: PathBuf,
    pub title: String,
    pub status: Status,
    /// Parked here while the tab is in the background; the active tab's document lives in `Viewer::doc`.
    pub doc: Option<Doc>,
    pub zoom: Zoom,
    pub spot: Spot,
    pub turns: u8,
    /// When the file was last read, to notice it changing on disk.
    pub stamp: Option<SystemTime>,
    /// The file asked for a password and has not been opened yet.
    pub locked: bool,
    pub password_wrong: bool,
    /// Drawn on or edited since it was opened, and not yet saved as a copy.
    pub dirty: bool,
    /// Pinned tabs sit first and show only their icon.
    pub pinned: bool,
}

pub(crate) fn title_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

fn folder_of(path: &Path) -> String {
    path.parent().map(|p| p.display().to_string()).unwrap_or_default()
}

impl Viewer {
    fn new_tab(&mut self, path: PathBuf, zoom: Zoom, spot: Spot, turns: u8) -> usize {
        self.next_id += 1;
        let title = title_of(&path);
        self.tabs.push(Tab { id: self.next_id, path, title, status: Status::Unloaded, doc: None, zoom, spot, turns, stamp: None, locked: false, password_wrong: false, dirty: false, pinned: false });
        self.tabs.len() - 1
    }

    /// Saves the reading position of the active tab and releases its bitmaps.
    fn park_active(&mut self, ui: &AppWindow) {
        let Some(a) = self.active else { return };
        let spot = self.current_spot(ui);
        let doc = self.doc.take();
        let tab = &mut self.tabs[a];
        if doc.is_some() {
            tab.zoom = self.zoom;
            tab.spot = spot.unwrap_or(tab.spot);
            tab.turns = self.turns;
            tab.doc = doc;
        }
        if let Some(r) = self.recents.iter_mut().find(|r| store::same_file(&r.path, &tab.path)) {
            r.spot = tab.spot;
        }
        self.clear_view();
    }

    pub(crate) fn activate(&mut self, ui: &AppWindow, index: Option<usize>) {
        if self.active.is_some() && self.active == index {
            return;
        }
        if self.presenting.is_some() {
            self.toggle_presentation(ui);
        }
        self.park_active(ui);
        self.active = index.filter(|&i| i < self.tabs.len());
        if let Some(i) = self.active {
            let tab = &mut self.tabs[i];
            match tab.status {
                Status::Unloaded => {
                    tab.status = Status::Loading;
                    tab.stamp = store::modified(&tab.path);
                    self.renderer.open(tab.id, tab.path.clone(), None);
                }
                Status::Ready => {
                    self.doc = tab.doc.take();
                    self.turns = tab.turns;
                    let (zoom, spot) = (tab.zoom, tab.spot);
                    self.sync_ui(ui);
                    self.show_doc(ui, zoom, spot);
                }
                Status::Loading | Status::Failed(_) => {}
            }
        }
        self.sync_ui(ui);
        self.save_session(ui);
        store::save_recents(&self.recents);
    }

    pub(crate) fn open_path(&mut self, ui: &AppWindow, path: PathBuf) {
        self.open_at(ui, path, None);
    }

    /// Opens a file, or switches to its tab, optionally at a 1-based page.
    pub(crate) fn open_at(&mut self, ui: &AppWindow, path: PathBuf, page: Option<usize>) {
        if let Some(i) = self.tabs.iter().position(|t| store::same_file(&t.path, &path)) {
            if matches!(self.tabs[i].status, Status::Failed(_)) {
                self.tabs[i].status = Status::Unloaded;
                if self.active == Some(i) {
                    self.active = None;
                }
            }
            if let Some(p) = page {
                self.tabs[i].spot = Spot { page: p.saturating_sub(1), fy: 0.0 };
            }
            let was_active = self.active == Some(i);
            self.activate(ui, Some(i));
            if let (true, Some(p)) = (was_active, page) {
                self.go_to(ui, p.saturating_sub(1), 0.0);
            }
            return;
        }
        let remembered = self.recents.iter().find(|r| store::same_file(&r.path, &path)).map(|r| r.spot).unwrap_or_default();
        let spot = page.map(|p| Spot { page: p.saturating_sub(1), fy: 0.0 }).unwrap_or(remembered);
        let i = self.new_tab(path, self.settings.default_zoom, spot, 0);
        self.activate(ui, Some(i));
    }

    pub(crate) fn close_tab(&mut self, ui: &AppWindow, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let was_active = self.active == Some(index);
        if was_active {
            self.park_active(ui);
            self.active = None;
        }
        let tab = self.tabs.remove(index);
        self.renderer.close(tab.id);
        self.closed.push((tab.path.clone(), tab.spot.page + 1));
        if self.closed.len() > 20 {
            self.closed.remove(0);
        }
        let next = match self.active {
            Some(a) if a > index => Some(a - 1),
            Some(a) => Some(a),
            None if was_active && !self.tabs.is_empty() => Some(index.min(self.tabs.len() - 1)),
            None => None,
        };
        self.active = None;
        self.activate(ui, next);
    }

    pub(crate) fn close_other_tabs(&mut self, ui: &AppWindow, keep: usize) {
        if keep >= self.tabs.len() {
            return;
        }
        self.activate(ui, Some(keep));
        self.park_active(ui);
        let kept = self.tabs.remove(keep);
        for tab in self.tabs.drain(..) {
            self.renderer.close(tab.id);
        }
        self.tabs.push(kept);
        self.active = None;
        self.activate(ui, Some(0));
    }

    /// Moves a tab to another place in the strip, keeping the same tab in front.
    pub(crate) fn move_tab(&mut self, ui: &AppWindow, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        // Pinned tabs stay among themselves, and the others among theirs.
        let pinned = self.tabs.iter().filter(|t| t.pinned).count();
        let to = if self.tabs[from].pinned { to.min(pinned.saturating_sub(1)) } else { to.max(pinned) };
        if from == to {
            return;
        }
        let front = self.active.map(|a| self.tabs[a].id);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = front.and_then(|id| self.tabs.iter().position(|t| t.id == id));
        self.sync_ui(ui);
        self.save_session(ui);
    }

    /// Pins the tab (it moves behind the other pinned ones) or unpins it (it moves in front of the rest).
    pub(crate) fn toggle_pin(&mut self, ui: &AppWindow, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let front = self.active.map(|a| self.tabs[a].id);
        let mut tab = self.tabs.remove(index);
        tab.pinned = !tab.pinned;
        let pinned = self.tabs.iter().filter(|t| t.pinned).count();
        self.tabs.insert(pinned, tab);
        self.active = front.and_then(|id| self.tabs.iter().position(|t| t.id == id));
        self.sync_ui(ui);
        self.save_session(ui);
    }

    /// Ctrl+Shift+PageUp and Ctrl+Shift+PageDown.
    pub(crate) fn move_active_tab(&mut self, ui: &AppWindow, step: i32) {
        let Some(a) = self.active else { return };
        let to = (a as i32 + step).clamp(0, self.tabs.len() as i32 - 1) as usize;
        self.move_tab(ui, a, to);
    }

    /// Ctrl+Shift+T: the tab closed last comes back where it was reading.
    pub(crate) fn reopen_closed_tab(&mut self, ui: &AppWindow) {
        match self.closed.pop() {
            Some((path, page)) => self.open_at(ui, path, Some(page)),
            None => self.notify(ui, "There is no closed tab to reopen."),
        }
    }

    pub(crate) fn cycle_tab(&mut self, ui: &AppWindow, step: i32) {
        let n = self.tabs.len() as i32;
        if n == 0 {
            return;
        }
        let next = match self.active {
            Some(a) => (a as i32 + step).rem_euclid(n),
            None if step > 0 => 0,
            None => n - 1,
        };
        self.activate(ui, Some(next as usize));
    }

    pub(crate) fn on_document(&mut self, ui: &AppWindow, event: Event) {
        let current = self.doc.as_ref().map(|d| d.id);
        let (id, result) = match event {
            Event::Opened { doc, sizes, outline } => (doc, Ok((sizes, outline))),
            Event::Failed { doc, message } => (doc, Err(message)),
            Event::NeedPassword { doc, wrong } => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == doc) {
                    tab.status = Status::Failed("This file is protected with a password.".into());
                    tab.locked = true;
                    tab.password_wrong = wrong;
                    self.sync_ui(ui);
                }
                return;
            }
            Event::Rendered { req, pixels } => return self.on_rendered(ui, req, pixels),
            Event::SearchHits { doc, generation, page, hits } if Some(doc) == current => {
                return self.on_search_hits(ui, generation, page, hits);
            }
            Event::SearchDone { doc, generation } if Some(doc) == current => return self.on_search_done(ui, generation),
            Event::Selected { doc, generation, pages, text } if Some(doc) == current => {
                return self.on_selected(ui, generation, pages, text);
            }
            Event::Properties { doc, rows } if Some(doc) == current => return self.show_properties_rows(ui, rows),
            Event::Link { doc, target } if Some(doc) == current => return self.on_link(ui, target),
            Event::Printed { pages, message } => return self.on_printed(ui, pages, message),
            Event::AnnotationList { doc, rows } if Some(doc) == current => return self.show_annotations(ui, rows),
            Event::AnnotationList { .. } => return,
            Event::History { doc, undo, redo } if Some(doc) == current => return self.show_history(ui, undo, redo),
            Event::History { .. } => return,
            Event::SearchHits { .. } | Event::SearchDone { .. } | Event::Selected { .. } | Event::Link { .. } | Event::Properties { .. } => return,
            Event::Annotated { doc, pages, message } => return self.on_annotated(ui, doc, pages, message),
            Event::PageText { doc, text } if Some(doc) == current => return self.on_page_text(ui, text),
            Event::PageText { .. } => return,
            Event::Saved { doc, path, token, error, backup } => return self.on_saved(ui, doc, path, token, error, backup),
        };
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return };
        match result {
            Ok((sizes, outline)) if !sizes.is_empty() => {
                let max_width = sizes.iter().map(|s| s.0).fold(1.0, f32::max);
                let max_height = sizes.iter().map(|s| s.1).fold(1.0, f32::max);
                let doc = Doc { id, sizes, max_width, max_height, outline };
                let tab = &mut self.tabs[i];
                tab.status = Status::Ready;
                let (path, zoom, spot, turns) = (tab.path.clone(), tab.zoom, tab.spot, tab.turns);
                store::touch_recent(&mut self.recents, &path, Some(spot));
                if self.active == Some(i) {
                    self.doc = Some(doc);
                    self.turns = turns;
                    self.sync_ui(ui);
                    self.show_doc(ui, zoom, spot);
                } else {
                    self.tabs[i].doc = Some(doc);
                }
                store::save_recents(&self.recents);
            }
            Ok(_) => self.tabs[i].status = Status::Failed("The document has no pages.".into()),
            Err(message) => self.tabs[i].status = Status::Failed(message),
        }
        self.sync_ui(ui);
    }

    pub(crate) fn is_favorite(&self, path: &Path) -> bool {
        self.favorites.iter().any(|f| store::same_file(f, path))
    }

    /// Pushes tab, Home and state information to the window.
    pub(crate) fn sync_ui(&mut self, ui: &AppWindow) {
        let infos: Vec<TabInfo> = self
            .tabs
            .iter()
            .map(|t| TabInfo {
                title: if t.dirty { format!("\u{2022} {}", t.title) } else { t.title.clone() }.into(),
                path: t.path.display().to_string().into(),
                busy: matches!(t.status, Status::Loading),
                failed: matches!(t.status, Status::Failed(_)),
                favorite: self.is_favorite(&t.path),
                pinned: t.pinned,
            })
            .collect();
        self.tab_model.set_vec(infos);
        let b = ui.global::<Bridge>();
        b.set_pinned_count(self.tabs.iter().filter(|t| t.pinned).count() as i32);
        b.set_active_tab(self.active.map(|a| a as i32).unwrap_or(-1));

        match self.active.map(|a| &self.tabs[a]) {
            None => {
                b.set_doc_title(SharedString::new());
                b.set_state(DocState::Empty);
                self.sync_home(ui);
            }
            Some(tab) => {
                b.set_doc_title(tab.title.as_str().into());
                match &tab.status {
                    Status::Unloaded | Status::Loading => b.set_state(DocState::Loading),
                    Status::Ready => b.set_state(DocState::Ready),
                    Status::Failed(message) => {
                        b.set_error_text(message.as_str().into());
                        b.set_needs_password(tab.locked);
                        b.set_password_wrong(tab.password_wrong);
                        b.set_state(DocState::Failed);
                    }
                }
            }
        }
    }

    fn item_for(&self, path: &Path, opened: Option<u64>, page: usize) -> RecentItem {
        let now = store::now();
        RecentItem {
            name: title_of(path).into(),
            folder: folder_of(path).into(),
            when: opened.map(|t| store::describe_time(t, now)).unwrap_or_default().into(),
            page: page as i32 + 1,
            favorite: self.is_favorite(path),
            missing: !path.exists(),
        }
    }

    /// Recent files, favorites, bookmarked files and the preference buttons.
    pub(crate) fn sync_home(&mut self, ui: &AppWindow) {
        let recents: Vec<RecentItem> = self.recents.iter().map(|r| self.item_for(&r.path, Some(r.opened), r.spot.page)).collect();
        self.recent_model.set_vec(recents);
        let favorites: Vec<RecentItem> = self
            .favorites
            .iter()
            .map(|p| {
                let recent = self.recents.iter().find(|r| store::same_file(&r.path, p));
                self.item_for(p, recent.map(|r| r.opened), recent.map(|r| r.spot.page).unwrap_or(0))
            })
            .collect();
        self.favorite_model.set_vec(favorites);
        let files: Vec<BookmarkFile> = self
            .bookmarks
            .iter()
            .map(|(path, pages)| BookmarkFile {
                name: title_of(path).into(),
                pages: ModelRc::new(VecModel::from(pages.iter().map(|&p| p as i32).collect::<Vec<_>>())),
            })
            .collect();
        self.bookmark_file_model.set_vec(files);
        let b = ui.global::<Bridge>();
        b.set_restore_tabs(self.settings.restore_tabs);
        b.set_auto_reload(self.settings.auto_reload);
        b.set_default_zoom(match self.settings.default_zoom {
            Zoom::Fit => 1,
            Zoom::Width => 2,
            Zoom::Actual => 3,
            _ => 0,
        });
        ui.global::<Theme>().set_choice(self.settings.theme);
    }

    pub(crate) fn open_recent(&mut self, ui: &AppWindow, row: usize) {
        if let Some(path) = self.recents.get(row).map(|r| r.path.clone()) {
            self.open_path(ui, path);
        }
    }

    /// Middle click on Home: the file gets a tab of its own and Home stays in front.
    pub(crate) fn open_background(&mut self, ui: &AppWindow, path: Option<PathBuf>) {
        let Some(path) = path else { return };
        if self.tabs.iter().any(|t| store::same_file(&t.path, &path)) {
            return;
        }
        let spot = self.recents.iter().find(|r| store::same_file(&r.path, &path)).map(|r| r.spot).unwrap_or_default();
        self.new_tab(path, self.settings.default_zoom, spot, 0);
        self.sync_ui(ui);
        self.save_session(ui);
    }

    pub(crate) fn open_favorite(&mut self, ui: &AppWindow, row: usize) {
        if let Some(path) = self.favorites.get(row).cloned() {
            self.open_path(ui, path);
        }
    }

    pub(crate) fn toggle_favorite(&mut self, ui: &AppWindow, path: &Path) {
        match self.favorites.iter().position(|f| store::same_file(f, path)) {
            Some(i) => {
                self.favorites.remove(i);
            }
            None => self.favorites.push(path.to_path_buf()),
        }
        store::save_favorites(&self.favorites);
        self.sync_ui(ui);
        self.sync_home(ui);
    }

    pub(crate) fn recent_path(&self, row: usize) -> Option<PathBuf> {
        self.recents.get(row).map(|r| r.path.clone())
    }

    pub(crate) fn tab_path(&self, index: usize) -> Option<PathBuf> {
        self.tabs.get(index).map(|t| t.path.clone())
    }

    pub(crate) fn remove_recent(&mut self, ui: &AppWindow, row: usize) {
        if row < self.recents.len() {
            self.recents.remove(row);
            store::save_recents(&self.recents);
            self.sync_home(ui);
        }
    }

    pub(crate) fn clear_recents(&mut self, ui: &AppWindow) {
        self.recents.clear();
        store::save_recents(&self.recents);
        self.sync_home(ui);
    }

    pub(crate) fn remove_favorite(&mut self, ui: &AppWindow, row: usize) {
        if row < self.favorites.len() {
            self.favorites.remove(row);
            store::save_favorites(&self.favorites);
            self.sync_ui(ui);
            self.sync_home(ui);
        }
    }

    /// The window as it is now; while maximized, the size it returns to keeps the last normal one.
    fn placement(&mut self, ui: &AppWindow) -> Option<Placement> {
        let window = ui.window();
        if window.is_minimized() || window.is_fullscreen() {
            return self.last_normal;
        }
        let maximized = window.is_maximized();
        if !maximized {
            let (pos, size) = (window.position(), window.size());
            self.last_normal = Some(Placement { x: pos.x, y: pos.y, width: size.width, height: size.height, maximized: false });
        }
        self.last_normal.map(|p| Placement { maximized, ..p })
    }

    pub(crate) fn save_session(&mut self, ui: &AppWindow) {
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let live = self.active == Some(i) && self.doc.is_some();
                SessionTab {
                    path: t.path.clone(),
                    spot: if live { self.current_spot(ui).unwrap_or(t.spot) } else { t.spot },
                    zoom: if live { self.zoom } else { t.zoom },
                    turns: if live { self.turns } else { t.turns },
                    pinned: t.pinned,
                }
            })
            .collect();
        let window = self.placement(ui);
        store::save_session(&Session { tabs, active: self.active, window });
    }

    pub(crate) fn save_settings(&mut self, ui: &AppWindow) {
        self.settings.sidebar = ui.global::<Bridge>().get_sidebar_open();
        if self.presenting.is_none() {
            self.settings.scroll_mode = self.scroll_mode;
            self.settings.spread_mode = self.spread_mode;
        }
        self.settings.tone = self.tone as i32;
        store::save_settings(&self.settings);
    }

    /// Called when the window closes: remember positions in the session, the recent list and the settings.
    pub(crate) fn save_all(&mut self, ui: &AppWindow) {
        if self.presenting.is_some() {
            self.toggle_presentation(ui);
        }
        if let (Some(a), Some(spot)) = (self.active, self.current_spot(ui)) {
            let path = self.tabs[a].path.clone();
            if let Some(r) = self.recents.iter_mut().find(|r| store::same_file(&r.path, &path)) {
                r.spot = spot;
            }
        }
        self.save_session(ui);
        self.save_settings(ui);
        store::save_recents(&self.recents);
    }

    pub(crate) fn restore(&mut self, ui: &AppWindow, session: Session) {
        for t in session.tabs {
            let i = self.new_tab(t.path, t.zoom, t.spot, t.turns);
            self.tabs[i].pinned = t.pinned;
        }
        self.sync_ui(ui);
        if let Some(a) = session.active {
            self.activate(ui, Some(a));
        }
    }

    /// Reloads tabs whose file changed on disk, keeping their page and zoom.
    pub(crate) fn reload_changed(&mut self, ui: &AppWindow) {
        if !self.settings.auto_reload {
            return;
        }
        let changed: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| matches!(t.status, Status::Ready) && t.stamp.is_some() && !t.dirty)
            .filter(|(_, t)| {
                let now = store::modified(&t.path);
                now.is_some() && now != t.stamp
            })
            .map(|(i, _)| i)
            .collect();
        for i in changed {
            let active = self.active == Some(i);
            if active {
                self.park_active(ui);
                self.active = None;
            }
            self.renderer.close(self.tabs[i].id);
            self.next_id += 1;
            let id = self.next_id;
            let tab = &mut self.tabs[i];
            tab.id = id;
            tab.doc = None;
            tab.status = Status::Unloaded;
            tab.stamp = None;
            if active {
                self.activate(ui, Some(i));
            }
        }
    }
}
