// SPDX-License-Identifier: GPL-3.0-or-later

//! PDFium lives on one worker thread; the UI thread only posts requests and receives results.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use pdfium_render::prelude::*;
use slint::{Rgba8Pixel, SharedPixelBuffer};

/// Page colors applied to rendered pixels.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tone {
    #[default]
    Normal,
    Dark,
    Sepia,
}

/// A page rendered at `width` x `height` pixels (already rotated). With `tile` set,
/// only that pixel rectangle (x, y, width, height) of the page is produced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Request {
    pub doc: u64,
    pub page: usize,
    pub width: u32,
    pub height: u32,
    pub tile: Option<[u32; 4]>,
    /// Clockwise quarter turns added by the user.
    pub turns: u8,
    pub tone: Tone,
    pub thumb: bool,
}

/// A rectangle as fractions of the displayed page: x, y, width, height.
pub type Frac = [f32; 4];

#[derive(Clone, Debug)]
pub struct OutlineItem {
    pub depth: usize,
    pub title: String,
    pub page: Option<usize>,
}

pub enum Event {
    Opened { doc: u64, sizes: Vec<(f32, f32)>, outline: Vec<OutlineItem> },
    Failed { doc: u64, message: String },
    /// The file wants a password; `wrong` is set when one was given and did not work.
    NeedPassword { doc: u64, wrong: bool },
    Rendered { req: Request, pixels: SharedPixelBuffer<Rgba8Pixel> },
    SearchHits { doc: u64, generation: u64, page: usize, hits: Vec<Vec<Frac>> },
    SearchDone { doc: u64, generation: u64 },
    Selected { doc: u64, generation: u64, pages: Vec<(usize, Vec<Frac>)>, text: String },
    Link { doc: u64, target: LinkTarget },
    /// An edit changed these pages (or failed with a message).
    Annotated { doc: u64, pages: Vec<usize>, message: Option<String> },
    PageText { doc: u64, text: String },
    Saved { doc: u64, path: PathBuf, token: u64, error: Option<String>, backup: Option<PathBuf> },
    Properties { doc: u64, rows: Vec<(String, String)> },
    Printed { pages: usize, message: Option<String> },
}

/// A printer device context from the print dialog, with the pages the user chose.
pub struct PrintJob {
    pub doc: u64,
    pub hdc: isize,
    pub pages: Vec<usize>,
    pub title: String,
}

/// What the find bar asks for besides the words.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct FindOpts {
    pub case: bool,
    pub word: bool,
    pub diacritics: bool,
}

struct SearchJob {
    doc: u64,
    generation: u64,
    query: String,
    opts: FindOpts,
    turns: u8,
    next_page: usize,
}

/// Where a link inside a page leads.
pub enum LinkTarget {
    Uri(String),
    Page(usize),
}

/// Changes to the open document, made on the PDFium thread.
pub enum EditJob {
    Highlight { doc: u64, pieces: Vec<SelectPiece>, turns: u8, color: [u8; 3], style: u8 },
    Ink { doc: u64, page: usize, turns: u8, points: Vec<[f32; 2]>, color: [u8; 3], width: f32 },
    Erase { doc: u64, page: usize, turns: u8, point: [f32; 2] },
    Text { doc: u64, page: usize, turns: u8, point: [f32; 2], text: String, size: f32, color: [u8; 3] },
    PageText { doc: u64, page: usize },
    Save { doc: u64, path: PathBuf, token: u64 },
    /// Writes the changes into the open file itself, keeping the version before the first save beside it.
    SaveOriginal { doc: u64, path: PathBuf, token: u64 },
}

impl EditJob {
    fn doc(&self) -> u64 {
        match self {
            EditJob::Highlight { doc, .. }
            | EditJob::Ink { doc, .. }
            | EditJob::Erase { doc, .. }
            | EditJob::Text { doc, .. }
            | EditJob::PageText { doc, .. }
            | EditJob::Save { doc, .. }
            | EditJob::SaveOriginal { doc, .. } => *doc,
        }
    }
}

pub struct LinkJob {
    pub doc: u64,
    pub page: usize,
    pub turns: u8,
    /// The click as fractions of the displayed page.
    pub point: [f32; 2],
}

/// A stretch of one page to select. A missing end means the start or the end of the page.
#[derive(Clone)]
pub struct SelectPiece {
    pub page: usize,
    pub from: Option<[f32; 2]>,
    pub to: Option<[f32; 2]>,
}

pub struct SelectJob {
    pub doc: u64,
    pub generation: u64,
    pub turns: u8,
    /// In reading order; a selection that crosses pages has a piece per page.
    pub pieces: Vec<SelectPiece>,
}

#[derive(Default)]
struct Inbox {
    open: VecDeque<(u64, PathBuf, Option<String>)>,
    close: Vec<u64>,
    wanted: Vec<Request>,
    thumbs: Vec<Request>,
    search: Option<SearchJob>,
    select: Option<SelectJob>,
    edits: VecDeque<EditJob>,
    link: Option<LinkJob>,
    properties: Option<u64>,
    print: Option<PrintJob>,
}

impl Inbox {
    fn idle(&self) -> bool {
        self.open.is_empty()
            && self.close.is_empty()
            && self.wanted.is_empty()
            && self.thumbs.is_empty()
            && self.search.is_none()
            && self.select.is_none()
            && self.edits.is_empty()
            && self.link.is_none()
            && self.properties.is_none()
            && self.print.is_none()
    }
}

#[derive(Clone)]
pub struct Renderer {
    inbox: Arc<(Mutex<Inbox>, Condvar)>,
}

