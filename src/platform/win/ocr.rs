//! Text recognition with the OCR engine built into Windows 10 and 11 (`Windows.Media.Ocr`).
//! It works offline and reads the languages that are installed in Windows (Settings > Time &
//! language > Language & region). Preparing the image and using the result is in `crate::ocr`.

use crate::ocr::{Bounds, Engine, Line, OcrError, Page, Word};
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

pub struct WinOcr {
    engine: OcrEngine,
    max: u32,
}

fn failed(e: windows::core::Error) -> OcrError {
    OcrError::Failed(e.message().to_string())
}

/// The engine for the user's languages (the first one Windows has a recognizer for), or else
/// for any language that is installed. Call it on the thread that reads.
pub fn ocr_engine() -> Result<Box<dyn Engine>, OcrError> {
    unsafe {
        // Fails harmlessly when the thread is already initialized.
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().ok().or_else(|| {
        let langs = OcrEngine::AvailableRecognizerLanguages().ok()?;
        let first = langs.GetAt(0).ok()?;
        OcrEngine::TryCreateFromLanguage(&first).ok()
    });
    let Some(engine) = engine else {
        return Err(OcrError::NoLanguage);
    };
    let max = OcrEngine::MaxImageDimension().unwrap_or(2600).max(1000);
    Ok(Box::new(WinOcr { engine, max }))
}

impl Engine for WinOcr {
    fn max_dimension(&self) -> u32 {
        self.max
    }

    fn read(&self, bgra: &[u8], w: u32, h: u32) -> Result<Page, OcrError> {
        let writer = DataWriter::new().map_err(failed)?;
        writer.WriteBytes(&bgra[..(w * h * 4) as usize]).map_err(failed)?;
        let buffer = writer.DetachBuffer().map_err(failed)?;
        // Screen captures leave the alpha byte at 0: read it as opaque, not as transparent.
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            w as i32,
            h as i32,
            BitmapAlphaMode::Ignore,
        )
        .map_err(failed)?;
        let result = self.engine.RecognizeAsync(&bitmap).map_err(failed)?.get().map_err(failed)?;
        let mut page = Page::default();
        for line in result.Lines().map_err(failed)? {
            let mut words = Vec::new();
            for word in line.Words().map_err(failed)? {
                let r = word.BoundingRect().map_err(failed)?;
                let text = word.Text().map_err(failed)?.to_string_lossy();
                if !text.trim().is_empty() {
                    words.push(Word { text, rect: Bounds::new(r.X, r.Y, r.Width, r.Height) });
                }
            }
            if !words.is_empty() {
                page.lines.push(Line { words });
            }
        }
        Ok(page)
    }
}

/// Opens Windows Settings where languages (with their text recognition) are added.
pub fn open_language_settings() {
    super::open_folder(std::path::Path::new("ms-settings:regionlanguage"));
}
