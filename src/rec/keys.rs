//! Which keys a recording shows, and how they are labelled.
//!
//! Privacy comes first. "Show keys and clicks" is off by default. When it is on, the default
//! is *shortcuts and clicks only*: combinations with Ctrl, Alt or Win, the function and
//! navigation keys on their own (F1-F24, Esc, Enter, Tab, the arrows, Delete, ...), and clicks
//! with a modifier (Ctrl+click). Shift with a letter, digit or punctuation mark is typing, not
//! a shortcut, and is never shown: it would put text and passwords in the video. AltGr is
//! Ctrl+Alt to Windows, but AltGr+2 types "@" on many layouts: that is typing too.
//!
//! Only *all keys* shows ordinary text. The filter runs before anything is written to the
//! event log, so in the other modes typed text never reaches the disk.

use super::events::{Button, Mods};
use crate::i18n::t;
use serde::{Deserialize, Serialize};

/// Which keys and clicks a recording shows (Settings > Recording).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyMode {
    /// No keys at all (the keyboard is not even watched).
    #[default]
    Off,
    /// Shortcuts, function and navigation keys, and clicks with a modifier.
    Shortcuts,
    /// Every key, ordinary text included.
    All,
}

impl KeyMode {
    pub const ALL: [KeyMode; 3] = [KeyMode::Off, KeyMode::Shortcuts, KeyMode::All];

    pub fn label(self) -> &'static str {
        match self {
            KeyMode::Off => t("Off"),
            KeyMode::Shortcuts => t("Shortcuts and clicks only (recommended)"),
            KeyMode::All => t("All keys and clicks"),
        }
    }
}

/// A key press as the keyboard hook sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyPress {
    /// Windows virtual-key code.
    pub vk: u32,
    /// Modifiers held down (for AltGr: Ctrl and Alt, as Windows reports it).
    pub mods: Mods,
    /// AltGr is down (right Alt on a layout that has AltGr).
    pub altgr: bool,
    /// The character printed on the key, without Shift (`MapVirtualKeyEx`, no side effects),
    /// to name punctuation keys in a combination ("Ctrl + ,").
    pub key_char: Option<char>,
    /// The text the key types with the current modifiers and layout. Only looked up in
    /// "all keys" mode; ignored in every other mode.
    pub typed: Option<String>,
}

/// What to show for a key press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// A key or combination, shown as a label of its own ("Ctrl + C", "F5").
    Combo(String),
    /// Ordinary text, joined with the text typed just before it.
    Typed(String),
}

fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | 0x5B | 0x5C | 0x14 | 0x90 | 0x91)
}

/// Keys that do something on their own instead of typing a character.
fn is_function_or_navigation(vk: u32) -> bool {
    matches!(
        vk,
        0x09 // Tab
        | 0x0D // Enter
        | 0x13 // Pause
        | 0x1B // Esc
        | 0x21..=0x28 // Page Up, Page Down, End, Home, arrows
        | 0x2C..=0x2E // Print Screen, Insert, Delete
        | 0x5D // Menu
        | 0x70..=0x87 // F1-F24
    )
}

/// The name on the key.
pub fn key_name(vk: u32, key_char: Option<char>) -> String {
    let fixed = match vk {
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0D => "Enter",
        0x13 => "Pause",
        0x1B => "Esc",
        0x20 => "Space",
        0x21 => "Page Up",
        0x22 => "Page Down",
        0x23 => "End",
        0x24 => "Home",
        0x25 => "\u{2190}",
        0x26 => "\u{2191}",
        0x27 => "\u{2192}",
        0x28 => "\u{2193}",
        0x2C => "Print Screen",
        0x2D => "Insert",
        0x2E => "Delete",
        0x5D => "Menu",
        0x6A => "Num *",
        0x6B => "Num +",
        0x6D => "Num -",
        0x6E => "Num .",
        0x6F => "Num /",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_string();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(String::from).unwrap_or_default(),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => match key_char.filter(|c| !c.is_control() && !c.is_whitespace()) {
            Some(c) => c.to_uppercase().collect(),
            None => format!("#{vk}"),
        },
    }
}

