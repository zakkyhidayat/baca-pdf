// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading text off a picture of a page with the recognizer that ships with Windows. It works
//! offline, with the languages installed in Windows.

use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

/// One recognized word and where it sits, in pixels of the picture, from the top left.
#[derive(Clone)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// `bgra` is a top-down picture of `width` x `height` pixels. Runs on the calling thread and waits.
pub fn recognize(bgra: &[u8], width: u32, height: u32) -> Result<Vec<Word>, String> {
    let fail = |e: windows::core::Error| e.message();
    unsafe {
        // Already set up on this thread is fine.
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(fail)
        .map_err(|_| "Windows has no text recognition for your languages. Add one in Settings, Time & language, Language & region.".to_string())?;
    let max = OcrEngine::MaxImageDimension().unwrap_or(2600);
    if width > max || height > max {
        return Err("The page is too large to read at this size.".into());
    }
    let writer = DataWriter::new().map_err(fail)?;
    writer.WriteBytes(bgra).map_err(fail)?;
    let buffer = writer.DetachBuffer().map_err(fail)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, width as i32, height as i32).map_err(fail)?;
    let result = engine.RecognizeAsync(&bitmap).map_err(fail)?.join().map_err(fail)?;
    let mut words = Vec::new();
    for line in result.Lines().map_err(fail)? {
        for word in line.Words().map_err(fail)? {
            let r = word.BoundingRect().map_err(fail)?;
            let text = word.Text().map_err(fail)?.to_string();
            if !text.trim().is_empty() {
                words.push(Word { text, x: r.X, y: r.Y, w: r.Width, h: r.Height });
            }
        }
    }
    Ok(words)
}

/// The largest side, in pixels, the recognizer accepts.
pub fn max_side() -> u32 {
    OcrEngine::MaxImageDimension().unwrap_or(2600)
}
