//! Interface language: English (the default), Dutch, German, French, Spanish and Portuguese.
//!
//! The English text is the key: `t("Copy")` returns the text in the current language, and
//! falls back to the English when a translation is missing. Placeholders are `{0}`, `{1}`,
//! ... and filled in with [`tf`]. The tables are in `i18n_nl.rs`, `i18n_de.rs`, ...; tests check
//! that every `t("...")` / `tf("...")` in the sources has an entry in every table.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// Stored in the settings by name: only add variants at the end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    English,
    Dutch,
    German,
    French,
    Spanish,
    Portuguese,
}

type Table = &'static [(&'static str, &'static str)];

impl Language {
    pub const ALL: [Language; 6] = [
        Language::English,
        Language::Dutch,
        Language::German,
        Language::French,
        Language::Spanish,
        Language::Portuguese,
    ];
    /// The language's own name (shown untranslated, so you can always find yours).
    pub fn native_name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Dutch => "Nederlands",
            Language::German => "Deutsch",
            Language::French => "Fran\u{e7}ais",
            Language::Spanish => "Espa\u{f1}ol",
            Language::Portuguese => "Portugu\u{ea}s (Brasil)",
        }
    }
    fn table(self) -> Option<Table> {
        match self {
            Language::English => None,
            Language::Dutch => Some(crate::i18n_nl::NL),
            Language::German => Some(crate::i18n_de::DE),
            Language::French => Some(crate::i18n_fr::FR),
            Language::Spanish => Some(crate::i18n_es::ES),
            Language::Portuguese => Some(crate::i18n_pt::PT),
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_language(l: Language) {
    LANG.store(l as u8, Ordering::Relaxed);
}

pub fn language() -> Language {
    let i = LANG.load(Ordering::Relaxed) as usize;
    Language::ALL.get(i).copied().unwrap_or_default()
}

fn map(l: Language) -> Option<&'static HashMap<&'static str, &'static str>> {
    static MAPS: OnceLock<Vec<HashMap<&'static str, &'static str>>> = OnceLock::new();
    let maps = MAPS.get_or_init(|| {
        Language::ALL.iter().map(|l| l.table().map(|t| t.iter().copied().collect()).unwrap_or_default()).collect()
    });
    l.table().map(|_| &maps[l as usize])
}

/// `en` in language `l`.
pub fn t_in(l: Language, en: &'static str) -> &'static str {
    map(l).and_then(|m| m.get(en).copied()).unwrap_or(en)
}

/// `en` in the current language.
pub fn t(en: &'static str) -> &'static str {
    t_in(language(), en)
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
        ("history.rs", include_str!("history.rs")),
        ("clip_ui.rs", include_str!("clip_ui.rs")),
        ("pin_ui.rs", include_str!("pin_ui.rs")),
        ("table.rs", include_str!("table.rs")),
        ("rec/mod.rs", include_str!("rec/mod.rs")),
        ("rec/bar.rs", include_str!("rec/bar.rs")),
        ("rec/keys.rs", include_str!("rec/keys.rs")),
        ("rec/export.rs", include_str!("rec/export.rs")),
        ("platform/win/rec/mod.rs", include_str!("platform/win/rec/mod.rs")),
        ("platform/win/rec/mf.rs", include_str!("platform/win/rec/mf.rs")),
        ("platform/win/rec/recorder.rs", include_str!("platform/win/rec/recorder.rs")),
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

    fn translated() -> impl Iterator<Item = (Language, &'static HashMap<&'static str, &'static str>)> {
        Language::ALL.into_iter().filter_map(|l| map(l).map(|m| (l, m)))
    }

    #[test]
    fn every_text_has_a_translation() {
        let mut missing = Vec::new();
        for (l, m) in translated() {
            for (file, src) in SOURCES {
                for k in keys(src) {
                    if !m.contains_key(k.as_str()) {
                        missing.push(format!("{l:?} {file}: {k:?}"));
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "missing translations:\n{}", missing.join("\n"));
    }

    /// Every table has the same keys as the Dutch one (no forgotten or stale entries).
    #[test]
    fn all_tables_have_the_same_keys() {
        let nl: std::collections::BTreeSet<&str> = crate::i18n_nl::NL.iter().map(|(k, _)| *k).collect();
        for l in Language::ALL {
            if let Some(t) = l.table() {
                let keys: std::collections::BTreeSet<&str> = t.iter().map(|(k, _)| *k).collect();
                let extra: Vec<_> = keys.difference(&nl).collect();
                let missing: Vec<_> = nl.difference(&keys).collect();
                assert!(extra.is_empty() && missing.is_empty(), "{l:?}: extra {extra:?}, missing {missing:?}");
            }
        }
    }

    /// Names that are translated where they are shown, with `t(name)`.
    #[test]
    fn every_name_has_a_translation() {
        let mut names: Vec<&str> = Vec::new();
        names.extend(crate::annot::ShapeKind::ALL.iter().map(|k| k.name()));
        names.extend(crate::annot::ArrowHead::ALL.iter().map(|k| k.name()));
        names.extend(crate::frame::Aspect::ALL.iter().chain(&crate::frame::Aspect::SIZES).map(|a| a.name()));
        names.extend(crate::frame::PRESETS.iter().map(|p| p.name));
        names.extend(crate::frame::DEVICES.iter().map(|d| d.group.name()));
        for (l, m) in translated() {
            let missing: Vec<_> = names.iter().filter(|n| !n.contains(':') && !m.contains_key(*n)).collect();
            assert!(missing.is_empty(), "missing {l:?}: {missing:?}");
        }
    }

    #[test]
    fn no_duplicate_keys_and_placeholders_match() {
        for l in Language::ALL {
            let Some(table) = l.table() else { continue };
            let mut seen = std::collections::HashSet::new();
            for (en, tr) in table {
                assert!(seen.insert(en), "{l:?}: duplicate key {en:?}");
                for i in 0..5 {
                    let p = format!("{{{i}}}");
                    assert_eq!(en.contains(&p), tr.contains(&p), "{l:?}: placeholder {p} in {en:?}");
                }
                // A menu accelerator (&x) stays a single accelerator.
                if en.contains('&') && !en.contains(" & ") {
                    assert_eq!(tr.matches('&').count(), 1, "{l:?}: accelerator in {en:?}");
                }
                assert_eq!(en.contains('\t'), tr.contains('\t'), "{l:?}: tab in {en:?}");
            }
        }
    }

    #[test]
    fn fallback_and_format() {
        // (Does not switch the process language: other tests run at the same time.)
        assert_eq!(t_in(Language::German, "this text has no translation"), "this text has no translation");
        assert_eq!(t_in(Language::Dutch, "Copy"), "Kopi\u{eb}ren");
        assert_eq!(tf("{0} of {1}", &[&1, &"x"]), "1 of x");
    }
}