fn combo(mods: Mods, name: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if mods.ctrl {
        parts.push("Ctrl");
    }
    if mods.alt {
        parts.push("Alt");
    }
    if mods.shift {
        parts.push("Shift");
    }
    if mods.win {
        parts.push("Win");
    }
    parts.push(name);
    parts.join(" + ")
}

/// What to show for `key` in `mode`, or nothing.
pub fn filter(mode: KeyMode, key: &KeyPress) -> Option<Shown> {
    if mode == KeyMode::Off || is_modifier(key.vk) {
        return None;
    }
    let shortcut = !key.altgr && (key.mods.ctrl || key.mods.alt || key.mods.win);
    if shortcut || is_function_or_navigation(key.vk) {
        // With AltGr held, a function key still counts, but without the fake Ctrl + Alt.
        let mods = if key.altgr { Mods { ctrl: false, alt: false, ..key.mods } } else { key.mods };
        return Some(Shown::Combo(combo(mods, &key_name(key.vk, key.key_char))));
    }
    // Everything else types: letters, digits, punctuation, Space, AltGr characters, with or
    // without Shift.
    if mode != KeyMode::All {
        return None;
    }
    if key.vk == 0x08 {
        return Some(Shown::Combo(key_name(key.vk, None)));
    }
    let text = key.typed.as_deref().unwrap_or("");
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    (!text.is_empty()).then_some(Shown::Typed(text))
}

