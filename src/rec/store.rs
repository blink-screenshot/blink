//! Where recordings live while they can still be edited.
//!
//! Each recording gets a folder in `%LOCALAPPDATA%\Blink\recordings`, named after the moment
//! it started (Unix milliseconds):
//!
//! ```text
//! master.mp4     the clean video, without pointer, at a higher bitrate
//! events.jsonl   the event log (see `events`)
//! cursors\7.png  every pointer shape, once
//! spec.json      what to record (written by the tray, read by the recorder)
//! result.json    how it ended (written by the recorder, read by the tray)
//! ```
//!
//! The exported MP4 or GIF goes to the recordings folder the user chose (Videos\Blink by
//! default). These working folders are only for the clip window, and are deleted after a day,
//! beyond the newest few: they hold the key log and take room.

use super::keys::KeyMode;
use crate::config::{Recording, VideoFormat};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How many working folders are kept, and for how long.
pub const KEEP: usize = 5;
pub const MAX_AGE_MS: u64 = 24 * 3600 * 1000;
/// A folder without `result.json` is a recording in progress, unless it is older than this.
const ABANDONED_MS: u64 = 12 * 3600 * 1000;

pub fn root() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(crate::config::config_dir).join("Blink").join("recordings")
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The working folder of one recording.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rec {
    pub dir: PathBuf,
}

impl Rec {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Rec { dir: dir.into() }
    }
    pub fn master(&self) -> PathBuf {
        self.dir.join("master.mp4")
    }
    pub fn events(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }
    pub fn cursors(&self) -> PathBuf {
        self.dir.join("cursors")
    }
    pub fn spec(&self) -> PathBuf {
        self.dir.join("spec.json")
    }
    pub fn result(&self) -> PathBuf {
        self.dir.join("result.json")
    }
    /// When it started (from the folder name), in Unix ms.
    pub fn id(&self) -> Option<u64> {
        self.dir.file_name()?.to_str()?.parse().ok()
    }

    pub fn read_spec(&self) -> Result<Spec, String> {
        let text = std::fs::read_to_string(self.spec()).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }
    pub fn write_spec(&self, spec: &Spec) -> Result<(), String> {
        write_json(&self.spec(), spec)
    }
    pub fn read_result(&self) -> Option<Outcome> {
        let text = std::fs::read_to_string(self.result()).ok()?;
        serde_json::from_str(&text).ok()
    }
    pub fn write_result(&self, o: &Outcome) -> Result<(), String> {
        write_json(&self.result(), o)
    }

    /// The event log and pointer pictures, for drawing.
    pub fn clip(&self) -> Result<super::export::Clip, String> {
        Ok(super::export::Clip {
            log: super::events::read(&self.events())?,
            cursors: super::compose::Cursors::load(&self.cursors()),
        })
    }
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// A new, empty working folder under `root`.
pub fn create(root: &Path, now: u64) -> std::io::Result<Rec> {
    std::fs::create_dir_all(root)?;
    let mut id = now;
    loop {
        let dir = root.join(id.to_string());
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                std::fs::create_dir_all(dir.join("cursors"))?;
                return Ok(Rec { dir });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => id += 1,
            Err(e) => return Err(e),
        }
    }
}

/// Every working folder, newest first.
pub fn list(root: &Path) -> Vec<Rec> {
    let mut out: Vec<Rec> = std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| Rec { dir: e.path() })
                .filter(|r| r.id().is_some())
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|r| std::cmp::Reverse(r.id()));
    out
}

/// Deletes working folders older than a day and beyond the newest `KEEP` that are finished.
/// A recording in progress (no result yet) is left alone, unless it is clearly abandoned.
pub fn prune(root: &Path, now: u64) {
    let mut finished = 0;
    for r in list(root) {
        let age = now.saturating_sub(r.id().unwrap_or(0));
        let done = r.result().exists();
        let delete = if done {
            finished += 1;
            finished > KEEP || age > MAX_AGE_MS
        } else {
            age > ABANDONED_MS
        };
        if delete {
            std::fs::remove_dir_all(&r.dir).ok();
        }
    }
}

