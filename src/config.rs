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
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            region: Hotkey::new(false, false, false, VK_SNAPSHOT),
            fullscreen: Hotkey::new(false, false, true, VK_SNAPSHOT),
            active_window: Hotkey::new(false, true, false, VK_SNAPSHOT),
            scrolling: Hotkey::new(true, false, true, VK_SNAPSHOT),
            picker: Hotkey::new(true, false, false, VK_SNAPSHOT),
        }
    }
}

impl Hotkeys {
    /// (stable id, display name, hotkey) for every capture hotkey.
    pub fn list(&self) -> [(&'static str, &'static str, Hotkey); 5] {
        [
            ("region", crate::i18n::t("Region / window"), self.region),
            ("fullscreen", crate::i18n::t("Full screen"), self.fullscreen),
            ("active_window", crate::i18n::t("Active window"), self.active_window),
            ("scrolling", crate::i18n::t("Scrolling page"), self.scrolling),
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
        let stem = chrono::Local::now().format(&self.filename_pattern).to_string();
        let stem = sanitize_filename(&stem);
        let ext = self.format.ext();
        let mut path = self.output_dir.join(format!("{stem}.{ext}"));
        let mut n = 2;
        while path.exists() {
            path = self.output_dir.join(format!("{stem} ({n}).{ext}"));
            n += 1;
        }
        path
    }
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
    }

    #[test]
    fn labels() {
        assert_eq!(Hotkeys::default().scrolling.label(), "Ctrl + Shift + Print Screen");
        assert_eq!(Hotkey::none().label(), "(none)");
        assert_eq!(sanitize_filename("a:b/c"), "a_b_c");
    }
}
