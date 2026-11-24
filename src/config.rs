//! Persistent settings, stored as JSON in %APPDATA%\Blink\config.json.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A global hotkey: modifier flags plus a Windows virtual-key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Windows virtual-key code (0 = disabled).
    pub vk: u32,
    /// Take the key combination even when another app registered it (keyboard hook).
    #[serde(default)]
    pub force: bool,
}

pub const VK_SNAPSHOT: u32 = 0x2C;

impl Hotkey {
    pub const fn new(ctrl: bool, alt: bool, shift: bool, vk: u32) -> Self {
        Self { ctrl, alt, shift, win: false, vk, force: false }
    }
    #[cfg(test)]
    pub const fn none() -> Self {
        Self { ctrl: false, alt: false, shift: false, win: false, vk: 0, force: false }
    }
    /// Same key combination (ignoring how it is registered).
    pub fn same_combo(&self, o: &Hotkey) -> bool {
        (self.ctrl, self.alt, self.shift, self.win, self.vk) == (o.ctrl, o.alt, o.shift, o.win, o.vk)
    }
    pub fn is_enabled(&self) -> bool {
        self.vk != 0
    }
    pub fn label(&self) -> String {
        if !self.is_enabled() {
            return crate::i18n::t("(none)").into();
        }
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(key_name(self.vk));
        parts.join(" + ")
    }
}

/// Keys offered in the settings UI, as (virtual-key code, name).
pub fn selectable_keys() -> Vec<(u32, String)> {
    let mut keys = vec![(0, crate::i18n::t("(none)").to_string()), (VK_SNAPSHOT, "Print Screen".to_string())];
    for (i, f) in (0x70..=0x87).enumerate() {
        keys.push((f, format!("F{}", i + 1)));
    }
    for c in b'A'..=b'Z' {
        keys.push((c as u32, (c as char).to_string()));
    }
    for c in b'0'..=b'9' {
        keys.push((c as u32, (c as char).to_string()));
    }
    keys.extend([
        (0x13, "Pause".to_string()),
        (0x91, "Scroll Lock".to_string()),
        (0x2D, "Insert".to_string()),
        (0x24, "Home".to_string()),
        (0x23, "End".to_string()),
        (0x21, "Page Up".to_string()),
        (0x22, "Page Down".to_string()),
    ]);
    keys
}

pub fn key_name(vk: u32) -> String {
    selectable_keys().into_iter().find(|(k, _)| *k == vk).map(|(_, n)| n).unwrap_or_else(|| format!("Key 0x{vk:02X}"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    /// Region / window selection overlay.
    pub region: Hotkey,
    /// Full monitor under the mouse cursor.
    pub fullscreen: Hotkey,
    /// Foreground window.
    pub active_window: Hotkey,
    /// Scrolling (full page) capture.
    pub scrolling: Hotkey,
    /// Small menu to pick the capture type.
    pub picker: Hotkey,
    /// Copy the text you point at (the overlay in text mode).
    pub text: Hotkey,
    /// Start a screen recording (the overlay in recording mode), or stop the one that runs.
    pub record: Hotkey,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            region: Hotkey::new(false, false, false, VK_SNAPSHOT),
            fullscreen: Hotkey::new(false, false, true, VK_SNAPSHOT),
            active_window: Hotkey::new(false, true, false, VK_SNAPSHOT),
            scrolling: Hotkey::new(true, false, true, VK_SNAPSHOT),
            picker: Hotkey::new(true, false, false, VK_SNAPSHOT),
            text: Hotkey::new(true, true, false, VK_SNAPSHOT),
            // Not Alt + Shift + Print Screen: Left Alt + Left Shift + Print Screen switches
            // High Contrast on in Windows.
            record: Hotkey::new(true, true, true, VK_SNAPSHOT),
        }
    }
}

