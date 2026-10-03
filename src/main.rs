// SPDX-License-Identifier: GPL-3.0-or-later
#![windows_subsystem = "windows"]

mod actions;
mod annotate;
mod find;
mod platform;
mod render;
mod single;
mod sidebar;
mod store;
mod tabs;
mod titlebar;
mod undo;
mod view;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
use slint::{CloseRequestResponse, ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use render::{OutlineItem, Renderer, Tone};
use store::Zoom;
use tabs::Tab;

slint::slint! {
    export {
        AppWindow, Bridge, Theme, Metrics, DocState, PageView, Mark, ThumbView, OutlineRow, TabInfo, RecentItem,
        BookmarkFile, PropRow, AnnotationRow
    } from "ui/app.slint";
}

/// Logical pixels per PDF point at 100% zoom.
const BASE_SCALE: f32 = 96.0 / 72.0;
const MARGIN: f32 = 20.0;
const GAP: f32 = 12.0;
const ZOOM_STEPS: [f32; 19] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0,
];
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 8.0;
/// Whole-page bitmaps stop at this size; past it, a sharp tile covers only the visible part.
const BASE_PIXELS: f32 = 4_000_000.0;
/// Extra area rendered around the viewport so short scrolls stay sharp.
const TILE_MARGIN: f32 = 192.0;
const CACHE_PAGES: usize = 8;

struct Doc {
    id: u64,
    /// Page sizes in points before the user's rotation.
    sizes: Vec<(f32, f32)>,
    #[allow(dead_code)]
    max_width: f32,
    #[allow(dead_code)]
    max_height: f32,
    outline: Vec<OutlineItem>,
}

/// A sharp render of part of a page, placed by fractions of the page so it survives zoom changes.
struct Detail {
    width: u32,
    px: [u32; 4],
    frac: [f32; 4],
    image: Image,
}

struct Viewer {
    ui: slint::Weak<AppWindow>,
    renderer: Renderer,
    next_id: u64,
    tabs: Vec<Tab>,
    active: Option<usize>,
    settings: store::Settings,
    recents: Vec<store::Recent>,
    favorites: Vec<PathBuf>,
    bookmarks: Vec<(PathBuf, Vec<usize>)>,
    tab_model: Rc<VecModel<TabInfo>>,
    recent_model: Rc<VecModel<RecentItem>>,
    favorite_model: Rc<VecModel<RecentItem>>,
    bookmark_file_model: Rc<VecModel<BookmarkFile>>,
    last_normal: Option<store::Placement>,
    presenting: Option<actions::Presenting>,
    stroke: Option<annotate::Stroke>,
    text_target: Option<annotate::TextTarget>,
    /// The text being typed becomes a sticky note instead of text on the page.
    note_mode: bool,
    /// Sticky notes of the open document: page, index, area as page fractions, text.
    annots: Vec<(usize, usize, &'static str, [f32; 4], String)>,
    /// The annotation picked on the page, and a drag that moves or resizes it.
    selected: Option<annotate::Selected>,
    annot_drag: Option<annotate::AnnotDrag>,
    /// The note whose editor is open: page and index.
    note_edit: Option<(usize, usize)>,
    /// Where the page menu was opened, in document coordinates.
    menu_point: Option<(f32, f32)>,
    /// What waits for the answer to "save your changes?".
    pending: Option<annotate::Pending>,
    confirm_tab: Option<u64>,
    save_continue: Option<u64>,
    /// Tokens of saves that write into the file itself, not into a copy.
    in_place: std::collections::HashSet<u64>,
    token: u64,
    speaker: Option<platform::Speaker>,
    /// Tabs closed lately, newest last, as file and 1-based page, for Ctrl+Shift+T.
    closed: Vec<(PathBuf, usize)>,
    /// Pages left by following a link, for Alt+Left and Alt+Right.
    back: Vec<usize>,
    forward: Vec<usize>,
    // The view below always belongs to the active tab.
    doc: Option<Doc>,
    zoom: Zoom,
    scale: f32,
    scroll_mode: i32,
    spread_mode: i32,
    /// The pair of pages (or single page) shown in one-page-at-a-time mode.
    page_group: usize,
    rects: Vec<Option<[f32; 4]>>,
    content_w: f32,
    content_h: f32,
    last_viewport: (f32, f32),
    pinch_start: f32,
    pinching: bool,
    visible: (usize, usize),
    cache: HashMap<usize, (u32, Image)>,
    details: HashMap<usize, Detail>,
    model: Rc<VecModel<PageView>>,
    turns: u8,
    tone: Tone,
    search: Option<find::SearchState>,
    search_gen: u64,
    find_opts: render::FindOpts,
    highlight_all: bool,
    selection: Option<find::Selection>,
    select_gen: u64,
    thumbs: HashMap<usize, (u32, Image)>,
    thumb_model: Rc<VecModel<ThumbView>>,
    mark_model: Rc<VecModel<ThumbView>>,
    annot_model: Rc<VecModel<AnnotationRow>>,
    outline_model: Rc<VecModel<OutlineRow>>,
}

thread_local! {
    static VIEWER: RefCell<Option<Viewer>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

fn with_window(f: impl FnOnce(&AppWindow) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = WINDOW.with(|w| w.borrow().as_ref().and_then(|w| w.upgrade())) {
            f(&ui);
        }
    });
}