/// What to record. Written by the tray when you have picked the area, read by the recorder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    /// The area, in virtual-screen pixels.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// The monitor it is on.
    pub monitor: [i32; 4],
    /// Its display scale.
    pub scale: f32,
    pub settings: Recording,
    pub filename_pattern: String,
    /// The recording hotkey: pressing it stops the recording, so it is not shown as a key.
    #[serde(default)]
    pub hotkey: Option<crate::config::Hotkey>,
}

impl Spec {
    /// What gets logged of the keyboard (nothing when off).
    pub fn keys(&self) -> KeyMode {
        self.settings.keys
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Recorded and exported to `path`.
    Saved,
    /// Recorded, but the export failed: the master is there, the clip window can try again.
    ExportFailed,
    /// Thrown away with the bin button.
    Discarded,
    /// Esc during the countdown.
    Cancelled,
    /// Could not record at all (no Media Foundation, ...).
    Failed,
}

/// How a recording ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub status: Status,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub format: Option<VideoFormat>,
    #[serde(default)]
    pub duration_ms: u32,
    #[serde(default)]
    pub bytes: u64,
    /// For people: what went wrong.
    #[serde(default)]
    pub error: Option<String>,
    /// Media Foundation is missing (a Windows N edition without the Media Feature Pack).
    #[serde(default)]
    pub no_media_foundation: bool,
}

impl Outcome {
    pub fn simple(status: Status) -> Self {
        Outcome { status, path: None, format: None, duration_ms: 0, bytes: 0, error: None, no_media_foundation: false }
    }
    pub fn failed(error: String) -> Self {
        Outcome { error: Some(error), ..Outcome::simple(Status::Failed) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blink-rec-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        d
    }

    #[test]
    fn creates_unique_folders() {
        let root = temp_root("create");
        let a = create(&root, 1000).unwrap();
        let b = create(&root, 1000).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.id(), Some(1000));
        assert_eq!(b.id(), Some(1001));
        assert!(a.cursors().is_dir());
        assert_eq!(list(&root), vec![b, a]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn spec_and_result_round_trip() {
        let root = temp_root("json");
        let r = create(&root, 5).unwrap();
        let spec = Spec {
            x: -1920,
            y: 0,
            w: 800,
            h: 600,
            monitor: [-1920, 0, 1920, 1080],
            scale: 1.25,
            settings: Recording::default(),
            filename_pattern: "Blink_%Y".into(),
            hotkey: Some(crate::config::Hotkeys::default().record),
        };
        r.write_spec(&spec).unwrap();
        assert_eq!(r.read_spec().unwrap(), spec);
        assert!(r.read_result().is_none());
        let out =
            Outcome { path: Some("C:/a.mp4".into()), duration_ms: 1200, bytes: 42, ..Outcome::simple(Status::Saved) };
        r.write_result(&out).unwrap();
        assert_eq!(r.read_result(), Some(out));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn prune_keeps_recent_and_running_recordings() {
        let root = temp_root("prune");
        let now = 100 * 24 * 3600 * 1000;
        let finished = |id: u64| {
            let r = create(&root, id).unwrap();
            r.write_result(&Outcome::simple(Status::Saved)).unwrap();
            r
        };
        // Seven finished in the last hour: the newest five stay.
        let recent: Vec<Rec> = (0..7).map(|i| finished(now - 3_600_000 + i * 1000)).collect();
        // One finished two days ago: gone.
        let old = finished(now - 2 * MAX_AGE_MS);
        // One running (no result) for ten minutes: stays. One "running" for a day: abandoned.
        let running = create(&root, now - 600_000).unwrap();
        let abandoned = create(&root, now - MAX_AGE_MS).unwrap();
        prune(&root, now);
        assert!(!recent[0].dir.exists() && !recent[1].dir.exists());
        assert!(recent[2..].iter().all(|r| r.dir.exists()));
        assert!(!old.dir.exists());
        assert!(running.dir.exists());
        assert!(!abandoned.dir.exists());
        std::fs::remove_dir_all(&root).ok();
    }
}
