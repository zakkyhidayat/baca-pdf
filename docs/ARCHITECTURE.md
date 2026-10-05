# How Baca PDF is put together

This page is for anyone who forks Baca PDF and wants to change something. It says which file to open for which part of the app, and how the parts talk to each other.

## The two halves

The interface is written in [Slint](https://slint.dev) and lives in `ui/`. Everything else (opening and rendering PDFs, saving, settings, talking to Windows) is Rust and lives in `src/`.

The two halves meet in one file, `ui/bridge.slint`. It holds every value the interface shows (the open tabs, the page number, the zoom, the search results) and every callback the interface sends (open a file, go to a page, save). Rust fills in the values and handles the callbacks. The callbacks are connected to Rust in `src/main.rs`.

To add a feature that needs both halves:

1. Add a property or callback to `Bridge` in `ui/bridge.slint`.
2. Use it from the `.slint` file that draws the feature.
3. Connect it in `src/main.rs`, and put the work in the module it belongs to (see below).

## Where to change what

| To change | Open |
|---|---|
| Colors, light and dark theme, sizes of the bars | `ui/theme.slint` |
| Buttons, text boxes, menu rows, the menu surface | `ui/controls.slint` |
| The tabs, the Home button and the window buttons | `ui/titlebar.slint`, and `src/titlebar.rs` for how the strip sits in the Windows title bar |
| The toolbar, its menus, the find bar, the highlight and draw options | `ui/toolbar.slint` |
| The pages, the side panel, the right-click menu on a page, the copy bar | `ui/viewer.slint` |
| Home (favorites, bookmarks, recent files, settings) | `ui/home.slint`, and `src/tabs.rs` for the lists behind it |
| Dialogs (properties, shortcuts, About, Sign, save changes) | `ui/dialogs.slint` |
| Notices, tooltips, the drop-files overlay, how the window is laid out | `ui/app.slint` |
| Zoom, scrolling, which pages get rendered | `src/view.rs` |
| Rendering pages and saving files (PDFium) | `src/render.rs` |
| Highlight, draw, erase, add text, notes | `src/annotate.rs`, and `src/undo.rs` for undo and redo |
| Find in the document, selecting and copying text | `src/find.rs` |
| Filling in forms | `src/forms.rs` |
| Signatures | `src/sign.rs` |
| Thumbnails and the outline | `src/sidebar.rs` |
| Print, properties, save a copy, page bookmarks, theme | `src/actions.rs` |
| Settings and lists kept between runs | `src/store.rs` |
| File pickers, print dialog, clipboard, other Windows calls | `src/platform.rs` |
| One window for every PDF opened from Explorer | `src/single.rs` |
| The program icon and the Windows manifest in the exe | `build.rs` |
| The portable zip and its `Install.cmd` | `portable/`, packed by `portable/make-zip.ps1` |

## Threads

PDFium is not thread-safe, so it lives on one worker thread of its own (`src/render.rs`). The interface thread never calls PDFium directly. It posts a request (render this page, find this text, save this file) and gets the result back as a message. Long work never blocks the window this way.

## Conventions

- Interface text is in US English and in sentence case ("Save a copy", not "Save A Copy").
- Icons come from Segoe Fluent Icons, which ships with Windows 11. Most code points have the icon's name in a comment above them.
- Menus are 200px wide, or 260px when they show keyboard shortcuts. View options is wider, for its picture tiles. Menu rows are 32px tall (40px in the touch layout).
- Controls have 4px corners, menus and dialogs 8px. Icon buttons are round.
- Every dialog ends with a row of buttons. The main action is a `PrimaryButton`, the others are `SecondaryButton`.
- Text boxes are drawn with `FieldFrame`.