impl Hotkeys {
    /// (stable id, display name, hotkey) for every capture hotkey.
    pub fn list(&self) -> [(&'static str, &'static str, Hotkey); 7] {
        [
            ("region", crate::i18n::t("Region / window"), self.region),
            ("fullscreen", crate::i18n::t("Full screen"), self.fullscreen),
            ("active_window", crate::i18n::t("Active window"), self.active_window),
            ("scrolling", crate::i18n::t("Scrolling page"), self.scrolling),
            ("text", crate::i18n::t("Copy text from the screen"), self.text),
            ("record", crate::i18n::t("Record the screen"), self.record),
            ("picker", crate::i18n::t("Capture menu"), self.picker),
        ]
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Hotkey> {
        match id {
            "region" => Some(&mut self.region),
            "fullscreen" => Some(&mut self.fullscreen),
            "active_window" => Some(&mut self.active_window),
            "scrolling" => Some(&mut self.scrolling),
            "picker" => Some(&mut self.picker),
            "text" => Some(&mut self.text),
            "record" => Some(&mut self.record),
            _ => None,
        }
    }
}

/// How a hotkey ended up after the tray process tried to register it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HotkeyState {
    Active,
    /// Taken over from another app with the keyboard hook.
    Overriding,
    /// Another app (or Windows) has registered this combination.
    InUse,
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HotkeyStatusEntry {
    pub id: String,
    pub hotkey: Hotkey,
    pub state: HotkeyState,
}

/// Written by the tray process after every (re)registration, read by the settings window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyStatus {
    pub entries: Vec<HotkeyStatusEntry>,
}

impl HotkeyStatus {
    pub fn path() -> PathBuf {
        config_dir().join("hotkey_status.json")
    }
    pub fn load() -> Option<Self> {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok())
    }
    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            std::fs::create_dir_all(config_dir()).ok();
            std::fs::write(Self::path(), json).ok();
        }
    }
    pub fn get(&self, id: &str) -> Option<&HotkeyStatusEntry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    Png,
    Jpg,
    /// Lossless WebP.
    Webp,
}

impl ImageFormat {
    pub const ALL: [ImageFormat; 3] = [ImageFormat::Png, ImageFormat::Jpg, ImageFormat::Webp];

    pub fn ext(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpg => "jpg",
            ImageFormat::Webp => "webp",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpg => "JPG",
            ImageFormat::Webp => "WebP",
        }
    }
}

/// What a screen recording is saved as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoFormat {
    #[default]
    Mp4,
    Gif,
}

impl VideoFormat {
    pub const ALL: [VideoFormat; 2] = [VideoFormat::Mp4, VideoFormat::Gif];

    pub fn ext(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "mp4",
            VideoFormat::Gif => "gif",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "MP4",
            VideoFormat::Gif => "GIF",
        }
    }
}

/// Settings > Recording.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recording {
    /// What a recording is saved as when you stop.
    pub format: VideoFormat,
    /// Frames per second: 15 or 30.
    pub fps: u32,
    /// GIFs wider than this are scaled down.
    pub gif_max_width: u32,
    /// A soft circle around the pointer.
    pub pointer_highlight: bool,
    /// A ring where you click.
    pub click_highlight: bool,
    /// Which keys and clicks to show (and log). Off by default: privacy.
    pub keys: crate::rec::keys::KeyMode,
    /// Count down 3-2-1 before recording starts.
    pub countdown: bool,
    /// Record every pixel of a sharp screen (200% and up). Off: half size there, which looks
    /// like the same screen at 100%, with files four times smaller and a quicker save.
    pub full_resolution: bool,
    /// Where recordings go.
    pub output_dir: PathBuf,
}

/// Frames per second a GIF gets.
pub const GIF_FPS: u32 = 12;

impl Default for Recording {
    fn default() -> Self {
        let videos = dirs::video_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
        Self {
            format: VideoFormat::Mp4,
            fps: 30,
            gif_max_width: 960,
            pointer_highlight: false,
            click_highlight: true,
            keys: crate::rec::keys::KeyMode::Off,
            countdown: true,
            full_resolution: false,
            output_dir: videos.join("Blink"),
        }
    }
}

/// Light or dark windows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Follow the Windows app mode setting.
    System,
    #[default]
    Dark,
    Light,
}

