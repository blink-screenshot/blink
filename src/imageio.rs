//! Image I/O: hand-off files between processes, saving, clipboard.

use crate::config::ImageFormat;
use crate::i18n::{t, tf};
use image::RgbaImage;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const RAW_MAGIC: &[u8; 8] = b"BLNKRAW1";

/// Fast uncompressed hand-off format (resident process -> editor process).
pub fn write_raw(path: &Path, img: &RgbaImage) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(RAW_MAGIC)?;
    f.write_all(&img.width().to_le_bytes())?;
    f.write_all(&img.height().to_le_bytes())?;
    f.write_all(img.as_raw())?;
    f.flush()
}

pub fn read_raw(path: &Path) -> std::io::Result<RgbaImage> {
    let mut f = std::fs::File::open(path)?;
    let mut header = [0u8; 16];
    f.read_exact(&mut header)?;
    if &header[..8] != RAW_MAGIC {
        return Err(std::io::Error::other("not a Blink raw image"));
    }
    let w = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let h = u32::from_le_bytes(header[12..16].try_into().unwrap());
    let mut data = vec![0u8; w as usize * h as usize * 4];
    f.read_exact(&mut data)?;
    RgbaImage::from_raw(w, h, data).ok_or_else(|| std::io::Error::other("bad raw image size"))
}

pub fn temp_raw_path() -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("blink_{}_{nanos}.raw", std::process::id()))
}

/// Deletes screenshots that were handed to an editor that never read them (it crashed or
/// was killed first). Only files older than a few minutes: a new editor may still be starting.
pub fn remove_stale_raw_files() {
    let Ok(dir) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("blink_") && name.ends_with(".raw")) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() > 300);
        if old {
            std::fs::remove_file(e.path()).ok();
        }
    }
}

/// Asks where to save a screenshot: starts in the screenshots folder with the next file
/// name, the configured format first. Without an extension the configured one is added.
pub fn ask_save_path(c: &crate::config::Config) -> Option<PathBuf> {
    let default_name = c
        .next_output_path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("screenshot.{}", c.format.ext()));
    // Always start in the screenshots folder. It must exist, otherwise Windows silently
    // falls back to the last folder it remembers for the app.
    let dir = c.output_dir.clone();
    std::fs::create_dir_all(&dir).ok();
    let mut formats = ImageFormat::ALL.to_vec();
    formats.sort_by_key(|f| *f != c.format);
    let mut dialog =
        rfd::FileDialog::new().set_title(t("Save screenshot as")).set_directory(dir).set_file_name(default_name);
    for f in formats {
        dialog = match f {
            ImageFormat::Png => dialog.add_filter(t("PNG image"), &["png"]),
            ImageFormat::Jpg => dialog.add_filter(t("JPEG image"), &["jpg", "jpeg"]),
            ImageFormat::Webp => dialog.add_filter(t("WebP image"), &["webp"]),
        };
    }
    let mut path = dialog.save_file()?;
    if path.extension().is_none() {
        path.set_extension(c.format.ext());
    }
    Some(path)
}

pub fn format_from_path(path: &Path, fallback: ImageFormat) -> ImageFormat {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => ImageFormat::Jpg,
        Some("png") => ImageFormat::Png,
        Some("webp") => ImageFormat::Webp,
        _ => fallback,
    }
}

pub fn save_image(path: &Path, img: &RgbaImage, format: ImageFormat, jpg_quality: u8) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| tf("Cannot create folder {1}: {0}", &[&e, &dir.display()]))?;
    }
    let file = std::fs::File::create(path).map_err(|e| tf("Cannot write {1}: {0}", &[&e, &path.display()]))?;
    let mut w = std::io::BufWriter::new(file);
    match format {
        ImageFormat::Png => {
            let enc = image::codecs::png::PngEncoder::new_with_quality(
                &mut w,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Adaptive,
            );
            img.write_with_encoder(enc).map_err(|e| e.to_string())?;
        }
        ImageFormat::Jpg => {
            // JPEG has no alpha: flatten onto white.
            let rgb = image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
                let p = img.get_pixel(x, y).0;
                let a = p[3] as u32;
                let blend = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
                image::Rgb([blend(p[0]), blend(p[1]), blend(p[2])])
            });
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, jpg_quality.clamp(1, 100));
            rgb.write_with_encoder(enc).map_err(|e| e.to_string())?;
        }
        ImageFormat::Webp => {
            let enc = image::codecs::webp::WebPEncoder::new_lossless(&mut w);
            img.write_with_encoder(enc).map_err(|e| e.to_string())?;
        }
    }
    w.flush().map_err(|e| e.to_string())
}

pub fn copy_to_clipboard(img: &RgbaImage) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_image(arboard::ImageData {
        width: img.width() as usize,
        height: img.height() as usize,
        bytes: std::borrow::Cow::Borrowed(img.as_raw()),
    })
    .map_err(|e| e.to_string())
}

pub fn image_from_clipboard() -> Result<RgbaImage, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    if let Ok(data) = cb.get_image() {
        return RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned())
            .ok_or_else(|| "Clipboard image has an unexpected size".to_string());
    }
    // A copied image *file* (e.g. from Explorer) arrives as a path in text form.
    if let Ok(text) = cb.get_text() {
        let p = PathBuf::from(text.trim().trim_matches('"'));
        if p.is_file() {
            return load_image_file(&p);
        }
    }
    Err(t("The clipboard does not contain an image").into())
}

pub fn load_image_file(path: &Path) -> Result<RgbaImage, String> {
    image::open(path).map(|i| i.to_rgba8()).map_err(|e| tf("Cannot open {1}: {0}", &[&e, &path.display()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_roundtrip() {
        let img = RgbaImage::from_fn(7, 3, |x, y| image::Rgba([x as u8, y as u8, 9, 255]));
        let p = std::env::temp_dir().join("blink_test_roundtrip.raw");
        write_raw(&p, &img).unwrap();
        let back = read_raw(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(img, back);
    }
}