impl Renderer {
    pub fn start(deliver: impl Fn(Event) + Send + 'static) -> Self {
        let inbox = Arc::new((Mutex::new(Inbox::default()), Condvar::new()));
        let worker_inbox = inbox.clone();
        std::thread::Builder::new()
            .name("pdfium".into())
            .spawn(move || worker(worker_inbox, deliver))
            .expect("spawn render thread");
        Self { inbox }
    }

    fn update(&self, f: impl FnOnce(&mut Inbox)) {
        let (lock, wake) = &*self.inbox;
        f(&mut lock.lock().unwrap());
        wake.notify_one();
    }

    pub fn open(&self, doc: u64, path: PathBuf, password: Option<String>) {
        self.update(|inbox| inbox.open.push_back((doc, path, password)));
    }

    pub fn close(&self, doc: u64) {
        self.update(|inbox| {
            inbox.open.retain(|(d, _, _)| *d != doc);
            inbox.wanted.retain(|r| r.doc != doc);
            inbox.thumbs.retain(|r| r.doc != doc);
            if inbox.search.as_ref().is_some_and(|s| s.doc == doc) {
                inbox.search = None;
            }
            inbox.close.push(doc);
        });
    }

    /// Replaces the page queue; the first entry is rendered first.
    pub fn want(&self, wanted: Vec<Request>) {
        let (lock, wake) = &*self.inbox;
        let mut inbox = lock.lock().unwrap();
        if inbox.wanted != wanted {
            inbox.wanted = wanted;
            wake.notify_one();
        }
    }

    /// Replaces the thumbnail queue, which runs only when no page is waiting.
    pub fn want_thumbs(&self, thumbs: Vec<Request>) {
        let (lock, wake) = &*self.inbox;
        let mut inbox = lock.lock().unwrap();
        if inbox.thumbs != thumbs {
            inbox.thumbs = thumbs;
            wake.notify_one();
        }
    }

    /// Starts searching every page, replacing any search still running. An empty query stops it.
    pub fn search(&self, doc: u64, generation: u64, query: String, opts: FindOpts, turns: u8) {
        self.update(|inbox| {
            inbox.search = (!query.is_empty())
                .then_some(SearchJob { doc, generation, query, opts, turns, next_page: 0 });
        });
    }

    /// Only the newest selection request matters while the pointer moves.
    pub fn select(&self, job: SelectJob) {
        self.update(|inbox| inbox.select = Some(job));
    }

    pub fn edit(&self, job: EditJob) {
        self.update(|inbox| inbox.edits.push_back(job));
    }

    pub fn link(&self, job: LinkJob) {
        self.update(|inbox| inbox.link = Some(job));
    }

    pub fn properties(&self, doc: u64) {
        self.update(|inbox| inbox.properties = Some(doc));
    }

    pub fn print(&self, job: PrintJob) {
        self.update(|inbox| inbox.print = Some(job));
    }
}

fn bind() -> Result<Pdfium, String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default();
    Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&exe_dir))
        .map(Pdfium::new)
        .map_err(|e| format!("pdfium.dll could not be loaded ({e})."))
}

enum Job {
    Close(u64),
    Open(u64, PathBuf, Option<String>),
    Select(SelectJob),
    Edit(EditJob),
    Link(LinkJob),
    Properties(u64),
    Print(PrintJob),
    Render(Request),
    Search { doc: u64, generation: u64, query: String, opts: FindOpts, turns: u8, page: usize },
}

fn next_job(inbox: &mut Inbox) -> Job {
    if let Some(doc) = inbox.close.pop() {
        return Job::Close(doc);
    }
    if let Some((doc, path, password)) = inbox.open.pop_front() {
        return Job::Open(doc, path, password);
    }
    if let Some(job) = inbox.edits.pop_front() {
        return Job::Edit(job);
    }
    if let Some(job) = inbox.select.take() {
        return Job::Select(job);
    }
    if let Some(job) = inbox.link.take() {
        return Job::Link(job);
    }
    if let Some(doc) = inbox.properties.take() {
        return Job::Properties(doc);
    }
    if let Some(job) = inbox.print.take() {
        return Job::Print(job);
    }
    if !inbox.wanted.is_empty() {
        return Job::Render(inbox.wanted.remove(0));
    }
    if !inbox.thumbs.is_empty() {
        return Job::Render(inbox.thumbs.remove(0));
    }
    let s = inbox.search.as_mut().expect("idle() said there was work");
    let job = Job::Search {
        doc: s.doc,
        generation: s.generation,
        query: s.query.clone(),
        opts: s.opts,
        turns: s.turns,
        page: s.next_page,
    };
    s.next_page += 1;
    job
}

