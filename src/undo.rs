// SPDX-License-Identifier: GPL-3.0-or-later

//! Undo and redo for what is added to the open document: highlights, underlines, strikethroughs,
//! drawings, shapes and notes, and moving, resizing or retyping them.
//!
//! PDFium numbers annotations by position, and a position shifts whenever an earlier one is removed. So
//! every annotation gets an id of its own, kept in a list per page that is updated on each change. What
//! was added is remembered as the job that made it, which is how it comes back after a removal.
//! Annotations that were already in the file can be moved or retyped and that can be undone, but when
//! one is removed it cannot be brought back, because nothing says how to draw it again.

use std::collections::HashMap;

use pdfium_render::prelude::*;

use crate::render::{self, EditJob};

enum Entry {
    /// These annotations were added (a highlight over several lines is still one).
    Added(Vec<u64>),
    Deleted { id: u64, rect: [f32; 4] },
    Moved { id: u64, from: [f32; 4], to: [f32; 4] },
    Text { id: u64, from: String, to: String },
    Color { id: u64, from: [u8; 3], to: [u8; 3] },
}

#[derive(Default)]
pub struct History {
    next: u64,
    ids: HashMap<usize, Vec<u64>>,
    made: HashMap<u64, EditJob>,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

const LIMIT: usize = 100;

fn page_of(job: &EditJob) -> usize {
    match job {
        EditJob::Highlight { pieces, .. } => pieces.first().map(|p| p.page).unwrap_or(0),
        EditJob::Ink { page, .. } | EditJob::Note { page, .. } => *page,
        _ => 0,
    }
}

fn count(document: &PdfDocument, page: usize) -> usize {
    document.pages().get(page as _).map(|p| p.annotations().len() as usize).unwrap_or(0)
}

/// An annotation's place as fractions of the page: left, top, right, bottom.
fn rect_of(document: &PdfDocument, page: usize, index: usize) -> Option<[f32; 4]> {
    let page = document.pages().get(page as _).ok()?;
    let (w, h) = (page.width().value.max(1.0), page.height().value.max(1.0));
    let annotations = page.annotations();
    let b = annotations.get(index).ok()?.bounds().ok()?;
    Some([b.left().value / w, 1.0 - b.top().value / h, b.right().value / w, 1.0 - b.bottom().value / h])
}

impl History {
    pub fn sync(&mut self, document: &PdfDocument, page: usize) {
        let n = count(document, page);
        let list = self.ids.entry(page).or_default();
        while list.len() < n {
            self.next += 1;
            list.push(self.next);
        }
    }

    /// The color a highlight made in this session was given, if this is one.
    pub fn color_of(&self, page: usize, index: usize) -> Option<[u8; 3]> {
        let id = self.ids.get(&page)?.get(index)?;
        match self.made.get(id)? {
            EditJob::Highlight { color, .. } => Some(*color),
            _ => None,
        }
    }

    fn set_made_color(&mut self, id: u64, to: [u8; 3]) {
        if let Some(EditJob::Highlight { color, .. }) = self.made.get_mut(&id) {
            *color = to;
        }
    }

    pub fn set_color(&mut self, document: &PdfDocument, page: usize, index: usize, to: [u8; 3]) -> Result<Vec<usize>, String> {
        self.sync(document, page);
        let id = *self.ids.get(&page).and_then(|l| l.get(index)).ok_or("That annotation is not there any more.")?;
        let from = self.color_of(page, index);
        render::set_color(document, page, index, to)?;
        match from {
            Some(from) => {
                self.set_made_color(id, to);
                self.push(Entry::Color { id, from, to });
            }
            // Its old color is not known, so this cannot be taken back.
            None => self.redo.clear(),
        }
        Ok(vec![page])
    }

    fn locate(&self, id: u64) -> Option<(usize, usize)> {
        self.ids.iter().find_map(|(page, list)| list.iter().position(|&x| x == id).map(|i| (*page, i)))
    }