/// How sharp the editor's preview of a background / mockup is. Copy and Save always use the
/// full resolution; this only changes what the editor draws on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewQuality {
    /// Half the screen resolution: the fastest, a little soft.
    Fast,
    /// Screen resolution: sharp on screen, and fast.
    #[default]
    Balanced,
    /// The full output resolution (up to the graphics card's limit): the slowest.
    Full,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkeys: Hotkeys,
    /// Show a small menu at the mouse after each capture to choose what to do with it.
    pub after_capture_ask: bool,
    pub after_capture_open_editor: bool,
    pub after_capture_copy: bool,
    pub after_capture_save: bool,
    /// Pin the screenshot to the screen (a small window that stays on top).
    pub after_capture_pin: bool,
    pub output_dir: PathBuf,
    /// chrono strftime pattern, without extension.
    pub filename_pattern: String,
    pub format: ImageFormat,
    pub jpg_quality: u8,
    pub start_with_windows: bool,
    pub show_magnifier: bool,
    pub close_editor_after_copy: bool,
    pub close_editor_after_save: bool,
    /// Milliseconds to wait for a page to settle after each scroll step.
    pub scroll_delay_ms: u32,
    /// Maximum height (px) of a scrolling capture.
    pub scroll_max_height: u32,
    /// Look for a new version on GitHub once a day.
    pub check_updates: bool,
    /// A version the user chose to skip (no reminders for it).
    pub skipped_version: String,
    /// Version that ran last, to say "updated" once after an update.
    pub last_version: String,
    /// When Blink first ran as `last_version` after an update (Unix seconds, 0 if it was not
    /// an update): the tray menu offers "See what's new" for a day after that.
    pub updated_at: i64,
    pub language: crate::i18n::Language,
    pub theme: ThemeChoice,
    pub preview_quality: PreviewQuality,
    /// Esc in the editor asks before closing it.
    pub confirm_escape_close: bool,
    /// Text labels under the editor's tool icons.
    pub show_tool_labels: bool,
    /// How many screenshots the tray menu's History keeps (0 = off).
    pub history_size: u32,
    /// How long History keeps a screenshot, in hours (0 = no limit).
    pub history_max_age_hours: u32,
    /// Capture the mouse pointer too (a separate, movable object in the editor).
    pub capture_pointer: bool,
    /// Experimental: tone map monitors that have HDR on instead of clipping their highlights
    /// (`platform/win/hdr.rs`). Off by default.
    pub hdr_screenshots: bool,
    /// Screen recording.
    pub recording: Recording,
}