fn worker(inbox: Arc<(Mutex<Inbox>, Condvar)>, deliver: impl Fn(Event)) {
    let pdfium = bind();
    let mut documents: HashMap<u64, PdfDocument> = HashMap::new();
    // Text added with the text tool is page content, so remember where it went to be able to erase it.
    let mut passwords: HashMap<u64, Option<String>> = HashMap::new();
    let mut added_text: HashMap<u64, Vec<(usize, [f32; 4])>> = HashMap::new();
    let (lock, wake) = &*inbox;

    loop {
        let job = {
            let mut guard = lock.lock().unwrap();
            while guard.idle() {
                guard = wake.wait(guard).unwrap();
            }
            next_job(&mut guard)
        };

        match job {
            Job::Close(doc) => {
                documents.remove(&doc);
                added_text.remove(&doc);
                passwords.remove(&doc);
            }
            Job::Open(doc, path, password) => {
                let pdfium = match &pdfium {
                    Ok(p) => p,
                    Err(message) => {
                        deliver(Event::Failed { doc, message: message.clone() });
                        continue;
                    }
                };
                match pdfium.load_pdf_from_file(&path, password.as_deref()) {
                    Ok(document) => {
                        let sizes = document
                            .pages()
                            .iter()
                            .map(|p| (p.width().value, p.height().value))
                            .collect();
                        let outline = outline(&document);
                        documents.insert(doc, document);
                        passwords.insert(doc, password.clone());
                        deliver(Event::Opened { doc, sizes, outline });
                    }
                    Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
                        deliver(Event::NeedPassword { doc, wrong: password.is_some() })
                    }
                    Err(e) => deliver(Event::Failed { doc, message: describe(&e) }),
                }
            }
            Job::Render(req) => {
                let Some(document) = documents.get(&req.doc) else { continue };
                if let Some(pixels) = render(document, &req) {
                    deliver(Event::Rendered { req, pixels });
                }
            }
            Job::Select(job) => {
                let Some(document) = documents.get(&job.doc) else { continue };
                let mut pages = Vec::new();
                let mut texts = Vec::new();
                for piece in &job.pieces {
                    if let Some((rects, text)) = select_piece(document, piece, job.turns) {
                        pages.push((piece.page, rects));
                        if !text.is_empty() {
                            texts.push(text);
                        }
                    }
                }
                deliver(Event::Selected { doc: job.doc, generation: job.generation, pages, text: texts.join("\r\n") });
            }
            Job::Edit(job) => {
                let doc = job.doc();
                if let EditJob::SaveOriginal { path, token, .. } = &job {
                    let outcome = match &pdfium {
                        Ok(p) => save_original(p, &mut documents, &passwords, doc, path),
                        Err(message) => Err(message.clone()),
                    };
                    added_text.remove(&doc);
                    let (backup, error) = match outcome {
                        Ok(backup) => (backup, None),
                        Err(message) => (None, Some(message)),
                    };
                    deliver(Event::Saved { doc, path: path.clone(), token: *token, error, backup });
                    continue;
                }
                let Some(document) = documents.get_mut(&doc) else { continue };
                let ledger = added_text.entry(doc).or_default();
                match job {
                    EditJob::Highlight { pieces, turns, color, style, .. } => {
                        let (pages, message) = split(edit_highlight(document, &pieces, turns, color, style));
                        deliver(Event::Annotated { doc, pages, message });
                    }
                    EditJob::Ink { page, turns, points, color, width, .. } => {
                        let (pages, message) = split(edit_ink(document, page, turns, &points, color, width).map(|p| vec![p]));
                        deliver(Event::Annotated { doc, pages, message });
                    }
                    EditJob::Erase { page, turns, point, .. } => {
                        let (pages, message) = split(edit_erase(document, ledger, page, turns, point).map(|hit| if hit { vec![page] } else { vec![] }));
                        deliver(Event::Annotated { doc, pages, message });
                    }
                    EditJob::Text { page, turns, point, text, size, color, .. } => {
                        let (pages, message) =
                            split(edit_text(document, ledger, page, turns, point, &text, size, color).map(|_| vec![page]));
                        deliver(Event::Annotated { doc, pages, message });
                    }
                    EditJob::PageText { page, .. } => {
                        let text = document.pages().get(page as _).ok().and_then(|p| p.text().ok().map(|t| t.all())).unwrap_or_default();
                        deliver(Event::PageText { doc, text });
                    }
                    EditJob::Save { path, token, .. } => {
                        let error = document.save_to_file(&path).err().map(|e| describe(&e));
                        deliver(Event::Saved { doc, path, token, error, backup: None });
                    }
                    EditJob::SaveOriginal { .. } => {}
                }
            }
            Job::Link(job) => {
                let Some(document) = documents.get(&job.doc) else { continue };
                if let Some(target) = link_at(document, &job) {
                    deliver(Event::Link { doc: job.doc, target });
                }
            }
            Job::Properties(doc) => {
                let Some(document) = documents.get(&doc) else { continue };
                deliver(Event::Properties { doc, rows: properties(document) });
            }
            Job::Print(job) => {
                let result = match documents.get(&job.doc) {
                    Some(document) => print(document, &job),
                    None => Err("The document is no longer open.".into()),
                };
                match result {
                    Ok(pages) => deliver(Event::Printed { pages, message: None }),
                    Err(message) => deliver(Event::Printed { pages: 0, message: Some(message) }),
                }
            }
            Job::Search { doc, generation, query, opts, turns, page } => {
                let Some(document) = documents.get(&doc) else {
                    lock.lock().unwrap().search = None;
                    continue;
                };
                if page >= document.pages().len() as usize {
                    let mut guard = lock.lock().unwrap();
                    if guard.search.as_ref().is_some_and(|s| s.generation == generation) {
                        guard.search = None;
                    }
                    drop(guard);
                    deliver(Event::SearchDone { doc, generation });
                    continue;
                }
                let hits = search_page(document, page, &query, opts, turns);
                if !hits.is_empty() {
                    deliver(Event::SearchHits { doc, generation, page, hits });
                }
            }
        }
    }
}

fn rotation(turns: u8) -> PdfPageRenderRotation {
    match turns % 4 {
        1 => PdfPageRenderRotation::Degrees90,
        2 => PdfPageRenderRotation::Degrees180,
        3 => PdfPageRenderRotation::Degrees270,
        _ => PdfPageRenderRotation::None,
    }
}

/// PDFium draws straight into the buffer Slint uploads, so each page costs one allocation.
fn render(document: &PdfDocument, req: &Request) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let page = document.pages().get(req.page as _).ok()?;
    let [x, y, w, h] = req.tile.unwrap_or([0, 0, req.width, req.height]);
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    {
        let mut bitmap =
            PdfBitmap::from_bytes(w as _, h as _, PdfBitmapFormat::BGRA, pixels.make_mut_bytes()).ok()?;
        let config = PdfRenderConfig::new()
            .set_fixed_size(req.width as _, req.height as _)
            .set_origin(-(x as i32), -(y as i32))
            .rotate(rotation(req.turns), false)
            .set_format(PdfBitmapFormat::BGRA)
            .set_reverse_byte_order(true)
            .render_form_data(true);
        page.render_into_bitmap_with_config(&mut bitmap, &config).ok()?;
    }
    apply_tone(pixels.make_mut_bytes(), req.tone);
    Some(pixels)
}

