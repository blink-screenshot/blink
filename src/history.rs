//! Screenshot history: the last captures, kept on this PC so the tray menu can copy or edit
//! them again. Stored in %LOCALAPPDATA%\Blink\history (local, not roaming: screenshots should
//! not travel with a roaming profile). Per screenshot:
//!
//! - `<id>.png`: the screenshot, without the mouse pointer
//! - `<id>.pointer.png`: the mouse pointer, when it was captured (kept apart for the editor)
//! - `<id>.thumb.png`: a small preview for the menu
//! - `<id>.json`: size and pointer position; written last, so a half-written entry is never
//!   listed
//!
//! `<id>` is the capture time in milliseconds since 1970.

use crate::imageio::{self, Pointer};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Largest size of the stored preview; the menu scales it down to its DPI.
pub const THUMB_W: u32 = 192;
pub const THUMB_H: u32 = 120;
/// The most screenshots the setting allows.
pub const MAX_KEEP: u32 = 100;

/// How much the history keeps (from the settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most screenshots; 0 = History is off.
    pub keep: u32,
    /// The oldest a screenshot may get, in hours; 0 = no limit.
    pub max_age_hours: u32,
}

impl Limits {
    pub fn from_config(c: &crate::config::Config) -> Self {
        Self { keep: c.history_size, max_age_hours: c.history_max_age_hours }
    }
    /// Whether a screenshot taken at `id` (ms) is still kept at `now` (ms).
    fn fresh(&self, id: u64, now: u64) -> bool {
        self.max_age_hours == 0 || now.saturating_sub(id) <= self.max_age_hours as u64 * 3_600_000
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Meta {
    width: u32,
    height: u32,
    #[serde(default)]
    pointer: Option<(i32, i32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: u64,
    pub width: u32,
    pub height: u32,
}

impl Entry {
    /// When it was taken, in local time.
    pub fn time(&self) -> Option<chrono::DateTime<chrono::Local>> {
        chrono::DateTime::from_timestamp_millis(self.id as i64).map(|t| t.with_timezone(&chrono::Local))
    }
}

pub fn dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(crate::config::config_dir).join("Blink").join("history")
}

fn file(dir: &Path, id: u64, suffix: &str) -> PathBuf {
    dir.join(format!("{id}{suffix}"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Adds a screenshot and removes what is beyond the limits. Slow for big screenshots (PNG):
/// call it off the UI thread.
pub fn add(img: &RgbaImage, pointer: Option<&Pointer>, limits: Limits) -> Result<u64, String> {
    add_in(&dir(), img, pointer, limits, now_ms())
}

pub fn add_in(dir: &Path, img: &RgbaImage, pointer: Option<&Pointer>, limits: Limits, now: u64) -> Result<u64, String> {
    if limits.keep == 0 {
        return Err("history is off".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut id = now;
    while file(dir, id, ".json").exists() || file(dir, id, ".png").exists() {
        id += 1;
    }
    let png = crate::config::ImageFormat::Png;
    imageio::save_image(&file(dir, id, ".png"), img, png, 100)?;
    let mut flat = None;
    if let Some(p) = pointer {
        imageio::save_image(&file(dir, id, ".pointer.png"), &p.img, png, 100)?;
        let mut f = img.clone();
        imageio::flatten(&mut f, p);
        flat = Some(f);
    }
    let thumb = fit(flat.as_ref().unwrap_or(img));
    imageio::save_image(&file(dir, id, ".thumb.png"), &thumb, png, 100)?;
    let meta = Meta { width: img.width(), height: img.height(), pointer: pointer.map(|p| (p.x, p.y)) };
    let json = serde_json::to_string(&meta).map_err(|e| e.to_string())?;
    std::fs::write(file(dir, id, ".json"), json).map_err(|e| e.to_string())?;
    prune_in(dir, limits, now);
    Ok(id)
}

/// A preview within `THUMB_W` x `THUMB_H`, keeping the aspect ratio.
fn fit(img: &RgbaImage) -> RgbaImage {
    let s = (THUMB_W as f32 / img.width() as f32).min(THUMB_H as f32 / img.height() as f32).min(1.0);
    let w = ((img.width() as f32 * s).round() as u32).max(1);
    let h = ((img.height() as f32 * s).round() as u32).max(1);
    image::imageops::thumbnail(img, w, h)
}

/// The screenshots within the limits, newest first.
pub fn list(limits: Limits) -> Vec<Entry> {
    let now = now_ms();
    list_in(&dir()).into_iter().filter(|e| limits.fresh(e.id, now)).take(limits.keep as usize).collect()
}

/// Every finished entry on disk, newest first.
pub fn list_in(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Entry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id: u64 = name.strip_suffix(".json")?.parse().ok()?;
            let meta: Meta = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            Some(Entry { id, width: meta.width, height: meta.height })
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.id));
    out
}

/// The screenshot and its pointer.
pub fn load(id: u64) -> Result<(RgbaImage, Option<Pointer>), String> {
    load_in(&dir(), id)
}

pub fn load_in(dir: &Path, id: u64) -> Result<(RgbaImage, Option<Pointer>), String> {
    let meta: Meta = std::fs::read_to_string(file(dir, id, ".json"))
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))?;
    let img = imageio::load_image_file(&file(dir, id, ".png"))?;
    let pointer = meta.pointer.and_then(|(x, y)| {
        let img = imageio::load_image_file(&file(dir, id, ".pointer.png")).ok()?;
        Some(Pointer { img, x, y })
    });
    Ok((img, pointer))
}

pub fn thumbnail(id: u64) -> Option<RgbaImage> {
    image::open(file(&dir(), id, ".thumb.png")).ok().map(|i| i.to_rgba8())
}

/// Removes the screenshots beyond the limits (the oldest beyond `keep`, and those older than
/// `max_age_hours`), and leftovers of entries that were never finished (no `.json`).
pub fn prune(limits: Limits) {
    prune_in(&dir(), limits, now_ms())
}

pub fn prune_in(dir: &Path, limits: Limits, now: u64) {
    let entries = list_in(dir);
    let kept: std::collections::HashSet<u64> =
        entries.iter().filter(|e| limits.fresh(e.id, now)).take(limits.keep as usize).map(|e| e.id).collect();
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let newest = entries.first().map_or(0, |e| e.id);
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = name.split('.').next().and_then(|s| s.parse::<u64>().ok()) else { continue };
        // Files newer than the newest finished entry may belong to one being written now.
        if !kept.contains(&id) && id <= newest {
            std::fs::remove_file(e.path()).ok();
        }
    }
}

/// Deletes every screenshot in the history.
pub fn clear() {
    clear_in(&dir())
}

pub fn clear_in(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.split('.').next().is_some_and(|s| s.parse::<u64>().is_ok()) {
            std::fs::remove_file(e.path()).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blink_test_history_{name}_{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        d
    }

    #[test]
    fn keeps_the_newest_and_loads_them_back() {
        let d = tmp("keep");
        let img = |v: u8| RgbaImage::from_pixel(400, 100, image::Rgba([v, v, v, 255]));
        let limits = Limits { keep: 3, max_age_hours: 0 };
        for i in 0..5u64 {
            add_in(&d, &img(i as u8), None, limits, 1000 + i).unwrap();
        }
        let ids: Vec<u64> = list_in(&d).iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![1004, 1003, 1002]);
        let (back, ptr) = load_in(&d, 1003).unwrap();
        assert_eq!(back, img(3));
        assert!(ptr.is_none());
        // Only the files of the three kept entries remain.
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 9);
        let thumb = image::open(file(&d, 1004, ".thumb.png")).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (THUMB_W, 48));
        clear_in(&d);
        assert!(list_in(&d).is_empty());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn same_millisecond_and_pointer() {
        let d = tmp("ptr");
        let img = RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 255]));
        let p = Pointer { img: RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])), x: 3, y: 4 };
        let limits = Limits { keep: 10, max_age_hours: 0 };
        let a = add_in(&d, &img, Some(&p), limits, 5).unwrap();
        let b = add_in(&d, &img, None, limits, 5).unwrap();
        assert_ne!(a, b);
        let (back, ptr) = load_in(&d, a).unwrap();
        assert_eq!(back, img);
        assert_eq!(ptr, Some(p));
        // The preview shows the pointer.
        let thumb = image::open(file(&d, a, ".thumb.png")).unwrap().to_rgba8();
        assert_eq!(thumb.get_pixel(3, 4).0, [255, 0, 0, 255]);
        assert!(add_in(&d, &img, None, Limits { keep: 0, ..limits }, 9).is_err());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn old_screenshots_expire() {
        let d = tmp("age");
        let img = RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]));
        let hour = 3_600_000;
        let day = Limits { keep: 10, max_age_hours: 24 };
        let start = 100 * hour;
        add_in(&d, &img, None, day, start).unwrap();
        add_in(&d, &img, None, day, start + 20 * hour).unwrap();
        assert_eq!(list_in(&d).len(), 2);
        // 25 hours after the first one: it is gone, the second one stays.
        prune_in(&d, day, start + 25 * hour);
        let ids: Vec<u64> = list_in(&d).iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![start + 20 * hour]);
        assert!(!day.fresh(start, start + 25 * hour) && day.fresh(start, start + 24 * hour));
        // No age limit: kept however old.
        let forever = Limits { max_age_hours: 0, ..day };
        prune_in(&d, forever, start + 10_000 * hour);
        assert_eq!(list_in(&d).len(), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn unfinished_entries_are_not_listed() {
        let d = tmp("half");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("77.png"), b"x").unwrap();
        assert!(list_in(&d).is_empty());
        std::fs::remove_dir_all(&d).ok();
    }
}