impl Default for Config {
    fn default() -> Self {
        let pictures = dirs::picture_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
        Self {
            hotkeys: Hotkeys::default(),
            after_capture_ask: false,
            after_capture_open_editor: true,
            after_capture_copy: false,
            after_capture_save: false,
            after_capture_pin: false,
            output_dir: pictures.join("Blink"),
            filename_pattern: "Blink_%Y-%m-%d_%H-%M-%S".into(),
            format: ImageFormat::Png,
            jpg_quality: 90,
            start_with_windows: true,
            show_magnifier: true,
            close_editor_after_copy: false,
            close_editor_after_save: false,
            scroll_delay_ms: 120,
            scroll_max_height: 30000,
            check_updates: true,
            skipped_version: String::new(),
            last_version: String::new(),
            updated_at: 0,
            language: crate::i18n::Language::English,
            theme: ThemeChoice::Dark,
            preview_quality: PreviewQuality::Balanced,
            confirm_escape_close: true,
            show_tool_labels: false,
            history_size: 10,
            history_max_age_hours: 24,
            capture_pointer: false,
            hdr_screenshots: false,
            recording: Recording::default(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("Blink")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

impl Config {
    pub fn load() -> Self {
        std::fs::read_to_string(config_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Whether the windows use the light variant.
    pub fn light_theme(&self) -> bool {
        match self.theme {
            ThemeChoice::Dark => false,
            ThemeChoice::Light => true,
            ThemeChoice::System => crate::platform::system_uses_light_theme(),
        }
    }

    /// Applies the language and theme to this process.
    pub fn apply_globals(&self) {
        crate::i18n::set_language(self.language);
        crate::theme::set_light(self.light_theme());
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir())?;
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = config_path().with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, config_path())
    }

    /// Next free file path in the output folder according to the filename pattern.
    pub fn next_output_path(&self) -> PathBuf {
        next_free_path(&self.output_dir, &self.filename_pattern, self.format.ext())
    }
}

/// `<dir>/<pattern now>.<ext>`, or with " (2)", " (3)", ... when that exists.
pub fn next_free_path(dir: &std::path::Path, pattern: &str, ext: &str) -> PathBuf {
    // An invalid pattern would make chrono panic while formatting: fall back to the default.
    let mut stem = String::new();
    {
        use std::fmt::Write;
        if write!(stem, "{}", chrono::Local::now().format(pattern)).is_err() {
            stem = chrono::Local::now().format("Blink_%Y-%m-%d_%H-%M-%S").to_string();
        }
    }
    let stem = sanitize_filename(&stem);
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    path
}

fn sanitize_filename(s: &str) -> String {
    let cleaned: String =
        s.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    if cleaned.trim().is_empty() {
        "screenshot".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let c = Config::default();
        let json = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hotkeys.region, c.hotkeys.region);
        // Missing fields fall back to defaults.
        let partial: Config = serde_json::from_str(r#"{"jpg_quality": 70}"#).unwrap();
        assert_eq!(partial.jpg_quality, 70);
        assert!(partial.after_capture_open_editor && !partial.after_capture_ask);
        // Hotkeys saved before `force` existed still load.
        let old: Hotkey =
            serde_json::from_str(r#"{"ctrl":true,"alt":false,"shift":false,"win":false,"vk":44}"#).unwrap();
        assert!(!old.force && old.same_combo(&Hotkeys::default().picker));
        // Settings saved before recording existed get the defaults.
        let before: Config = serde_json::from_str(
            r#"{"hotkeys":{"region":{"ctrl":false,"alt":false,"shift":false,"win":false,"vk":44}}}"#,
        )
        .unwrap();
        assert_eq!(before.recording, Recording::default());
        assert_eq!(before.hotkeys.record, Hotkeys::default().record);
        assert_eq!(before.recording.keys, crate::rec::keys::KeyMode::Off, "keys are off unless you switch them on");
    }

    #[test]
    fn the_record_hotkey_clashes_with_nothing() {
        let h = Hotkeys::default();
        let list = h.list();
        for (i, a) in list.iter().enumerate() {
            for b in &list[i + 1..] {
                assert!(!a.2.same_combo(&b.2), "{} and {}", a.0, b.0);
            }
        }
        // Windows: Left Alt + Left Shift + Print Screen is High Contrast, Win + Shift + R the
        // Snipping Tool, Win + Alt + R the Game Bar.
        let taken = [Hotkey::new(false, true, true, VK_SNAPSHOT)];
        assert!(taken.iter().all(|t| !t.same_combo(&h.record)));
        assert_eq!(h.record.label(), "Ctrl + Alt + Shift + Print Screen");
    }

    #[test]
    fn free_paths_count_up() {
        let dir = std::env::temp_dir().join(format!("blink-paths-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = next_free_path(&dir, "rec", "mp4");
        assert_eq!(a, dir.join("rec.mp4"));
        std::fs::write(&a, b"x").unwrap();
        assert_eq!(next_free_path(&dir, "rec", "mp4"), dir.join("rec (2).mp4"));
        // A broken pattern does not panic.
        let p = next_free_path(&dir, "%Q%", "gif");
        assert!(p.extension().is_some_and(|e| e == "gif"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn labels() {
        assert_eq!(Hotkeys::default().scrolling.label(), "Ctrl + Shift + Print Screen");
        assert_eq!(Hotkey::none().label(), "(none)");
        assert_eq!(sanitize_filename("a:b/c"), "a_b_c");
    }
}