/// Dark keeps hues but swaps light and dark (white paper becomes dark grey, black text light grey);
/// sepia tints the page like warm paper.
fn apply_tone(bytes: &mut [u8], tone: Tone) {
    match tone {
        Tone::Normal => {}
        Tone::Dark => {
            for px in bytes.chunks_exact_mut(4) {
                let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
                let shift = 255 - r.max(g).max(b) - r.min(g).min(b);
                for (i, c) in [r, g, b].into_iter().enumerate() {
                    let v = (c + shift).clamp(0, 255);
                    px[i] = (32 + v * (226 - 32) / 255) as u8;
                }
            }
        }
        Tone::Sepia => {
            for px in bytes.chunks_exact_mut(4) {
                px[0] = (px[0] as u32 * 244 / 255) as u8;
                px[1] = (px[1] as u32 * 234 / 255) as u8;
                px[2] = (px[2] as u32 * 212 / 255) as u8;
            }
        }
    }
}

/// A large virtual device so that whole-pixel coordinates still give precise fractions.
const VIRTUAL: f32 = 100_000.0;

fn virtual_config(page: &PdfPage, turns: u8) -> (PdfRenderConfig, f32, f32) {
    let (w, h) = (page.width().value, page.height().value);
    let (dw, dh) = if turns % 2 == 1 { (h, w) } else { (w, h) };
    let (vw, vh) = (VIRTUAL, (VIRTUAL * dh / dw).round());
    let config = PdfRenderConfig::new().set_fixed_size(vw as _, vh as _).rotate(rotation(turns), false);
    (config, vw, vh)
}

/// Converts a rectangle in page points to fractions of the displayed (rotated) page.
fn to_frac(page: &PdfPage, config: &PdfRenderConfig, vw: f32, vh: f32, r: &PdfRect) -> Option<Frac> {
    let a = page.points_to_pixels(r.left(), r.top(), config).ok()?;
    let b = page.points_to_pixels(r.right(), r.bottom(), config).ok()?;
    let (x0, x1) = (a.0.min(b.0) as f32, a.0.max(b.0) as f32);
    let (y0, y1) = (a.1.min(b.1) as f32, a.1.max(b.1) as f32);
    Some([x0 / vw, y0 / vh, (x1 - x0) / vw, (y1 - y0) / vh])
}

/// Plain letter for an accented one, so "resume" finds "résumé" the way pdf.js does.
fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' | 'Ą' => 'A',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'Ç' | 'Ć' | 'Ĉ' | 'Ċ' | 'Č' => 'C',
        'ď' | 'đ' => 'd',
        'Ď' | 'Đ' => 'D',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => 'E',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => 'G',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ĩ' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => 'I',
        'ĺ' | 'ļ' | 'ľ' | 'ł' => 'l',
        'Ĺ' | 'Ļ' | 'Ľ' | 'Ł' => 'L',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'Ñ' | 'Ń' | 'Ņ' | 'Ň' => 'N',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' | 'Ŏ' | 'Ő' => 'O',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'Ŕ' | 'Ŗ' | 'Ř' => 'R',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'Ś' | 'Ŝ' | 'Ş' | 'Š' => 'S',
        'ţ' | 'ť' | 'ŧ' => 't',
        'Ţ' | 'Ť' | 'Ŧ' => 'T',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ũ' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => 'U',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'Ý' | 'Ÿ' | 'Ŷ' => 'Y',
        'ź' | 'ż' | 'ž' => 'z',
        'Ź' | 'Ż' | 'Ž' => 'Z',
        other => other,
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Finds matches by comparing letters with their accents removed. PDFium cannot do this, so it
/// only runs on pages that have accented letters (or when the query has some).
fn search_folded(page: &PdfPage, text: &PdfPageText, query: &str, opts: FindOpts, turns: u8) -> Vec<Vec<Frac>> {
    let key = |c: char| {
        let c = fold(c);
        if opts.case { c } else { c.to_lowercase().next().unwrap_or(c) }
    };
    let needle: Vec<char> = query.chars().map(key).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let chars = text.chars();
    let count = text.len() as usize;
    let mut hay: Vec<char> = Vec::with_capacity(count);
    let mut bounds: Vec<Option<(f32, f32, f32, f32)>> = Vec::with_capacity(count);
    for i in 0..count {
        let c = chars.get(i).ok();
        hay.push(c.as_ref().and_then(|c| c.unicode_char()).unwrap_or(' '));
        bounds.push(c.and_then(|c| c.loose_bounds().ok()).map(|r| (r.left().value, r.bottom().value, r.right().value, r.top().value)));
    }
    let folded: Vec<char> = hay.iter().map(|&c| key(c)).collect();
    let (config, vw, vh) = virtual_config(page, turns);
    let mut hits = Vec::new();
    let mut at = 0;
    while at + needle.len() <= folded.len() {
        if folded[at..at + needle.len()] != needle[..] {
            at += 1;
            continue;
        }
        let end = at + needle.len();
        let edge_ok = !opts.word
            || ((at == 0 || !is_word(hay[at - 1])) && (end >= hay.len() || !is_word(hay[end])));
        if edge_ok {
            // One rectangle per line of the match.
            let mut lines: Vec<(f32, f32, f32, f32)> = Vec::new();
            for r in bounds[at..end].iter().flatten().filter(|r| r.2 > r.0) {
                let mid = (r.1 + r.3) / 2.0;
                match lines.last_mut().filter(|l| mid > l.1 && mid < l.3) {
                    Some(l) => {
                        l.0 = l.0.min(r.0);
                        l.1 = l.1.min(r.1);
                        l.2 = l.2.max(r.2);
                        l.3 = l.3.max(r.3);
                    }
                    None => lines.push(*r),
                }
            }
            let rects: Vec<Frac> = lines
                .iter()
                .filter_map(|l| {
                    let r = PdfRect::new(PdfPoints::new(l.1), PdfPoints::new(l.0), PdfPoints::new(l.3), PdfPoints::new(l.2));
                    to_frac(page, &config, vw, vh, &r)
                })
                .collect();
            if !rects.is_empty() {
                hits.push(rects);
            }
            at = end;
        } else {
            at += 1;
        }
    }
    hits
}

