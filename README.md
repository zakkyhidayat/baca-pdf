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
- **Annotation tools**: highlight selected text (five colors), draw freehand (five colors, three thicknesses), erase, and add text. Changes live in memory and show as a dot on the tab; Save a copy writes them into a new PDF, and closing a tab with changes asks first. The original file is never changed.
- **Read aloud** speaks the selected text or the current page with the Windows voice. **Translate** opens the selected text in a translation page in your browser, in the language Windows is set to.
- **Side panel** with thumbnails and the document outline. Drag its edge to resize it.
- **Page bookmarks** with a ribbon on the page and its thumbnail, a list on Home, and Ctrl+B.
- **Password-protected files** ask for the password.
- **Document properties** (Alt+Enter), **Print** (Ctrl+P, through the Windows print dialog), **Save a copy** (Ctrl+S, the original is never changed), **presentation mode** (F5) and **full screen** (F11).
- **Auto reload** when a file changes on disk, and **restore tabs** at start, both optional on Home.
- **Light and dark theme**, following Windows unless you choose one on Home.
- **Touch**: larger targets, pinch zoom, and drag to scroll. The layout switches on by itself on a touch screen.
- **One window**: opening several PDFs from Explorer, or launching the program again, sends the files to the window that is already open as tabs.
- Keyboard shortcuts are listed in the app (F1).

## Not built yet

- Form filling, signatures, stamps and comments. Added text uses a built-in Latin font.
- Registering itself as the default `.pdf` program. Windows must be told once, in Settings, to open PDFs with Baca PDF.

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