/// Runs inside the window procedure, so the UI update waits for the event loop.
fn on_caption_state(maximized: bool, hover: bool, pressed: bool) {
    with_window(move |ui| {
        let b = ui.global::<Bridge>();
        b.set_is_maximized(maximized);
        b.set_max_hover(hover);
        b.set_max_pressed(pressed);
    });
}

fn on_settings_change() {
    with_window(|ui| ui.global::<Theme>().set_system_dark(platform::system_dark()));
}

fn with_viewer(f: impl FnOnce(&mut Viewer, &AppWindow)) {
    VIEWER.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else { return };
        let Some(viewer) = slot.as_mut() else { return };
        let Some(ui) = viewer.ui.upgrade() else { return };
        f(viewer, &ui);
    });
}

fn icon(bytes: &[u8], size: u32) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(bytes, size, size))
}

fn tone_of(i: i32) -> Tone {
    match i {
        1 => Tone::Dark,
        2 => Tone::Sepia,
        _ => Tone::Normal,
    }
}

fn main() -> Result<(), slint::PlatformError> {
    std::panic::set_hook(Box::new(|info| store::log_error(&format!("Crash: {info}"))));
    // Files go to the window that is already open, when there is one.
    let launch_files: Vec<PathBuf> = std::env::args_os().skip(1).filter(|a| !a.is_empty()).map(PathBuf::from).collect();
    if single::hand_over(&launch_files) {
        return Ok(());
    }
    platform::set_app_id();
    let ui = AppWindow::new()?;
    let b = ui.global::<Bridge>();
    ui.global::<Metrics>().set_touch(platform::prefers_touch());
    ui.global::<Theme>().set_system_dark(platform::system_dark());
    b.set_app_mark(icon(include_bytes!("../ui/img/app-mark.rgba"), 40));
    b.set_file_icon(icon(include_bytes!("../ui/img/tab-icon.rgba"), 64));
    b.set_app_icon(icon(include_bytes!("../ui/img/app-icon.rgba"), 128));
    b.set_version(env!("CARGO_PKG_VERSION").into());

    let renderer = Renderer::start(move |event| {
        let _ = slint::invoke_from_event_loop(move || {
            with_viewer(|viewer, ui| viewer.on_document(ui, event));
        });
    });

    let settings = store::load_settings();
    let model = Rc::new(VecModel::default());
    let tab_model = Rc::new(VecModel::default());
    let recent_model = Rc::new(VecModel::default());
    let favorite_model = Rc::new(VecModel::default());
    let bookmark_file_model = Rc::new(VecModel::default());
    let thumb_model = Rc::new(VecModel::default());
    let outline_model = Rc::new(VecModel::default());
    let mark_model = Rc::new(VecModel::default());
    let annot_model = Rc::new(VecModel::default());
    b.set_annotations(ModelRc::from(annot_model.clone()));
    b.set_thumbs(ModelRc::from(thumb_model.clone()));
    b.set_mark_thumbs(ModelRc::from(mark_model.clone()));
    b.set_outline(ModelRc::from(outline_model.clone()));
    b.set_pages(ModelRc::from(model.clone()));
    b.set_tabs(ModelRc::from(tab_model.clone()));
    b.set_recents(ModelRc::from(recent_model.clone()));
    b.set_favorites(ModelRc::from(favorite_model.clone()));
    b.set_bookmark_files(ModelRc::from(bookmark_file_model.clone()));
    b.set_sidebar_open(settings.sidebar);
    b.set_page_tone(settings.tone);
    b.set_scroll_mode(settings.scroll_mode);
    b.set_spread_mode(settings.spread_mode);
    ui.global::<Theme>().set_choice(settings.theme);
    VIEWER.with(|cell| {
        *cell.borrow_mut() = Some(Viewer {
            ui: ui.as_weak(),
            renderer,
            next_id: 0,
            tabs: Vec::new(),
            active: None,
            tone: tone_of(settings.tone),
            scroll_mode: settings.scroll_mode,
            spread_mode: settings.spread_mode,
            settings,
            recents: store::load_recents(),
            favorites: store::load_favorites(),
            bookmarks: store::load_bookmarks(),
            tab_model,
            recent_model,
            favorite_model,
            bookmark_file_model,
            last_normal: None,
            presenting: None,
            stroke: None,
            text_target: None,
            note_mode: false,
            annots: Vec::new(),
            selected: None,
            annot_drag: None,
            note_edit: None,
            menu_point: None,
            pending: None,
            confirm_tab: None,
            save_continue: None,
            in_place: Default::default(),
            token: 0,
            speaker: None,
            closed: Vec::new(),
            back: Vec::new(),
            forward: Vec::new(),
            doc: None,
            zoom: Zoom::Auto,
            scale: BASE_SCALE,
            page_group: 0,
            rects: Vec::new(),
            content_w: 0.0,
            content_h: 0.0,
            last_viewport: (0.0, 0.0),
            pinch_start: BASE_SCALE,
            pinching: false,
            visible: (0, 0),
            cache: HashMap::new(),
            details: HashMap::new(),
            model,
            turns: 0,
            search: None,
            search_gen: 0,
            find_opts: render::FindOpts::default(),
            highlight_all: true,
            selection: None,
            select_gen: 0,
            thumbs: HashMap::new(),
            thumb_model,
            mark_model,
            annot_model,
            outline_model,
        })
    });

    // Tabs and Home
    b.on_open_file(|| {
        // The folder of the file opened last, so the dialog starts where the reader left off.
        let mut start = None;
        with_viewer(|viewer, _| start = viewer.recents.first().and_then(|r| r.path.parent().map(|p| p.to_path_buf())));
        if let Some(path) = platform::pick_pdf(start.as_deref()) {
            with_viewer(|viewer, ui| viewer.open_path(ui, path));
        }
    });
    b.on_select_tab(|i| with_viewer(|viewer, ui| viewer.activate(ui, usize::try_from(i).ok())));
    b.on_close_tab(|i| with_viewer(|viewer, ui| viewer.request_close_tab(ui, i as usize)));
    b.on_close_other_tabs(|i| with_viewer(|viewer, ui| viewer.request_close_others(ui, i as usize)));
    b.on_close_active_tab(|| {
        with_viewer(|viewer, ui| {
            if let Some(a) = viewer.active {
                viewer.request_close_tab(ui, a);
            }
        })
    });
    b.on_move_tab(|from, to| with_viewer(|viewer, ui| viewer.move_tab(ui, from.max(0) as usize, to.max(0) as usize)));
    b.on_move_tab_by(|step| with_viewer(|viewer, ui| viewer.move_active_tab(ui, step)));
    b.on_reopen_closed_tab(|| with_viewer(|viewer, ui| viewer.reopen_closed_tab(ui)));
    b.on_cycle_tab(|step| with_viewer(|viewer, ui| viewer.cycle_tab(ui, step)));
    b.on_tab_pin(|i| with_viewer(|viewer, ui| viewer.toggle_pin(ui, i.max(0) as usize)));
    b.on_tab_favorite(|i| {
        with_viewer(|viewer, ui| {
            if let Some(path) = usize::try_from(i).ok().and_then(|i| viewer.tab_path(i)) {
                viewer.toggle_favorite(ui, &path);
                let on = viewer.is_favorite(&path);
                viewer.notify(ui, if on { "Added to favorites" } else { "Removed from favorites" });
            }
        })
    });
    b.on_tab_copy_path(|i| {
        with_viewer(|viewer, ui| {
            if let Some(path) = viewer.tab_path(i as usize) {
                platform::copy_text(&path.display().to_string());
                viewer.notify(ui, "Copied the path");
            }
        })
    });
    b.on_tab_show_in_folder(|i| {
        with_viewer(|viewer, _| {
            if let Some(path) = viewer.tab_path(i as usize) {
                platform::show_in_folder(&path);
            }
        })
    });
    b.on_open_recent(|row| with_viewer(|viewer, ui| viewer.open_recent(ui, row as usize)));
    b.on_open_recent_background(|row| with_viewer(|viewer, ui| viewer.open_background(ui, viewer.recent_path(row as usize))));
    b.on_open_favorite_background(|row| with_viewer(|viewer, ui| viewer.open_background(ui, viewer.favorites.get(row as usize).cloned())));
    b.on_recent_favorite(|row| {
        with_viewer(|viewer, ui| {
            if let Some(path) = viewer.recent_path(row as usize) {
                viewer.toggle_favorite(ui, &path);
            }
        })
    });
    b.on_recent_remove(|row| with_viewer(|viewer, ui| viewer.remove_recent(ui, row as usize)));
    b.on_recent_copy_path(|row| {
        with_viewer(|viewer, ui| {
            if let Some(path) = viewer.recent_path(row as usize) {
                platform::copy_text(&path.display().to_string());
                viewer.notify(ui, "Copied the path");
            }
        })
    });
    b.on_recent_show_in_folder(|row| {
        with_viewer(|viewer, _| {
            if let Some(path) = viewer.recent_path(row as usize) {
                platform::show_in_folder(&path);
            }
        })
    });
    b.on_clear_recents(|| with_viewer(|viewer, ui| viewer.clear_recents(ui)));
    b.on_open_favorite(|row| with_viewer(|viewer, ui| viewer.open_favorite(ui, row as usize)));
    b.on_favorite_remove(|row| with_viewer(|viewer, ui| viewer.remove_favorite(ui, row as usize)));
    b.on_open_bookmark(|file, page| with_viewer(|viewer, ui| viewer.open_bookmark(ui, file as usize, page.max(1) as usize)));
    b.on_set_theme(|choice| with_viewer(|viewer, ui| viewer.set_theme(ui, choice)));
    b.on_toggle_restore(|| {
        with_viewer(|viewer, ui| {
            viewer.settings.restore_tabs = !viewer.settings.restore_tabs;
            store::save_settings(&viewer.settings);
            viewer.sync_home(ui);
        })
    });
    b.on_toggle_reload(|| {
        with_viewer(|viewer, ui| {
            viewer.settings.auto_reload = !viewer.settings.auto_reload;
            store::save_settings(&viewer.settings);
            viewer.sync_home(ui);
        })
    });
    b.on_set_default_zoom(|i| {
        with_viewer(|viewer, ui| {
            viewer.settings.default_zoom = match i {
                1 => Zoom::Fit,
                2 => Zoom::Width,
                3 => Zoom::Actual,
                _ => Zoom::Auto,
            };
            store::save_settings(&viewer.settings);
            viewer.sync_home(ui);
        })
    });
    b.on_open_url(|url| platform::shell_open(&url));
    b.on_open_error_log(|| match store::error_log_path() {
        Some(path) => platform::shell_open(&path.display().to_string()),
        None => with_viewer(|viewer, ui| viewer.notify(ui, "No errors have been logged.")),
    });

    // The document
    b.on_viewport_changed(|| with_viewer(|viewer, ui| viewer.refresh(ui)));
    b.on_thumbs_changed(|| with_viewer(|viewer, ui| viewer.refresh_thumbs(ui)));
    b.on_go_annotation(|page, fy| with_viewer(|viewer, ui| viewer.go_to(ui, page.max(0) as usize, (fy - 0.05).max(0.0))));
    b.on_delete_annotation(|page, index| {
        with_viewer(|viewer, _ui| {
            let Some(doc) = viewer.active_id() else { return };
            viewer.renderer.edit(render::EditJob::DeleteAnnotation { doc, page: page.max(0) as usize, index: index.max(0) as usize });
        })
    });
    b.on_go_page(|page| with_viewer(|viewer, ui| viewer.go_to(ui, page.max(0) as usize, 0.0)));
    b.on_page_step(|step| with_viewer(|viewer, ui| viewer.page_step(ui, step)));
    b.on_go_to_page(|text| {
        with_viewer(|viewer, ui| {
            let Ok(n) = text.trim().parse::<usize>() else { return };
            viewer.go_to(ui, n.max(1) - 1, 0.0);
        })
    });
    b.on_scroll_by(|dy| {
        with_viewer(|viewer, ui| {
            let (sx, sy) = viewer.scroll(ui);
            viewer.set_scroll(ui, sx, sy + dy);
            viewer.refresh(ui);
        })
    });
    b.on_scroll_wheel(|delta| {
        let mut handled = false;
        with_viewer(|viewer, ui| handled = viewer.scroll_wheel(ui, delta));
        handled
    });
    b.on_scroll_to_edge(|end| {
        with_viewer(|viewer, ui| {
            let groups = viewer.groups();
            if viewer.scroll_mode == view::SCROLL_PAGE && !groups.is_empty() {
                let target = if end { groups[groups.len() - 1].start } else { 0 };
                viewer.go_to(ui, target, 0.0);
                return;
            }
            let (sx, _) = viewer.scroll(ui);
            viewer.set_scroll(ui, sx, if end { f32::MAX } else { 0.0 });
            viewer.refresh(ui);
        })
    });
    b.on_zoom_step(|direction| {
        with_viewer(|viewer, ui| {
            let percent = viewer.scale / BASE_SCALE;
            let next = if direction > 0 {
                ZOOM_STEPS.iter().copied().find(|&z| z > percent + 0.005)
            } else {
                ZOOM_STEPS.iter().rev().copied().find(|&z| z < percent - 0.005)
            };
            if let Some(z) = next {
                viewer.set_zoom(ui, Zoom::Custom(z));
            }
        })
    });
    b.on_set_zoom(|mode, factor| with_viewer(|viewer, ui| viewer.set_zoom(ui, Zoom::from_index(mode, factor))));
    b.on_zoom_by(|delta, ax, ay| {
        with_viewer(|viewer, ui| {
            let factor = (delta / 60.0 * 1.1f32.ln()).exp();
            let scale = (viewer.scale * factor).clamp(MIN_ZOOM * BASE_SCALE, MAX_ZOOM * BASE_SCALE);
            viewer.zoom = Zoom::Custom(scale / BASE_SCALE);
            viewer.zoom_to(ui, scale, ax, ay);
        })
    });
    b.on_zoom_to_rect(|x, y, w, h| {
        with_viewer(|viewer, ui| viewer.zoom_to_rect(ui, x, y, w, h))
    });
    b.on_zoom_toggle(|ax, ay| {
        with_viewer(|viewer, ui| {
            if viewer.presenting.is_some() {
                return;
            }
            let width = viewer.scale_for(ui, Zoom::Width);
            if (viewer.scale - width).abs() < 0.01 {
                viewer.zoom = Zoom::Custom(width * 2.0 / BASE_SCALE);
                viewer.zoom_to(ui, width * 2.0, ax, ay);
            } else {
                viewer.zoom = Zoom::Width;
                viewer.zoom_to(ui, width, ax, ay);
            }
        })
    });
    b.on_pinch_started(|| {
        with_viewer(|viewer, _| {
            viewer.pinch_start = viewer.scale;
            viewer.pinching = true;
        })
    });
    b.on_pinch_updated(|factor, cx, cy| {
        with_viewer(|viewer, ui| {
            let scale = (viewer.pinch_start * factor).clamp(MIN_ZOOM * BASE_SCALE, MAX_ZOOM * BASE_SCALE);
            viewer.zoom = Zoom::Custom(scale / BASE_SCALE);
            viewer.zoom_to(ui, scale, cx, cy);
        })
    });
    b.on_pinch_ended(|| {
        with_viewer(|viewer, ui| {
            viewer.pinching = false;
            viewer.refresh(ui);
        })
    });
    b.on_rotate(|step| with_viewer(|viewer, ui| viewer.rotate(ui, step)));
    b.on_set_scroll_mode(|mode| {
        with_viewer(|viewer, ui| {
            let spread = viewer.spread_mode;
            viewer.set_layout_modes(ui, mode, spread);
            viewer.save_settings(ui);
        })
    });
    b.on_set_spread_mode(|mode| {
        with_viewer(|viewer, ui| {
            let scroll = viewer.scroll_mode;
            viewer.set_layout_modes(ui, scroll, mode);
            viewer.save_settings(ui);
        })
    });
    b.on_set_tone(|tone| {
        with_viewer(|viewer, ui| {
            viewer.tone = tone_of(tone);
            ui.global::<Bridge>().set_page_tone(tone);
            viewer.drop_bitmaps();
            viewer.refresh(ui);
            viewer.save_settings(ui);
        })
    });
    b.on_sidebar_toggled(|| {
        with_viewer(|viewer, ui| {
            viewer.refresh_thumbs(ui);
            let page = ui.global::<Bridge>().get_current_page().max(1) as usize - 1;
            viewer.reveal_thumb(ui, page);
            viewer.save_settings(ui);
        })
    });
    b.on_find_changed(|text| with_viewer(|viewer, ui| viewer.start_search(ui, text.to_string())));
    b.on_find_step(|step| with_viewer(|viewer, ui| viewer.find_step(ui, step)));
    b.on_find_options(|| {
        with_viewer(|viewer, ui| {
            let b = ui.global::<Bridge>();
            let opts = render::FindOpts {
                case: b.get_match_case(),
                word: b.get_whole_words(),
                diacritics: b.get_match_diacritics(),
            };
            viewer.highlight_all = b.get_highlight_all();
            if opts == viewer.find_opts {
                viewer.refresh(ui);
                return;
            }
            viewer.find_opts = opts;
            viewer.start_search(ui, b.get_find_text().to_string());
        })
    });
    b.on_find_closed(|| with_viewer(|viewer, ui| viewer.start_search(ui, String::new())));
    b.on_tool_changed(|| with_viewer(|viewer, ui| viewer.tool_changed(ui)));
    b.on_annotate_selection(|| with_viewer(|viewer, ui| viewer.annotate_selection(ui)));
    b.on_stroke_begin(|x, y| with_viewer(|viewer, ui| viewer.stroke_begin(ui, x, y)));
    b.on_stroke_point(|x, y| with_viewer(|viewer, ui| viewer.stroke_point(ui, x, y)));
    b.on_stroke_end(|| with_viewer(|viewer, ui| viewer.stroke_end(ui)));
    b.on_erase_at(|x, y| with_viewer(|viewer, _| viewer.erase_at(x, y)));
    b.on_text_at(|x, y| with_viewer(|viewer, ui| viewer.text_at(ui, x, y)));
    b.on_text_commit(|text| with_viewer(|viewer, ui| viewer.text_commit(ui, text.to_string())));
    b.on_text_cancel(|| with_viewer(|viewer, ui| viewer.text_cancel(ui)));
    b.on_read_aloud(|| with_viewer(|viewer, ui| viewer.read_aloud(ui)));
    b.on_translate(|| with_viewer(|viewer, ui| viewer.translate(ui)));
    b.on_confirm_choice(|choice| with_viewer(|viewer, ui| viewer.confirm_choice(ui, choice)));
    b.on_link_click(|x, y| with_viewer(|viewer, ui| viewer.click_at(ui, x, y)));
    b.on_annot_press(|x, y| {
        let mut mode = 0;
        with_viewer(|viewer, ui| mode = viewer.annot_press(ui, x, y));
        mode
    });
    b.on_annot_drag(|x, y| with_viewer(|viewer, ui| viewer.annot_drag(ui, x, y)));
    b.on_annot_release(|| with_viewer(|viewer, ui| viewer.annot_release(ui)));
    b.on_annot_delete(|| with_viewer(|viewer, ui| viewer.delete_selected(ui)));
    b.on_annot_deselect(|| with_viewer(|viewer, ui| viewer.deselect(ui)));
    b.on_undo(|| with_viewer(|viewer, ui| viewer.undo_redo(ui, true)));
    b.on_redo(|| with_viewer(|viewer, ui| viewer.undo_redo(ui, false)));
    b.on_toggle_auto_scroll(|| with_viewer(|viewer, ui| viewer.toggle_auto_scroll(ui)));
    b.on_note_edit_save(|text| with_viewer(|viewer, ui| viewer.note_edit_save(ui, text.to_string())));
    b.on_note_edit_delete(|| with_viewer(|viewer, ui| viewer.note_edit_delete(ui)));
    b.on_note_edit_cancel(|| with_viewer(|viewer, ui| viewer.note_edit_close(ui)));
    b.on_history_step(|step| with_viewer(|viewer, ui| viewer.history_step(ui, step)));
    b.on_submit_password(|password| with_viewer(|viewer, ui| viewer.submit_password(ui, password.to_string())));
    b.on_select_at(|phase, x, y| with_viewer(|viewer, ui| viewer.select_at(ui, phase, x, y)));
    b.on_select_page(|| with_viewer(|viewer, ui| viewer.select_page(ui)));
    b.on_clear_selection(|| with_viewer(|viewer, ui| viewer.clear_selection(ui)));
    b.on_copy_selection(|| {
        let mut copied = false;
        with_viewer(|viewer, _| copied = viewer.copy_selection());
        copied
    });
    b.on_toggle_bookmark(|| with_viewer(|viewer, ui| viewer.toggle_bookmark(ui)));
    b.on_thumb_menu_prepare(|page| with_viewer(|viewer, ui| viewer.thumb_menu_prepare(ui, page.max(0) as usize)));
    b.on_note_here(|| with_viewer(|viewer, ui| viewer.note_here(ui)));
    b.on_page_menu_prepare(|x, y| with_viewer(|viewer, ui| viewer.page_menu_prepare(ui, x, y)));
    b.on_toggle_bookmark_page(|page| with_viewer(|viewer, ui| viewer.toggle_bookmark_page(ui, page.max(0) as usize + 1)));
    b.on_clear_bookmarks(|| with_viewer(|viewer, ui| viewer.clear_bookmarks(ui)));
    b.on_show_properties(|| with_viewer(|viewer, _| viewer.show_properties()));
    b.on_toggle_presentation(|| with_viewer(|viewer, ui| viewer.toggle_presentation(ui)));
    b.on_print(|| with_viewer(|viewer, ui| viewer.print(ui)));
    b.on_save(|| with_viewer(|viewer, ui| viewer.save_original(ui)));
    b.on_save_copy(|| with_viewer(|viewer, ui| viewer.save_copy(ui)));

    // The window
    b.on_toggle_fullscreen(|| {
        with_viewer(|viewer, ui| {
            if viewer.presenting.is_some() {
                viewer.toggle_presentation(ui);
                return;
            }
            let full = !ui.window().is_fullscreen();
            ui.window().set_fullscreen(full);
            ui.global::<Bridge>().set_fullscreen(full);
        })
    });
    b.on_caption_metrics(|height, drag_from| titlebar::set_metrics(height, drag_from));
    b.on_minimize_window(|| with_viewer(|_, ui| ui.window().set_minimized(true)));
    b.on_toggle_maximize(|| {
        with_viewer(|_, ui| {
            let window = ui.window();
            window.set_maximized(!window.is_maximized());
        })
    });
    b.on_close_window(|| {
        with_viewer(|viewer, ui| viewer.save_all(ui));
        let _ = slint::quit_event_loop();
    });
    WINDOW.with(|w| *w.borrow_mut() = Some(ui.as_weak()));
    titlebar::on_settings_change(on_settings_change);
    platform::register_file_type();
    titlebar::on_navigate(|step| {
        let _ = slint::invoke_from_event_loop(move || with_viewer(|viewer, ui| viewer.history_step(ui, step)));
    });
    titlebar::on_open_files(|files| {
        let _ = slint::invoke_from_event_loop(move || {
            with_viewer(|viewer, ui| {
                for path in files {
                    viewer.open_path(ui, path);
                }
            })
        });
    });
    // The window becomes visible a moment after the event loop starts; retry until it is there.
    let weak = ui.as_weak();
    let attempts = std::cell::Cell::new(0);
    let installer = slint::Timer::default();
    installer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(30), move || {
        attempts.set(attempts.get() + 1);
        let Some(ui) = weak.upgrade() else { return };
        let b = ui.global::<Bridge>();
        titlebar::set_metrics(b.get_strip_height(), b.get_drag_from());
        if titlebar::installed() || attempts.get() > 100 {
            return;
        }
        titlebar::install(on_caption_state);
    });
    // Files changed on disk reload by themselves when that setting is on.
    let watcher = slint::Timer::default();
    watcher.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(1500), || {
        with_viewer(|viewer, ui| {
            viewer.reload_changed(ui);
            viewer.poll_speech(ui);
        });
    });
    ui.window().on_close_requested(|| {
        let mut may_close = true;
        with_viewer(|viewer, ui| {
            may_close = viewer.request_close_window(ui);
            if may_close {
                viewer.save_all(ui);
            }
        });
        if may_close { CloseRequestResponse::HideWindow } else { CloseRequestResponse::KeepWindowShown }
    });
    // Files dropped from Explorer open as tabs; the window dims the caption while it is not in front.
    ui.window().on_winit_window_event(|_, event| {
        match event {
            winit::event::WindowEvent::HoveredFile(_) => with_window(|ui| ui.global::<Bridge>().set_drop_hover(true)),
            winit::event::WindowEvent::HoveredFileCancelled => with_window(|ui| ui.global::<Bridge>().set_drop_hover(false)),
            winit::event::WindowEvent::DroppedFile(path) => {
                let path = path.clone();
                with_window(move |ui| {
                    ui.global::<Bridge>().set_drop_hover(false);
                    with_viewer(|viewer, ui| viewer.open_path(ui, path));
                });
            }
            winit::event::WindowEvent::Resized(_) => with_window(|ui| {
                let width = ui.window().size().width as f32 / ui.window().scale_factor();
                ui.global::<Bridge>().set_drawer(width < 760.0);
            }),
            winit::event::WindowEvent::Focused(on) => {
                let on = *on;
                with_window(move |ui| ui.global::<Bridge>().set_active_window(on));
            }
            _ => {}
        }
        EventResult::Propagate
    });

    let session = store::load_session();
    let place = session.window.map(platform::keep_on_screen).unwrap_or_else(platform::default_placement);
    ui.window().set_position(slint::PhysicalPosition::new(place.x, place.y));
    ui.window().set_size(slint::PhysicalSize::new(place.width, place.height));
    if place.maximized {
        ui.window().set_maximized(true);
    }
    let args: Vec<PathBuf> = std::env::args_os().skip(1).filter(|a| !a.is_empty()).map(PathBuf::from).collect();
    with_viewer(|viewer, ui| {
        viewer.last_normal = Some(store::Placement { maximized: false, ..place });
        // Last time's tabs come back only when asked for, and only when no file was given.
        if viewer.settings.restore_tabs && args.is_empty() {
            viewer.restore(ui, session);
        }
        viewer.sync_ui(ui);
        for path in args {
            viewer.open_path(ui, path);
        }
    });

    ui.run()
}