fn search_page(document: &PdfDocument, index: usize, query: &str, opts: FindOpts, turns: u8) -> Vec<Vec<Frac>> {
    let Ok(page) = document.pages().get(index as _) else { return Vec::new() };
    let Ok(text) = page.text() else { return Vec::new() };
    if !opts.diacritics
        && (query.chars().any(|c| fold(c) != c) || text.all().chars().any(|c| fold(c) != c))
    {
        return search_folded(&page, &text, query, opts, turns);
    }
    let options = PdfSearchOptions::new().match_case(opts.case).match_whole_word(opts.word);
    let Ok(search) = text.search(query, &options) else { return Vec::new() };
    let (config, vw, vh) = virtual_config(&page, turns);
    let mut hits = Vec::new();
    while let Some(segments) = search.find_next() {
        let rects: Vec<Frac> =
            segments.iter().filter_map(|s| to_frac(&page, &config, vw, vh, &s.bounds())).collect();
        if !rects.is_empty() {
            hits.push(rects);
        }
    }
    hits
}

fn link_at(document: &PdfDocument, job: &LinkJob) -> Option<LinkTarget> {
    let page = document.pages().get(job.page as _).ok()?;
    let (config, vw, vh) = virtual_config(&page, job.turns);
    let (x, y) = page.pixels_to_points((job.point[0] * vw) as i32, (job.point[1] * vh) as i32, &config).ok()?;
    let links = page.links();
    let link = links.link_at_point(x, y)?;
    if let Some(action) = link.action() {
        if let Some(uri) = action.as_uri_action() {
            return uri.uri().ok().map(LinkTarget::Uri);
        }
        if let Some(local) = action.as_local_destination_action() {
            let target = local.destination().ok()?.page_index().ok()?;
            return Some(LinkTarget::Page(target as usize));
        }
        return None;
    }
    link.destination()?.page_index().ok().map(|p| LinkTarget::Page(p as usize))
}

/// The first and last character a piece covers, from where the pointer was.
fn piece_range(page: &PdfPage, text: &PdfPageText, piece: &SelectPiece, turns: u8) -> Option<(usize, usize)> {
    let count = text.len() as usize;
    if count == 0 {
        return None;
    }
    let (config, vw, vh) = virtual_config(page, turns);
    let index_at = |p: [f32; 2]| -> Option<usize> {
        let (x, y) = page.pixels_to_points((p[0] * vw) as i32, (p[1] * vh) as i32, &config).ok()?;
        let tolerance = PdfPoints::new(6.0);
        let chars = text.chars();
        if let Some(c) = chars.get_char_near_point(x, tolerance, y, tolerance) {
            return Some(c.index());
        }
        // Dragging from a margin or between lines still starts at the closest character.
        let (px, py) = (x.value, y.value);
        chars
            .iter()
            .filter_map(|c| {
                let r = c.loose_bounds().ok()?;
                let dx = (r.left().value - px).max(px - r.right().value).max(0.0);
                let dy = (r.bottom().value - py).max(py - r.top().value).max(0.0);
                Some((dy * 4.0 + dx, c.index()))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, i)| i)
    };
    let from = piece.from.and_then(index_at);
    let to = piece.to.and_then(index_at);
    match (piece.from.is_some(), piece.to.is_some()) {
        (true, true) => {
            let (a, b) = (from?, to?);
            Some((a.min(b), a.max(b)))
        }
        (true, false) => Some((from?, count - 1)),
        (false, true) => Some((0, to?)),
        (false, false) => Some((0, count - 1)),
    }
}

fn select_piece(document: &PdfDocument, piece: &SelectPiece, turns: u8) -> Option<(Vec<Frac>, String)> {
    let page = document.pages().get(piece.page as _).ok()?;
    let text = page.text().ok()?;
    let (first, last) = piece_range(&page, &text, piece, turns)?;
    let segments = text.segments_subset(first, last - first + 1);
    let (config, vw, vh) = virtual_config(&page, turns);
    let rects = segments.iter().filter_map(|s| to_frac(&page, &config, vw, vh, &s.bounds())).collect();
    // PDFium adds generated spaces and line breaks between words and lines, so reading the
    // characters in order gives the text the way it reads on the page.
    let chars = text.chars();
    let mut out = String::new();
    for i in first..=last {
        if let Some(s) = chars.get(i).ok().and_then(|c| c.unicode_string()) {
            out.push_str(&s);
        }
    }
    let out = out.replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n");
    Some((rects, out.trim().to_string()))
}

fn split(result: Result<Vec<usize>, String>) -> (Vec<usize>, Option<String>) {
    match result {
        Ok(pages) => (pages, None),
        Err(message) => (Vec::new(), Some(message)),
    }
}

