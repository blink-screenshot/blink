//! Interface language: English (the default) and Dutch.
//!
//! The English text is the key: `t("Copy")` returns the text in the current language, and
//! falls back to the English when a translation is missing. Placeholders are `{0}`, `{1}`,
//! ... and filled in with [`tf`]. The Dutch table is in `i18n_nl.rs`; a test checks that every
//! `t("...")` / `tf("...")` in the sources has a Dutch entry.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    English,
    Dutch,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::English, Language::Dutch];
    /// The language's own name (shown untranslated, so you can always find yours).
    pub fn native_name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Dutch => "Nederlands",
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_language(l: Language) {
    LANG.store(l as u8, Ordering::Relaxed);
}

pub fn language() -> Language {
    match LANG.load(Ordering::Relaxed) {
        1 => Language::Dutch,
        _ => Language::English,
    }
}

fn dutch() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| crate::i18n_nl::NL.iter().copied().collect())
}

/// `en` in the current language.
pub fn t(en: &'static str) -> &'static str {
    match language() {
        Language::English => en,
        Language::Dutch => dutch().get(en).copied().unwrap_or(en),
    }
}

/// `en` in the current language with `{0}`, `{1}`, ... replaced by `args`.
pub fn tf(en: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    let mut s = t(en).to_string();
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), &a.to_string());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCES: &[(&str, &str)] = &[
        ("main.rs", include_str!("main.rs")),
        ("config.rs", include_str!("config.rs")),
        ("frame.rs", include_str!("frame.rs")),
        ("annot.rs", include_str!("annot.rs")),
        ("imageio.rs", include_str!("imageio.rs")),
        ("settings_ui.rs", include_str!("settings_ui.rs")),
        ("update_ui.rs", include_str!("update_ui.rs")),
        ("welcome_ui.rs", include_str!("welcome_ui.rs")),
        ("update.rs", include_str!("update.rs")),
        ("editor/mod.rs", include_str!("editor/mod.rs")),
        ("editor/ui.rs", include_str!("editor/ui.rs")),
        ("editor/canvas.rs", include_str!("editor/canvas.rs")),
        ("editor/frame_ui.rs", include_str!("editor/frame_ui.rs")),
        ("editor/textsel.rs", include_str!("editor/textsel.rs")),
        ("emoji.rs", include_str!("emoji.rs")),
        ("ocr.rs", include_str!("ocr.rs")),
        ("sensitive.rs", include_str!("sensitive.rs")),
        ("capture_bar.rs", include_str!("capture_bar.rs")),
        ("render.rs", include_str!("render.rs")),
        ("platform/win/mod.rs", include_str!("platform/win/mod.rs")),
        ("platform/win/http.rs", include_str!("platform/win/http.rs")),
        ("platform/win/resident.rs", include_str!("platform/win/resident.rs")),
        ("platform/win/overlay.rs", include_str!("platform/win/overlay.rs")),
    ];

    /// Every string literal passed directly to t( or tf( in `src`.
    fn keys(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        for pat in ["t(\"", "tf(\""] {
            let mut rest = src;
            while let Some(i) = rest.find(pat) {
                let before = rest[..i].chars().last();
                rest = &rest[i + pat.len()..];
                // Only the functions themselves, not e.g. `format(`.
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let mut s = String::new();
                let mut chars = rest.chars();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('"') => s.push('"'),
                            Some('\\') => s.push('\\'),
                            Some('u') => {
                                let code: String = chars.by_ref().skip(1).take_while(|c| *c != '}').collect();
                                s.push(char::from_u32(u32::from_str_radix(&code, 16).unwrap()).unwrap());
                            }
                            // Line continuation (LF, or CRLF in a Windows checkout): skip the
                            // line end and the leading whitespace, as the compiler does.
                            Some('\n' | '\r') => {
                                let rest: String = chars.clone().collect();
                                let skip = rest.len() - rest.trim_start().len();
                                for _ in 0..rest[..skip].chars().count() {
                                    chars.next();
                                }
                            }
                            Some(o) => s.push(o),
                            None => {}
                        },
                        '"' => break,
                        c => s.push(c),
                    }
                }
                out.push(s);
            }
        }
        out
    }

    #[test]
    fn every_text_has_a_dutch_translation() {
        let mut missing = Vec::new();
        for (file, src) in SOURCES {
            for k in keys(src) {
                if !dutch().contains_key(k.as_str()) {
                    missing.push(format!("{file}: {k:?}"));
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "missing Dutch:\n{}", missing.join("\n"));
    }

    /// Names that are translated where they are shown, with `t(name)`.
    #[test]
    fn every_name_has_a_dutch_translation() {
        let mut names: Vec<&str> = Vec::new();
        names.extend(crate::annot::ShapeKind::ALL.iter().map(|k| k.name()));
        names.extend(crate::annot::ArrowHead::ALL.iter().map(|k| k.name()));
        names.extend(crate::frame::Aspect::ALL.iter().chain(&crate::frame::Aspect::SIZES).map(|a| a.name()));
        names.extend(crate::frame::PRESETS.iter().map(|p| p.name));
        names.extend(crate::frame::DEVICES.iter().map(|d| d.group.name()));
        let missing: Vec<_> = names.into_iter().filter(|n| !n.contains(':') && !dutch().contains_key(n)).collect();
        assert!(missing.is_empty(), "missing Dutch: {missing:?}");
    }

    #[test]
    fn no_duplicate_keys_and_placeholders_match() {
        let mut seen = std::collections::HashSet::new();
        for (en, nl) in crate::i18n_nl::NL {
            assert!(seen.insert(en), "duplicate key {en:?}");
            for i in 0..5 {
                let p = format!("{{{i}}}");
                assert_eq!(en.contains(&p), nl.contains(&p), "placeholder {p} in {en:?}");
            }
        }
    }

    #[test]
    fn fallback_and_format() {
        // (Does not switch the process language: other tests run at the same time.)
        assert!(!dutch().contains_key("this text has no translation"));
        assert_eq!(tf("{0} of {1}", &[&1, &"x"]), "1 of x");
    }
}
