// SPDX-License-Identifier: GPL-3.0-or-later

//! Toolbar actions: presentation, document properties, print, save a copy, page bookmarks, theme.

use std::path::PathBuf;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::render::{LinkJob, LinkTarget, PrintJob};
use crate::store::{self, Zoom};
use crate::view::SCROLL_PAGE;
use crate::*;

/// What presentation mode changed, to put back when it ends.
pub(crate) struct Presenting {
    zoom: Zoom,
    scroll_mode: i32,
    spread_mode: i32,
    fullscreen: bool,
    hand: bool,
}

impl Viewer {
    pub(crate) fn notify(&self, ui: &AppWindow, text: &str) {
        ui.global::<Bridge>().set_notice(text.into());
    }

    pub(crate) fn active_path(&self) -> Option<PathBuf> {
        self.active.map(|a| self.tabs[a].path.clone())
    }

    /// Full screen, one page at a time, fitted, on black; arrows, space and clicks turn pages.
    pub(crate) fn toggle_presentation(&mut self, ui: &AppWindow) {
        let b = ui.global::<Bridge>();
        let window = ui.window();
        match self.presenting.take() {
            Some(saved) => {
                b.set_presenting(false);
                if !saved.fullscreen {
                    window.set_fullscreen(false);
                    b.set_fullscreen(false);
                }
                b.set_hand_tool(saved.hand);
                self.zoom = saved.zoom;
                self.set_layout_modes(ui, saved.scroll_mode, saved.spread_mode);
                self.set_zoom(ui, saved.zoom);
            }
            None => {
                if self.doc.is_none() {
                    return;
                }
                self.presenting = Some(Presenting {
                    zoom: self.zoom,
                    scroll_mode: self.scroll_mode,
                    spread_mode: self.spread_mode,
                    fullscreen: window.is_fullscreen(),
                    hand: b.get_hand_tool(),
                });
                b.set_presenting(true);
                b.set_hand_tool(false);
                window.set_fullscreen(true);
                b.set_fullscreen(true);
                self.selection = None;
                self.zoom = Zoom::Fit;
                self.set_layout_modes(ui, SCROLL_PAGE, 0);
                // The window size changes when full screen takes effect; refit then.
                let weak = ui.as_weak();
                slint::Timer::single_shot(std::time::Duration::from_millis(250), move || {
                    if weak.upgrade().is_some() {
                        with_viewer(|viewer, ui| {
                            if viewer.presenting.is_some() {
                                viewer.set_zoom(ui, Zoom::Fit);
                            }
                        });
                    }
                });
            }
        }
    }

    /// A click on the page: follows a link if there is one under the pointer.
    pub(crate) fn click_at(&mut self, _ui: &AppWindow, x: f32, y: f32) {
        let Some(doc) = self.doc.as_ref().map(|d| d.id) else { return };
        let Some(page) = self.page_near(x, y) else { return };
        let Some([px, py, w, h]) = self.page_rect(page) else { return };
        if x < px || y < py || x > px + w || y > py + h {
            return;
        }
        let point = [(x - px) / w, (y - py) / h];
        self.renderer.link(LinkJob { doc, page, turns: self.turns, point });
    }