fn pdf_color(rgb: [u8; 3]) -> PdfColor {
    PdfColor::new(rgb[0], rgb[1], rgb[2], 255)
}

/// `style` is 0 for a highlight, 1 for an underline and 2 for a strikethrough.
/// Writes the document next to the original, keeps the old file as "name (backup).pdf" the first time,
/// swaps the new file in and opens it again. The open file cannot be replaced while PDFium holds it.
fn save_original<'a>(
    pdfium: &'a Pdfium,
    documents: &mut HashMap<u64, PdfDocument<'a>>,
    passwords: &HashMap<u64, Option<String>>,
    doc: u64,
    path: &std::path::Path,
) -> Result<Option<PathBuf>, String> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let dir = path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let temp = dir.join(format!("{stem}.baca-saving"));
    let backup = dir.join(format!("{stem} (backup).pdf"));
    let password = passwords.get(&doc).cloned().flatten();

    documents.get(&doc).ok_or("The document is not open.")?.save_to_file(&temp).map_err(|e| describe(&e))?;
    let kept = if backup.exists() {
        None
    } else {
        match std::fs::copy(path, &backup) {
            Ok(_) => Some(backup),
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                return Err(format!("The backup could not be made, so nothing was changed: {e}"));
            }
        }
    };
    documents.remove(&doc);
    let reload = |from: &std::path::Path, documents: &mut HashMap<u64, PdfDocument<'a>>| {
        pdfium.load_pdf_from_file(from, password.as_deref()).ok().map(|d| documents.insert(doc, d)).is_some()
    };
    if let Err(e) = std::fs::rename(&temp, path) {
        // The new file could not be put in place: carry on from it so the changes are not lost.
        reload(&temp, documents);
        return Err(format!("The file could not be replaced ({e}). Your changes are still open; try Save a copy."));
    }
    if !reload(path, documents) {
        return Err("The file was saved, but it could not be opened again. Close the tab and open it once more.".into());
    }
    Ok(kept)
}

fn edit_highlight(document: &PdfDocument, pieces: &[SelectPiece], turns: u8, color: [u8; 3], style: u8) -> Result<Vec<usize>, String> {
    let mut touched = Vec::new();
    for piece in pieces {
        let mut page = document.pages().get(piece.page as _).map_err(|e| describe(&e))?;
        let rects: Vec<PdfRect> = {
            let text = page.text().map_err(|e| describe(&e))?;
            let Some((first, last)) = piece_range(&page, &text, piece, turns) else { continue };
            text.segments_subset(first, last - first + 1).iter().map(|s| s.bounds()).collect()
        };
        if rects.is_empty() {
            continue;
        }
        let (mut l, mut b, mut r, mut t) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for rect in &rects {
            l = l.min(rect.left().value);
            b = b.min(rect.bottom().value);
            r = r.max(rect.right().value);
            t = t.max(rect.top().value);
        }
        // The three markup kinds are different types with the same calls, hence the macro.
        macro_rules! mark {
            ($annotation:expr) => {{
                let mut annotation = $annotation.map_err(|e| describe(&e))?;
                annotation
                    .set_bounds(PdfRect::new(PdfPoints::new(b), PdfPoints::new(l), PdfPoints::new(t), PdfPoints::new(r)))
                    .map_err(|e| describe(&e))?;
                for rect in &rects {
                    annotation
                        .attachment_points_mut()
                        .create_attachment_point_at_end(PdfQuadPoints::new_from_values(
                            // PDFium reads the corners as top left, top right, bottom left, bottom right.
                            rect.left().value,
                            rect.top().value,
                            rect.right().value,
                            rect.top().value,
                            rect.left().value,
                            rect.bottom().value,
                            rect.right().value,
                            rect.bottom().value,
                        ))
                        .map_err(|e| describe(&e))?;
                }
                annotation.set_stroke_color(pdf_color(color)).map_err(|e| describe(&e))?;
            }};
        }
        match style {
            1 => mark!(page.annotations_mut().create_underline_annotation()),
            2 => mark!(page.annotations_mut().create_strikeout_annotation()),
            _ => mark!(page.annotations_mut().create_highlight_annotation()),
        }
        touched.push(piece.page);
    }
    Ok(touched)
}

fn edit_ink(document: &PdfDocument, index: usize, turns: u8, points: &[[f32; 2]], color: [u8; 3], width: f32) -> Result<usize, String> {
    let mut page = document.pages().get(index as _).map_err(|e| describe(&e))?;
    let (config, vw, vh) = virtual_config(&page, turns);
    let mut pts: Vec<(f32, f32)> = points
        .iter()
        .filter_map(|p| page.pixels_to_points((p[0] * vw) as i32, (p[1] * vh) as i32, &config).ok())
        .map(|(x, y)| (x.value, y.value))
        .collect();
    let Some(&first) = pts.first() else { return Err("There was nothing to draw.".into()) };
    if pts.len() == 1 {
        pts.push((first.0 + 0.1, first.1));
    }
    let pad = width + 1.0;
    let (mut l, mut b, mut r, mut t) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in &pts {
        l = l.min(x);
        r = r.max(x);
        b = b.min(y);
        t = t.max(y);
    }
    let mut annotation = page.annotations_mut().create_ink_annotation().map_err(|e| describe(&e))?;
    annotation
        .set_bounds(PdfRect::new(PdfPoints::new(b - pad), PdfPoints::new(l - pad), PdfPoints::new(t + pad), PdfPoints::new(r + pad)))
        .map_err(|e| describe(&e))?;
    let rgb = pdf_color(color);
    annotation.set_stroke_color(rgb).map_err(|e| describe(&e))?;
    for pair in pts.windows(2) {
        annotation
            .objects_mut()
            .create_path_object_line(
                PdfPoints::new(pair[0].0),
                PdfPoints::new(pair[0].1),
                PdfPoints::new(pair[1].0),
                PdfPoints::new(pair[1].1),
                rgb,
                PdfPoints::new(width),
            )
            .map_err(|e| describe(&e))?;
    }
    Ok(index)
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.5
}

