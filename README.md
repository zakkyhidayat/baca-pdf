# Baca PDF

A PDF reader for Windows 11 that renders with PDFium and draws its own interface with Slint. On a 138-page illustrated report it uses about 40 MB after opening, stays under 90 MB while scrolling from the first page to the last, and stays under 100 MB at 800% zoom.

This is the native rewrite of the Tauri and pdf.js version. Reading, searching, bookmarks, printing and basic annotation are built. Form filling and signatures are not.

This project is 100% vibe coded with Claude Code, made for personal use. I am happy to share it, and you are welcome to fork it.

## What works now

- **Tabs in the title bar**, next to Windows 11 style minimize, maximize and close buttons. Hovering maximize opens Snap Layouts. Each tab keeps its own zoom, page and rotation. Background tabs let go of their page images.
- **Home** lists favorites, pages you bookmarked, and recent files with the page where you stopped. A right-click menu offers favorites, copy path, show in folder and remove.
- **Zoom** from 25% to 800%: Automatic, Actual Size, Page Fit, Page Width, or a percentage, with Ctrl+plus, Ctrl+minus, Ctrl+wheel or a two-finger pinch.
- **View options**: scrolling (vertical, horizontal, wrapped, one page at a time), pages (single, two pages, book view), page color (normal, dark, sepia) and a touch layout.
- **Find** (Ctrl+F): a bar under the toolbar with the match count, previous, next and close. Every match is highlighted, and accented letters match their plain forms.
- **Select and copy** text across pages, with Ctrl+C or the Copy bar. Ctrl+A selects the current page.
- **Links** inside the PDF: web and mail addresses open in your browser, page links jump there. Alt+Left and Alt+Right go back and forward.
- **Annotation tools**: highlight, underline or strike through selected text (five colors), draw freehand or as a rectangle, ellipse, line or arrow (five colors, three thicknesses) and erase. Right-click a page and choose Add a note here for a sticky note; click its icon later to read, change or delete it. The Annotations tab in the side panel lists everything added; click one to jump to it, or remove it from there. Changes live in memory and show as a dot on the tab; Save (Ctrl+S) writes them into the file itself and keeps the earlier version once as "name (backup).pdf"; Save a copy (Ctrl+Shift+S) writes a new PDF instead. Closing a tab with changes asks first.
- **Read aloud** speaks the selected text or the current page with the Windows voice. **Translate** opens the selected text in a translation page in your browser, in the language Windows is set to.
- **Pinned tabs**: right-click a tab and choose Pin tab. Pinned tabs sit first as icons only, cannot be closed by accident with the middle button, and come back pinned the next time you start.
- **Auto-scroll**: turn it on in View options. The page moves down by itself; a small bar at the bottom changes the speed (eight steps) or stops it, and Esc stops it too. It stops by itself at the end of the document.
- **Zoom to area**: pick it in View options (or hold Ctrl and drag), then drag a rectangle on the page; the view zooms to fill the window with it.
- **Side panel** with thumbnails and the document outline. Drag its edge to resize it.
- **Page bookmarks** with a ribbon on the page and its thumbnail, a list on Home, and Ctrl+B.
- **Password-protected files** ask for the password.
- **Document properties** (Alt+Enter), **Print** (Ctrl+P, through the Windows print dialog), **Save a copy** (Ctrl+S, the original is never changed), **presentation mode** (F5) and **full screen** (F11).
- **Auto reload** when a file changes on disk, and **restore tabs** at start, both optional on Home.
- **Light and dark theme**, following Windows unless you choose one on Home.
- **Touch**: larger targets, pinch zoom, and drag to scroll. The layout switches on by itself on a touch screen.
- **One window**: opening several PDFs from Explorer, or launching the program again, sends the files to the window that is already open as tabs.
- Keyboard shortcuts are listed in the app (F1).

## To do

Ideas, not promises. The first group is what is known to be missing.

**Missing now**
- **Add text** is built but hidden from the toolbar until its options are settled (see "Add text, next steps"). Today it places one line of 12 pt Helvetica in the draw color.
- **One options box for the drawing tools.** Highlight and Draw each have their own small popup (colors for one, colors and thickness for the other). They should share a single box with a section per tool, so all the options sit in one place.
- Form filling, stamps and comments.
- **Add image**: place a picture on a page, such as a premade handwritten signature saved as a PNG. Pick the file, click where it goes, then move and resize it. Transparent PNGs should keep their transparency.
- **Sign**: type a name and get it as a cursive signature, with a choice of a few script fonts and ink colors, placed and resized like an image. Drawing a signature and keeping it for later would sit beside it.
- **Better translation.** Translate now opens the browser. Instead, a small floating window inside the app that shows the result, and a choice of the source and target language (today the target is the Windows language and the source is detected). It may need a translation service behind it, which sends the text out, or a local model, which stays on the computer but is large; which one is still open.
- Registering itself as the default `.pdf` program. Windows must be told once, in Settings, to open PDFs with Baca PDF.
- Add text today is one line: click where it goes, type, press Enter. It is 12 pt Helvetica in the draw color, cannot be edited or moved after it is placed (only erased), and does not work on a rotated page. Letters outside the Latin set do not show.

**Add text, next steps**
- Several lines, and a box that wraps.
- A color picker like the one for Draw, then font size, bold and italic.
- Edit, move and resize the text after placing it.
- A font that covers other scripts, and the choice to embed it.

**Annotation**
- Undo and redo (Ctrl+Z) for highlights, drawings and text. Open question: where the buttons go in the toolbar.
- Move and resize what was added (highlights, drawings, shapes, notes).
- Pen pressure and palm rejection on tablets.

**Reading**
- Split view for two documents side by side (built, kept on a side branch for now).
- Search across every open tab, and across recent files. The results should be saveable to a local file, so a user can keep them as a backup.
- Names and notes for page bookmarks.
- Kinetic scrolling on touch (the page already flicks with Slint's own inertia; it has not been tuned or tested on a touch screen).
- A laser pointer and a blank screen in presentation mode.
- Read aloud that follows the text and lets you pick the voice and speed.
- OCR for scanned pages with the text recognition built into Windows, so they become searchable.

**Home and Windows**
- Search, sort and group the recent files; a first-page thumbnail on each.
- Jump list entries for recent files on the taskbar icon.
- A one-click shortcut to the Windows default apps settings.
- An Indonesian interface and a Windows high contrast theme.
- A check for new releases.

**Pages**
- Reorder, delete, rotate, extract and merge pages, and export pages as images.
- A print preview with a page range.

## Where it keeps its data

`%LOCALAPPDATA%\io.github.zakkyhidayat.bacapdf\` holds small text files: settings, recent files, favorites, bookmarks, the open tabs and an error log. Deleting the folder resets all of it.

To run it as a portable program, create a folder named `data` next to `baca-pdf.exe`. The files then live there instead.

## How it stays small

Only the pages around the viewport are kept as bitmaps, at most 4 megapixels each. Past that size, PDFium renders just the visible part of the page at full sharpness. Rendering runs on its own thread, and drawing happens on the CPU, so each bitmap exists once instead of once in memory and again as a GPU texture.

## Building

Requires Rust (stable, MSVC toolchain) on Windows 10 or 11.

```
cargo build --release
```

Copy `pdfium\bin\pdfium.dll` next to `target\release\baca-pdf.exe` before running it.

`pdfium.dll` is the prebuilt chromium/8076 build from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries). Its licenses are in `pdfium\LICENSE` and `pdfium\licenses\`.

## License

GPL-3.0-or-later. See `LICENSE`.