/// The label for a click: only for clicks with a modifier ("Ctrl + Click"), in both modes
/// that show keys. A plain click shows as a click highlight, not as text.
pub fn click_label(mode: KeyMode, button: Button, mods: Mods) -> Option<String> {
    if mode == KeyMode::Off || !mods.any() {
        return None;
    }
    let name = match button {
        Button::Left => t("Click"),
        Button::Right => t("Right-click"),
        Button::Middle => t("Middle-click"),
    };
    Some(combo(mods, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: Mods = Mods { ctrl: true, alt: false, shift: false, win: false };
    const SHIFT: Mods = Mods { ctrl: false, alt: false, shift: true, win: false };
    const NONE: Mods = Mods { ctrl: false, alt: false, shift: false, win: false };

    fn key(vk: u32, mods: Mods) -> KeyPress {
        KeyPress { vk, mods, ..KeyPress::default() }
    }

    fn typed(vk: u32, mods: Mods, text: &str) -> KeyPress {
        KeyPress { vk, mods, typed: Some(text.into()), ..KeyPress::default() }
    }

    fn altgr(vk: u32, text: &str) -> KeyPress {
        KeyPress {
            vk,
            mods: Mods { ctrl: true, alt: true, ..NONE },
            altgr: true,
            typed: Some(text.into()),
            ..KeyPress::default()
        }
    }

    fn shows(mode: KeyMode, k: &KeyPress) -> Option<String> {
        filter(mode, k).map(|s| match s {
            Shown::Combo(c) => c,
            Shown::Typed(t) => format!("typed:{t}"),
        })
    }

    #[test]
    fn shortcut_mode_shows_shortcuts() {
        let m = KeyMode::Shortcuts;
        assert_eq!(shows(m, &key(0x43, CTRL)).as_deref(), Some("Ctrl + C"));
        assert_eq!(shows(m, &key(0x54, Mods { shift: true, ..CTRL })).as_deref(), Some("Ctrl + Shift + T"));
        assert_eq!(shows(m, &key(0x09, Mods { alt: true, ..NONE })).as_deref(), Some("Alt + Tab"));
        assert_eq!(shows(m, &key(0x45, Mods { win: true, ..NONE })).as_deref(), Some("Win + E"));
        assert_eq!(shows(m, &key(0x74, NONE)).as_deref(), Some("F5"));
        assert_eq!(shows(m, &key(0x87, NONE)).as_deref(), Some("F24"));
        assert_eq!(shows(m, &key(0x1B, NONE)).as_deref(), Some("Esc"));
        assert_eq!(shows(m, &key(0x0D, NONE)).as_deref(), Some("Enter"));
        assert_eq!(shows(m, &key(0x2E, NONE)).as_deref(), Some("Delete"));
        assert_eq!(shows(m, &key(0x25, NONE)).as_deref(), Some("\u{2190}"));
        // Shift with a navigation key is a shortcut too.
        assert_eq!(shows(m, &key(0x09, SHIFT)).as_deref(), Some("Shift + Tab"));
        assert_eq!(shows(m, &key(0x28, SHIFT)).as_deref(), Some("Shift + \u{2193}"));
        // Punctuation keys are named by the character on them.
        let comma = KeyPress { key_char: Some(','), ..key(0xBC, CTRL) };
        assert_eq!(shows(m, &comma).as_deref(), Some("Ctrl + ,"));
    }

    #[test]
    fn shortcut_mode_never_shows_typing() {
        let m = KeyMode::Shortcuts;
        // Letters, digits and punctuation, with and without Shift: typing.
        for vk in [0x41, 0x5A, 0x30, 0x39, 0xBC, 0xBA, 0xDE, 0x20, 0x08, 0x6B, 0x60] {
            assert_eq!(shows(m, &typed(vk, NONE, "x")), None, "vk {vk:#x}");
            assert_eq!(shows(m, &typed(vk, SHIFT, "X")), None, "Shift + vk {vk:#x}");
        }
        // AltGr is Ctrl + Alt to Windows, but AltGr+2 = "@" and AltGr+E = "€" are typing.
        assert_eq!(shows(m, &altgr(0x32, "@")), None);
        assert_eq!(shows(m, &altgr(0x45, "\u{20ac}")), None);
        // Modifiers on their own show nothing.
        for vk in [0x10, 0x11, 0x12, 0xA0, 0xA3, 0xA5, 0x5B, 0x14] {
            assert_eq!(shows(m, &key(vk, CTRL)), None);
        }
        // Even when the hook looked up the text, this mode ignores it.
        assert_eq!(shows(m, &typed(0x41, NONE, "a")), None);
    }

    #[test]
    fn off_shows_nothing() {
        assert_eq!(shows(KeyMode::Off, &key(0x43, CTRL)), None);
        assert_eq!(shows(KeyMode::Off, &key(0x74, NONE)), None);
        assert_eq!(click_label(KeyMode::Off, Button::Left, CTRL), None);
    }

    #[test]
    fn all_mode_shows_text_too() {
        let m = KeyMode::All;
        assert_eq!(shows(m, &typed(0x41, NONE, "a")).as_deref(), Some("typed:a"));
        assert_eq!(shows(m, &typed(0x41, SHIFT, "A")).as_deref(), Some("typed:A"));
        assert_eq!(shows(m, &typed(0x31, SHIFT, "!")).as_deref(), Some("typed:!"));
        assert_eq!(shows(m, &typed(0x20, NONE, " ")).as_deref(), Some("typed: "));
        assert_eq!(shows(m, &altgr(0x32, "@")).as_deref(), Some("typed:@"));
        assert_eq!(shows(m, &key(0x08, NONE)).as_deref(), Some("Backspace"));
        // Shortcuts stay shortcuts.
        assert_eq!(shows(m, &typed(0x43, CTRL, "\u{3}")).as_deref(), Some("Ctrl + C"));
        // A dead key types nothing (yet).
        assert_eq!(shows(m, &typed(0xDE, NONE, "")), None);
    }

    #[test]
    fn clicks_with_a_modifier_are_labelled() {
        let m = KeyMode::Shortcuts;
        assert_eq!(click_label(m, Button::Left, CTRL).as_deref(), Some("Ctrl + Click"));
        assert_eq!(click_label(m, Button::Left, SHIFT).as_deref(), Some("Shift + Click"));
        assert_eq!(
            click_label(KeyMode::All, Button::Right, Mods { alt: true, ..NONE }).as_deref(),
            Some("Alt + Right-click")
        );
        // A plain click is a highlight, not text.
        assert_eq!(click_label(m, Button::Left, NONE), None);
    }

    #[test]
    fn key_names() {
        assert_eq!(key_name(0x41, None), "A");
        assert_eq!(key_name(0x37, None), "7");
        assert_eq!(key_name(0x63, None), "Num 3");
        assert_eq!(key_name(0x70, None), "F1");
        assert_eq!(key_name(0xBF, Some('/')), "/");
        assert_eq!(key_name(0xDB, Some('\u{fc}')), "\u{dc}");
        assert_eq!(key_name(0xFF, None), "#255");
    }
}