/// Removes the annotation under the point, or text added with the text tool. Returns whether anything went.
fn edit_erase(
    document: &PdfDocument,
    ledger: &mut Vec<(usize, [f32; 4])>,
    index: usize,
    turns: u8,
    point: [f32; 2],
) -> Result<bool, String> {
    let mut page = document.pages().get(index as _).map_err(|e| describe(&e))?;
    let (config, vw, vh) = virtual_config(&page, turns);
    let (x, y) = page
        .pixels_to_points((point[0] * vw) as i32, (point[1] * vh) as i32, &config)
        .map_err(|e| describe(&e))?;
    let (x, y) = (x.value, y.value);
    let slack = 2.0;
    let mut hit = None;
    {
        let annotations = page.annotations();
        for i in (0..annotations.len()).rev() {
            let Ok(annotation) = annotations.get(i) else { continue };
            let erasable = matches!(
                annotation.annotation_type(),
                PdfPageAnnotationType::Highlight
                    | PdfPageAnnotationType::Underline
                    | PdfPageAnnotationType::Squiggly
                    | PdfPageAnnotationType::Strikeout
                    | PdfPageAnnotationType::Ink
                    | PdfPageAnnotationType::FreeText
                    | PdfPageAnnotationType::Text
                    | PdfPageAnnotationType::Square
                    | PdfPageAnnotationType::Circle
                    | PdfPageAnnotationType::Stamp
            );
            if !erasable {
                continue;
            }
            if let Ok(b) = annotation.bounds() {
                if x >= b.left().value - slack && x <= b.right().value + slack && y >= b.bottom().value - slack && y <= b.top().value + slack {
                    hit = Some(i);
                    break;
                }
            }
        }
    }
    if let Some(i) = hit {
        let mut annotations = page.annotations_mut();
        let annotation = annotations.get(i).map_err(|e| describe(&e))?;
        annotations.delete_annotation(annotation).map_err(|e| describe(&e))?;
        return Ok(true);
    }
    // Text the user added.
    for n in (0..ledger.len()).rev() {
        let (p, b) = ledger[n];
        if p != index || x < b[0] - slack || x > b[2] + slack || y < b[1] - slack || y > b[3] + slack {
            continue;
        }
        let found = {
            let objects = page.objects();
            (0..objects.len()).find(|&i| {
                objects
                    .get(i)
                    .ok()
                    .and_then(|o| o.bounds().ok())
                    .map(|r| near(r.left().value, b[0]) && near(r.bottom().value, b[1]) && near(r.right().value, b[2]))
                    .unwrap_or(false)
            })
        };
        if let Some(i) = found {
            page.objects_mut().remove_object_at_index(i).map_err(|e| describe(&e))?;
            page.regenerate_content().map_err(|e| describe(&e))?;
            ledger.remove(n);
            return Ok(true);
        }
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn edit_text(
    document: &mut PdfDocument,
    ledger: &mut Vec<(usize, [f32; 4])>,
    index: usize,
    turns: u8,
    point: [f32; 2],
    text: &str,
    size: f32,
    color: [u8; 3],
) -> Result<(), String> {
    if turns % 4 != 0 {
        return Err("Turn the page back upright to add text.".into());
    }
    let font = document.fonts_mut().helvetica();
    let mut page = document.pages().get(index as _).map_err(|e| describe(&e))?;
    let (config, vw, vh) = virtual_config(&page, turns);
    let (x, y) = page
        .pixels_to_points((point[0] * vw) as i32, (point[1] * vh) as i32, &config)
        .map_err(|e| describe(&e))?;
    // The click is the top left of the text; PDF places text by its baseline.
    let mut object = page
        .objects_mut()
        .create_text_object(x, PdfPoints::new(y.value - size * 0.8), text, font, PdfPoints::new(size))
        .map_err(|e| describe(&e))?;
    object.set_fill_color(pdf_color(color)).map_err(|e| describe(&e))?;
    let bounds = object.bounds().map_err(|e| describe(&e))?;
    ledger.push((index, [bounds.left().value, bounds.bottom().value, bounds.right().value, bounds.top().value]));
    page.regenerate_content().map_err(|e| describe(&e))?;
    Ok(())
}

fn outline(document: &PdfDocument) -> Vec<OutlineItem> {
    fn walk(b: Option<PdfBookmark>, depth: usize, out: &mut Vec<OutlineItem>) {
        let mut next = b;
        // Some files loop their outline back on itself; stop well before that hurts.
        while let Some(item) = next {
            if out.len() > 5_000 || depth > 32 {
                return;
            }
            let title = item.title().unwrap_or_default().trim().to_string();
            let page = item.destination().and_then(|d| d.page_index().ok()).map(|p| p as usize);
            out.push(OutlineItem { depth, title, page });
            walk(item.first_child(), depth + 1, out);
            next = item.next_sibling();
        }
    }
    let mut out = Vec::new();
    walk(document.bookmarks().root(), 0, &mut out);
    out
}

fn describe(e: &PdfiumError) -> String {
    match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            "This file is protected with a password. Password entry is not supported yet.".into()
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError) => {
            "The file is damaged or is not a PDF.".into()
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError) => {
            "The file could not be read. It may have been moved, or another app may be using it.".into()
        }
        PdfiumError::IoError(io) => match io.kind() {
            std::io::ErrorKind::NotFound => "The file is not there any more. It may have been moved, renamed, or deleted.".into(),
            std::io::ErrorKind::PermissionDenied => "Windows did not allow reading this file.".into(),
            _ => format!("The file could not be read: {io}"),
        },
        other => format!("PDFium reported an error: {other:?}"),
    }
}