    pub(crate) fn on_link(&mut self, ui: &AppWindow, target: LinkTarget) {
        match target {
            LinkTarget::Uri(uri) => {
                let lower = uri.to_ascii_lowercase();
                if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
                    platform::shell_open(&uri);
                } else {
                    self.notify(ui, "This link points to something other than a web page, so it was not opened.");
                }
            }
            LinkTarget::Page(page) => {
                let here = ui.global::<Bridge>().get_current_page().max(1) as usize - 1;
                self.back.push(here);
                self.forward.clear();
                self.go_to(ui, page, 0.0);
            }
        }
    }

    /// Alt+Left and Alt+Right: back to where a link was followed from, and forward again.
    pub(crate) fn history_step(&mut self, ui: &AppWindow, step: i32) {
        let here = ui.global::<Bridge>().get_current_page().max(1) as usize - 1;
        let target = if step < 0 { self.back.pop() } else { self.forward.pop() };
        let Some(target) = target else { return };
        if step < 0 { self.forward.push(here) } else { self.back.push(here) }
        self.go_to(ui, target, 0.0);
    }

    /// Tries the typed password on the file in the front tab.
    pub(crate) fn submit_password(&mut self, ui: &AppWindow, password: String) {
        let Some(a) = self.active else { return };
        let tab = &mut self.tabs[a];
        if !tab.locked || password.is_empty() {
            return;
        }
        tab.locked = false;
        tab.status = tabs::Status::Loading;
        self.renderer.open(tab.id, tab.path.clone(), Some(password));
        self.sync_ui(ui);
    }

    pub(crate) fn show_properties(&mut self) {
        if let Some(doc) = self.doc.as_ref().map(|d| d.id) {
            self.renderer.properties(doc);
        }
    }

    /// Adds what is known about the file and the current page to what PDFium read.
    pub(crate) fn show_properties_rows(&mut self, ui: &AppWindow, rows: Vec<(String, String)>) {
        let Some(path) = self.active_path() else { return };
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let size_text = if size >= 1 << 20 {
            format!("{:.1} MB ({} bytes)", size as f64 / (1 << 20) as f64, size)
        } else {
            format!("{:.0} KB ({} bytes)", size as f64 / 1024.0, size)
        };
        let page = ui.global::<Bridge>().get_current_page().max(1) as usize - 1;
        let (w, h) = self.doc.as_ref().and_then(|d| d.sizes.get(page).copied()).unwrap_or((0.0, 0.0));
        let page_text = format!(
            "{:.0} × {:.0} mm ({:.2} × {:.2} in)",
            w / 72.0 * 25.4,
            h / 72.0 * 25.4,
            w / 72.0,
            h / 72.0
        );
        let mut all = vec![
            ("File name".to_string(), tabs::title_of(&path)),
            ("Location".to_string(), path.parent().map(|p| p.display().to_string()).unwrap_or_default()),
            ("File size".to_string(), size_text),
        ];
        all.extend(rows);
        all.push(("Page size".into(), page_text));
        all.push(("Page count".into(), self.doc.as_ref().map(|d| d.sizes.len()).unwrap_or(0).to_string()));
        let rows: Vec<PropRow> = all.into_iter().map(|(label, value)| PropRow { label: label.into(), value: value.into() }).collect();
        let b = ui.global::<Bridge>();
        b.set_properties(ModelRc::new(VecModel::from(rows)));
        b.set_properties_open(true);
    }

    pub(crate) fn print(&mut self, ui: &AppWindow) {
        let Some(doc) = self.doc.as_ref() else { return };
        let (id, count) = (doc.id, doc.sizes.len());
        let current = ui.global::<Bridge>().get_current_page().max(1) as usize - 1;
        let Some((hdc, pages)) = platform::print_dialog(count, current) else { return };
        let title = self.active.map(|a| self.tabs[a].title.clone()).unwrap_or_default();
        self.notify(ui, &format!("Printing {} page{}", pages.len(), if pages.len() == 1 { "" } else { "s" }));
        self.renderer.print(PrintJob { doc: id, hdc, pages, title, actual_size: self.settings.print_scale == 1 });
    }

    pub(crate) fn on_printed(&mut self, ui: &AppWindow, pages: usize, message: Option<String>) {
        match message {
            Some(m) => {
                store::log_error(&format!("Print failed: {m}"));
                self.notify(ui, &m);
            }
            None => self.notify(ui, &format!("Sent {pages} page{} to the printer", if pages == 1 { "" } else { "s" })),
        }
    }

    /// Writes the changes into the file itself; the first time, the earlier version is kept beside it.
    pub(crate) fn save_original(&mut self, ui: &AppWindow) {
        let Some(path) = self.active_path() else { return };
        let Some(doc) = self.active.map(|a| &self.tabs[a]).filter(|t| t.dirty).map(|t| t.id) else {
            self.notify(ui, "There is nothing to save.");
            return;
        };
        let token = self.next_token();
        self.in_place.insert(token);
        self.renderer.edit(render::EditJob::SaveOriginal { doc, path, token });
    }

    /// Copies the file elsewhere; the original is never changed.
    pub(crate) fn save_copy(&mut self, ui: &AppWindow) {
        let Some(path) = self.active_path() else { return };
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
        let Some(target) = platform::pick_save_path(&format!("{stem} (copy).pdf")) else { return };
        if store::same_file(&target, &path) {
            self.notify(ui, "Choose a different name: a copy cannot replace the file itself.");
            return;
        }
        if let Some(doc) = self.active.map(|a| &self.tabs[a]).filter(|t| t.dirty).map(|t| t.id) {
            let token = self.next_token();
            self.renderer.edit(render::EditJob::Save { doc, path: target, token });
            return;
        }
        match std::fs::copy(&path, &target) {
            Ok(_) => self.notify(ui, &format!("Saved a copy as {}", tabs::title_of(&target))),
            Err(e) => {
                store::log_error(&format!("Save a copy failed: {e}"));
                self.notify(ui, &format!("The copy could not be saved: {e}"));
            }
        }
    }

    /// A copy with the annotations drawn into the pages. The open document stays editable.
    pub(crate) fn save_flat_copy(&mut self, ui: &AppWindow) {
        let Some(path) = self.active_path() else { return };
        let Some(doc) = self.active_id() else { return };
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
        let Some(target) = platform::pick_save_path(&format!("{stem} (flattened).pdf")) else { return };
        if store::same_file(&target, &path) {
            self.notify(ui, "Choose a different name: a copy cannot replace the file itself.");
            return;
        }
        let token = self.next_token();
        self.flat_tokens.insert(token);
        self.renderer.edit(render::EditJob::SaveFlat { doc, path: target, token });
    }

    pub(crate) fn bookmarks_for(&self, path: &std::path::Path) -> Vec<usize> {
        self.bookmarks.iter().find(|(p, _)| store::same_file(p, path)).map(|(_, pages)| pages.clone()).unwrap_or_default()
    }

    /// Whether the page in view is bookmarked, and the list the tag button shows.
    pub(crate) fn sync_bookmark_state(&self, ui: &AppWindow) {
        let pages = self.active_path().map(|p| self.bookmarks_for(&p)).unwrap_or_default();
        let b = ui.global::<Bridge>();
        let current = b.get_current_page().max(1) as usize;
        b.set_bookmarked(pages.contains(&current));
        b.set_bookmark_pages(ModelRc::new(VecModel::from(pages.iter().map(|&p| p as i32).collect::<Vec<_>>())));
    }

    pub(crate) fn toggle_bookmark(&mut self, ui: &AppWindow) {
        let page = ui.global::<Bridge>().get_current_page().max(1) as usize;
        self.toggle_bookmark_page(ui, page);
    }

    /// Where the right-click landed, for the page menu: which page, and whether it is bookmarked.
    pub(crate) fn page_menu_prepare(&mut self, ui: &AppWindow, x: f32, y: f32) {
        self.menu_point = Some((x, y));
        let Some(page) = self.page_near(x, y) else { return };
        let marked = self.active_path().map(|p| self.bookmarks_for(&p).contains(&(page + 1))).unwrap_or(false);
        let b = ui.global::<Bridge>();
        b.set_menu_page(page as i32);
        b.set_menu_bookmarked(marked);
    }

    /// The page whose thumbnail was right-clicked (0-based), for the same menu.
    pub(crate) fn thumb_menu_prepare(&mut self, ui: &AppWindow, page: usize) {
        let marked = self.active_path().map(|p| self.bookmarks_for(&p).contains(&(page + 1))).unwrap_or(false);
        let b = ui.global::<Bridge>();
        b.set_menu_page(page as i32);
        b.set_menu_bookmarked(marked);
    }

    /// Bookmarks or un-bookmarks a page (1-based).
    pub(crate) fn toggle_bookmark_page(&mut self, ui: &AppWindow, page: usize) {
        let Some(path) = self.active_path() else { return };
        match self.bookmarks.iter_mut().find(|(p, _)| store::same_file(p, &path)) {
            Some((_, pages)) => match pages.iter().position(|&p| p == page) {
                Some(i) => {
                    pages.remove(i);
                }
                None => {
                    pages.push(page);
                    pages.sort_unstable();
                }
            },
            None => self.bookmarks.push((path.clone(), vec![page])),
        }
        self.bookmarks.retain(|(_, pages)| !pages.is_empty());
        store::save_bookmarks(&self.bookmarks);
        self.sync_bookmark_state(ui);
        self.refresh_thumbs(ui);
        let marked = self.bookmarks_for(&path).contains(&page);
        self.notify(ui, &if marked { format!("Bookmarked page {page}") } else { format!("Removed the bookmark from page {page}") });
    }

    pub(crate) fn clear_bookmarks(&mut self, ui: &AppWindow) {
        let Some(path) = self.active_path() else { return };
        self.bookmarks.retain(|(p, _)| !store::same_file(p, &path));
        store::save_bookmarks(&self.bookmarks);
        self.sync_bookmark_state(ui);
        self.refresh_thumbs(ui);
    }

    pub(crate) fn open_bookmark(&mut self, ui: &AppWindow, file: usize, page: usize) {
        if let Some(path) = self.bookmarks.get(file).map(|(p, _)| p.clone()) {
            self.open_at(ui, path, Some(page));
        }
    }

    pub(crate) fn set_theme(&mut self, ui: &AppWindow, choice: i32) {
        self.settings.theme = choice.clamp(0, 2);
        ui.global::<Theme>().set_choice(self.settings.theme);
        store::save_settings(&self.settings);
    }
}