    fn push(&mut self, entry: Entry) {
        self.undo.push(entry);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Makes one annotation from a job, giving it `id` (or a new one). Returns the page, if one was made.
    fn make(&mut self, document: &PdfDocument, job: EditJob, id: Option<u64>) -> Result<Option<(usize, u64)>, String> {
        let page = page_of(&job);
        self.sync(document, page);
        let made = render::create(document, &job)?;
        if made.is_empty() {
            return Ok(None);
        }
        let id = id.unwrap_or_else(|| {
            self.next += 1;
            self.next
        });
        self.ids.entry(page).or_default().push(id);
        self.made.insert(id, job);
        Ok(Some((page, id)))
    }

    /// A new highlight, drawing or note.
    pub fn create(&mut self, document: &PdfDocument, job: EditJob) -> Result<Vec<usize>, String> {
        // A highlight spanning several pages becomes one annotation per piece, each coming back on its own.
        let jobs: Vec<EditJob> = match job {
            EditJob::Highlight { doc, pieces, turns, color, style } => {
                pieces.into_iter().map(|p| EditJob::Highlight { doc, pieces: vec![p], turns, color, style }).collect()
            }
            other => vec![other],
        };
        let mut ids = Vec::new();
        let mut pages = Vec::new();
        for job in jobs {
            if let Some((page, id)) = self.make(document, job, None)? {
                ids.push(id);
                pages.push(page);
            }
        }
        if !ids.is_empty() {
            self.push(Entry::Added(ids));
        }
        Ok(pages)
    }

    pub fn delete(&mut self, document: &PdfDocument, page: usize, index: usize) -> Result<Vec<usize>, String> {
        self.sync(document, page);
        let id = *self.ids.get(&page).and_then(|l| l.get(index)).ok_or("That annotation is not there any more.")?;
        let rect = rect_of(document, page, index).unwrap_or([0.0; 4]);
        render::delete_annotation(document, page, index)?;
        if let Some(list) = self.ids.get_mut(&page) {
            list.remove(index);
        }
        if self.made.contains_key(&id) {
            self.push(Entry::Deleted { id, rect });
        } else {
            // It came with the file; it cannot be redrawn, and what could be undone before stays.
            self.redo.clear();
        }
        Ok(vec![page])
    }

    pub fn set_bounds(&mut self, document: &PdfDocument, page: usize, index: usize, to: [f32; 4]) -> Result<Vec<usize>, String> {
        self.sync(document, page);
        let id = *self.ids.get(&page).and_then(|l| l.get(index)).ok_or("That annotation is not there any more.")?;
        let from = rect_of(document, page, index).unwrap_or(to);
        render::set_bounds(document, page, index, to)?;
        self.push(Entry::Moved { id, from, to });
        Ok(vec![page])
    }

    pub fn set_note(&mut self, document: &PdfDocument, page: usize, index: usize, text: &str) -> Result<Vec<usize>, String> {
        self.sync(document, page);
        let id = *self.ids.get(&page).and_then(|l| l.get(index)).ok_or("That annotation is not there any more.")?;
        let from = render::contents_of(document, page, index).unwrap_or_default();
        render::set_note(document, page, index, text)?;
        self.push(Entry::Text { id, from, to: text.to_string() });
        Ok(vec![page])
    }

    fn remove_id(&mut self, document: &PdfDocument, id: u64) -> Result<Option<usize>, String> {
        let Some((page, index)) = self.locate(id) else { return Ok(None) };
        render::delete_annotation(document, page, index)?;
        if let Some(list) = self.ids.get_mut(&page) {
            list.remove(index);
        }
        Ok(Some(page))
    }

    fn restore_id(&mut self, document: &PdfDocument, id: u64) -> Result<Option<usize>, String> {
        let Some(job) = self.made.get(&id).cloned() else { return Ok(None) };
        Ok(self.make(document, job, Some(id))?.map(|(page, _)| page))
    }

    fn set_rect(&mut self, document: &PdfDocument, id: u64, rect: [f32; 4]) -> Result<Option<usize>, String> {
        let Some((page, index)) = self.locate(id) else { return Ok(None) };
        render::set_bounds(document, page, index, rect)?;
        Ok(Some(page))
    }

    /// Takes the last step back (`undo`) or forward again, and returns the pages that changed.
    pub fn step(&mut self, document: &PdfDocument, undo: bool) -> Result<Vec<usize>, String> {
        let entry = if undo { self.undo.pop() } else { self.redo.pop() };
        let Some(entry) = entry else { return Ok(Vec::new()) };
        let mut pages = Vec::new();
        let result: Result<(), String> = (|| {
            match &entry {
                Entry::Added(ids) => {
                    for &id in ids {
                        let page = if undo { self.remove_id(document, id)? } else { self.restore_id(document, id)? };
                        pages.extend(page);
                    }
                }
                Entry::Deleted { id, rect, .. } => {
                    if undo {
                        pages.extend(self.restore_id(document, *id)?);
                        pages.extend(self.set_rect(document, *id, *rect)?);
                    } else {
                        pages.extend(self.remove_id(document, *id)?);
                    }
                }
                Entry::Moved { id, from, to, .. } => pages.extend(self.set_rect(document, *id, if undo { *from } else { *to })?),
                Entry::Color { id, from, to } => {
                    if let Some((page, index)) = self.locate(*id) {
                        let color = if undo { *from } else { *to };
                        render::set_color(document, page, index, color)?;
                        self.set_made_color(*id, color);
                        pages.push(page);
                    }
                }
                Entry::Text { id, from, to, .. } => {
                    if let Some((page, index)) = self.locate(*id) {
                        render::set_note(document, page, index, if undo { from } else { to })?;
                        pages.push(page);
                    }
                }
            }
            Ok(())
        })();
        let target = if undo { &mut self.redo } else { &mut self.undo };
        target.push(entry);
        result.map(|_| {
            pages.sort_unstable();
            pages.dedup();
            pages
        })
    }

    /// What Ctrl+Z and Ctrl+Y would do now, in a few words.
    pub fn labels(&self) -> (Option<String>, Option<String>) {
        (self.undo.last().map(|e| self.describe(e)), self.redo.last().map(|e| self.describe(e)))
    }

    fn describe(&self, entry: &Entry) -> String {
        let kind = |id: u64| match self.made.get(&id) {
            Some(EditJob::Highlight { style: 1, .. }) => "underline",
            Some(EditJob::Highlight { style: 2, .. }) => "strikethrough",
            Some(EditJob::Highlight { .. }) => "highlight",
            Some(EditJob::Ink { .. }) => "drawing",
            Some(EditJob::Note { .. }) => "note",
            _ => "annotation",
        };
        match entry {
            Entry::Added(ids) => format!("adding a {}", kind(ids.first().copied().unwrap_or(0))),
            Entry::Deleted { id, .. } => format!("removing a {}", kind(*id)),
            Entry::Moved { .. } => "moving or resizing".to_string(),
            Entry::Text { .. } => "editing a note".to_string(),
            Entry::Color { .. } => "changing a color".to_string(),
        }
    }

    /// After the file was written and opened again the positions may differ, so the past is let go.
    pub fn reset(&mut self) {
        *self = History { next: self.next, ..History::default() };
    }
}