fn properties(document: &PdfDocument) -> Vec<(String, String)> {
    let meta = document.metadata();
    let tag = |t: PdfDocumentMetadataTagType| meta.get(t).map(|m| m.value().trim().to_string()).unwrap_or_default();
    let date = |t| {
        let raw = tag(t);
        if raw.is_empty() { raw } else { crate::store::describe_pdf_date(&raw) }
    };
    let version = match document.version() {
        PdfDocumentVersion::Pdf1_0 => "1.0".into(),
        PdfDocumentVersion::Pdf1_1 => "1.1".into(),
        PdfDocumentVersion::Pdf1_2 => "1.2".into(),
        PdfDocumentVersion::Pdf1_3 => "1.3".into(),
        PdfDocumentVersion::Pdf1_4 => "1.4".into(),
        PdfDocumentVersion::Pdf1_5 => "1.5".into(),
        PdfDocumentVersion::Pdf1_6 => "1.6".into(),
        PdfDocumentVersion::Pdf1_7 => "1.7".into(),
        PdfDocumentVersion::Pdf2_0 => "2.0".into(),
        PdfDocumentVersion::Other(v) => format!("{}.{}", v / 10, v % 10),
        PdfDocumentVersion::Unset => String::new(),
    };
    vec![
        ("Title".into(), tag(PdfDocumentMetadataTagType::Title)),
        ("Author".into(), tag(PdfDocumentMetadataTagType::Author)),
        ("Subject".into(), tag(PdfDocumentMetadataTagType::Subject)),
        ("Keywords".into(), tag(PdfDocumentMetadataTagType::Keywords)),
        ("Created".into(), date(PdfDocumentMetadataTagType::CreationDate)),
        ("Modified".into(), date(PdfDocumentMetadataTagType::ModificationDate)),
        ("Application".into(), tag(PdfDocumentMetadataTagType::Creator)),
        ("PDF producer".into(), tag(PdfDocumentMetadataTagType::Producer)),
        ("PDF version".into(), version),
    ]
}

/// Largest bitmap sent to the printer for one page; past it the printer driver scales up.
const PRINT_PIXELS: f32 = 30_000_000.0;

fn print(document: &PdfDocument, job: &PrintJob) -> Result<usize, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{
        DeleteDC, GetDeviceCaps, SetStretchBltMode, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, HALFTONE, HDC, HORZRES, SRCCOPY, VERTRES,
    };
    use windows::Win32::Storage::Xps::{AbortDoc, EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};

    let hdc = HDC(job.hdc as _);
    let title: Vec<u16> = job.title.encode_utf16().chain(std::iter::once(0)).collect();
    let info = DOCINFOW { cbSize: std::mem::size_of::<DOCINFOW>() as i32, lpszDocName: PCWSTR(title.as_ptr()), ..Default::default() };
    let result = unsafe {
        if StartDocW(hdc, &info) <= 0 {
            let _ = DeleteDC(hdc);
            return Err("The printer did not accept the document.".into());
        }
        let (paper_w, paper_h) = (GetDeviceCaps(Some(hdc), HORZRES) as f32, GetDeviceCaps(Some(hdc), VERTRES) as f32);
        let mut printed = 0;
        let mut failure = None;
        for &index in &job.pages {
            let Ok(page) = document.pages().get(index as _) else { continue };
            let (w, h) = (page.width().value, page.height().value);
            // A landscape page on portrait paper, or the other way round, is turned to fill it.
            let turn = (w > h) != (paper_w > paper_h);
            let (dw, dh) = if turn { (h, w) } else { (w, h) };
            let fit = (paper_w / dw).min(paper_h / dh);
            let (out_w, out_h) = (dw * fit, dh * fit);
            let k = (PRINT_PIXELS / (out_w * out_h)).sqrt().min(1.0);
            let (bw, bh) = ((out_w * k).round().max(1.0) as i32, (out_h * k).round().max(1.0) as i32);
            let config = PdfRenderConfig::new()
                .set_fixed_size(bw, bh)
                .rotate(if turn { PdfPageRenderRotation::Degrees90 } else { PdfPageRenderRotation::None }, false)
                .set_format(PdfBitmapFormat::BGRA)
                .render_form_data(true)
                .use_print_quality(true);
            let Ok(mut bitmap) = PdfBitmap::empty(bw, bh, PdfBitmapFormat::BGRA) else { continue };
            if page.render_into_bitmap_with_config(&mut bitmap, &config).is_err() {
                continue;
            }
            let pixels = bitmap.as_raw_bytes();
            if StartPage(hdc) <= 0 {
                failure = Some("The printer stopped accepting pages.".to_string());
                break;
            }
            let header = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: bw,
                // Negative: rows run top to bottom, as PDFium writes them.
                biHeight: -bh,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let bmi = BITMAPINFO { bmiHeader: header, ..Default::default() };
            SetStretchBltMode(hdc, HALFTONE);
            let x = ((paper_w - out_w) / 2.0) as i32;
            let y = ((paper_h - out_h) / 2.0) as i32;
            StretchDIBits(hdc, x, y, out_w as i32, out_h as i32, 0, 0, bw, bh, Some(pixels.as_ptr() as _), &bmi, DIB_RGB_COLORS, SRCCOPY);
            EndPage(hdc);
            printed += 1;
        }
        match failure {
            Some(message) => {
                AbortDoc(hdc);
                Err(message)
            }
            None => {
                EndDoc(hdc);
                Ok(printed)
            }
        }
    };
    unsafe {
        let _ = DeleteDC(hdc);
    }
    result
}