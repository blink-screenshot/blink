//! File names, and folders, of saved screenshots and recordings, from the pattern in
//! Settings > Saving.
//!
//! A pattern is plain text with variables in braces: `Blink_{date}_{time}`. A backslash (or
//! slash) makes a folder, so `{year}-{month}\{app}_{time}` saves each month in a folder of its
//! own, made when the first screenshot goes in. Old patterns with `%Y`-style codes still work.
//!
//! Everything is worked out at the moment of saving, from what is known about that one
//! capture ([`Meta`]): nothing is kept in memory, and the folder is only read when the pattern
//! counts (`{n}`).

use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};

/// The pattern of new installs: the same names as before variables existed.
pub const DEFAULT_PATTERN: &str = "Blink_{date}_{time}";
/// The default before variables existed; read as [`DEFAULT_PATTERN`].
pub const OLD_DEFAULT_PATTERN: &str = "Blink_%Y-%m-%d_%H-%M-%S";

/// What is known about a capture when it is saved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// The program that was captured, by its file name without `.exe` ("chrome", "WINWORD").
    pub app: Option<String>,
    /// How it was captured: "region", "window", "screen", "screens", "scroll", "recording".
    pub mode: Option<String>,
    pub width: u32,
    pub height: u32,
}

impl Meta {
    pub fn sized(&self, width: u32, height: u32) -> Meta {
        Meta { width, height, ..self.clone() }
    }

    /// As command-line arguments for another Blink process (`--app chrome --mode window`).
    pub fn to_args(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(a) = &self.app {
            out.extend(["--app".to_string(), a.clone()]);
        }
        if let Some(m) = &self.mode {
            out.extend(["--mode".to_string(), m.clone()]);
        }
        out
    }

    /// Reads what [`Meta::to_args`] wrote, anywhere in `args`.
    pub fn from_args(args: &[String]) -> Meta {
        let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
        Meta { app: after("--app"), mode: after("--mode"), ..Meta::default() }
    }
}

/// The variables, in the order Settings offers them.
pub const VARIABLES: [&str; 15] = [
    "{date}", "{time}", "{year}", "{month}", "{day}", "{hour}", "{minute}", "{second}", "{app}", "{mode}", "{width}",
    "{height}", "{user}", "{pc}", "{n}",
];

/// A part of one folder or file name: text, or the place of the counter (`{n}`, `{n:3}`).
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text(String),
    Counter(usize),
}

fn env(names: &[&str]) -> String {
    names.iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty())).unwrap_or_default()
}

/// The value of a variable (without braces, lower case), or `None` for an unknown one.
fn value(name: &str, meta: &Meta, now: &DateTime<Local>) -> Option<String> {
    let f = |fmt: &str| now.format(fmt).to_string();
    let size = |v: u32| if v == 0 { String::new() } else { v.to_string() };
    Some(match name {
        "date" => f("%Y-%m-%d"),
        "time" => f("%H-%M-%S"),
        "year" => f("%Y"),
        "month" => f("%m"),
        "day" => f("%d"),
        "hour" => f("%H"),
        "minute" => f("%M"),
        "second" => f("%S"),
        "app" => meta.app.clone().filter(|a| !a.trim().is_empty()).unwrap_or_else(|| "Blink".into()),
        "mode" => meta.mode.clone().unwrap_or_else(|| "screenshot".into()),
        "width" => size(meta.width),
        "height" => size(meta.height),
        "user" => env(&["USERNAME", "USER"]),
        "pc" => env(&["COMPUTERNAME", "HOSTNAME"]),
        _ => return None,
    })
}

/// Literal text of the pattern: `%` codes are dates (the patterns from before variables).
/// An invalid code is kept as it is.
fn literal(s: &str, now: &DateTime<Local>) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    use std::fmt::Write;
    let mut out = String::new();
    match write!(out, "{}", now.format(s)) {
        Ok(()) => out,
        Err(_) => s.to_string(),
    }
}

/// Splits a pattern into its folders and file name, each a list of pieces.
fn parse(pattern: &str, meta: &Meta, now: &DateTime<Local>) -> Vec<Vec<Piece>> {
    let mut parts = Vec::new();
    for seg in pattern.split(['\\', '/']) {
        let mut pieces = Vec::new();
        let mut text = String::new();
        let mut rest = seg;
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}').map(|c| open + c) else { break };
            text.push_str(&rest[..open]);
            let name = rest[open + 1..close].trim().to_ascii_lowercase();
            if name == "n" || name.starts_with("n:") {
                let width = name.strip_prefix("n:").and_then(|w| w.parse().ok()).unwrap_or(1).clamp(1, 9);
                pieces.push(Piece::Text(literal(&std::mem::take(&mut text), now)));
                pieces.push(Piece::Counter(width));
            } else if let Some(v) = value(&name, meta, now) {
                pieces.push(Piece::Text(literal(&std::mem::take(&mut text), now)));
                pieces.push(Piece::Text(v));
            } else {
                // Not a variable: kept as typed.
                text.push_str(&rest[open..=close]);
            }
            rest = &rest[close + 1..];
        }
        text.push_str(rest);
        pieces.push(Piece::Text(literal(&text, now)));
        parts.push(pieces);
    }
    parts
}

/// A folder or file name Windows accepts: no reserved characters, no trailing dots or
/// spaces, not a device name like `CON`, at most 120 characters.
pub fn clean_name(s: &str) -> String {
    let cleaned: String =
        s.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).take(120).collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

fn text_of(pieces: &[Piece]) -> String {
    pieces.iter().filter_map(|p| if let Piece::Text(t) = p { Some(t.as_str()) } else { None }).collect()
}

/// The next number for a name `prefix` + number + `suffix` in `dir` (any extension): one more
/// than the highest that is there, so numbers never repeat when files are deleted.
fn next_number(dir: &Path, prefix: &str, suffix: &str) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 1 };
    let (prefix, suffix) = (prefix.to_lowercase(), suffix.to_lowercase());
    let mut max = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
        let Some(mid) = stem.strip_prefix(&prefix).and_then(|s| s.strip_suffix(&suffix)) else { continue };
        if !mid.is_empty() && mid.bytes().all(|b| b.is_ascii_digit()) {
            max = max.max(mid.parse::<u64>().unwrap_or(0));
        }
    }
    max + 1
}

/// Where to save: `dir`, the folders of the pattern, and the file name with `ext`. With
/// " (2)", " (3)", ... when that file exists already. Folders are not made here.
pub fn path_at(dir: &Path, pattern: &str, ext: &str, meta: &Meta, now: &DateTime<Local>) -> PathBuf {
    let pattern = if pattern.trim().is_empty() { DEFAULT_PATTERN } else { pattern };
    let mut parts = parse(pattern, meta, now);
    let file = parts.pop().unwrap_or_default();
    let mut folder = dir.to_path_buf();
    for p in &parts {
        // A counter in a folder name is left out: it counts files, not folders.
        let name = clean_name(&text_of(p));
        if !name.is_empty() && name != "." && name != ".." {
            folder.push(name);
        }
    }
    let stem = match file.iter().position(|p| matches!(p, Piece::Counter(_))) {
        Some(i) => {
            let width = if let Piece::Counter(w) = file[i] { w } else { 1 };
            let (prefix, suffix) = (clean_name(&text_of(&file[..i])), text_of(&file[i + 1..]));
            let suffix = clean_name(&suffix);
            let n = next_number(&folder, &prefix, &suffix);
            format!("{prefix}{n:0width$}{suffix}")
        }
        None => clean_name(&text_of(&file)),
    };
    let stem = if stem.is_empty() { "screenshot".to_string() } else { stem };
    let mut path = folder.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    path
}

pub fn path(dir: &Path, pattern: &str, ext: &str, meta: &Meta) -> PathBuf {
    path_at(dir, pattern, ext, meta, &Local::now())
}

/// The newest screenshot or recording in `dir` or the folders in it (up to three deep, as a
/// pattern makes them): what "Open folder" selects, so it is in view however the folder is
/// sorted.
pub fn newest_file(dir: &Path) -> Option<PathBuf> {
    const KINDS: [&str; 7] = ["png", "jpg", "jpeg", "webp", "bmp", "gif", "mp4"];
    fn walk(dir: &Path, depth: u32, best: &mut Option<(std::time::SystemTime, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            let path = e.path();
            if kind.is_dir() {
                if depth > 0 {
                    walk(&path, depth - 1, best);
                }
                continue;
            }
            let ext = path.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if !KINDS.contains(&ext.as_str()) {
                continue;
            }
            let Ok(time) = e.metadata().and_then(|m| m.modified()) else { continue };
            if best.as_ref().is_none_or(|(t, _)| time > *t) {
                *best = Some((time, path));
            }
        }
    }
    let mut best = None;
    walk(dir, 3, &mut best);
    best.map(|(_, p)| p)
}

/// Ready-made patterns for Settings: (name, pattern).
pub fn presets() -> [(&'static str, &'static str); 5] {
    use crate::i18n::t;
    [
        (t("All in one folder"), DEFAULT_PATTERN),
        (t("A folder per month"), "{year}-{month}\\Blink_{date}_{time}"),
        (t("A folder per app"), "{app}\\{app}_{date}_{time}"),
        (t("A folder per month, then per app"), "{year}-{month}\\{app}\\{app}_{date}_{time}"),
        (t("Numbered"), "Blink_{n:4}"),
    ]
}

/// What a variable stands for, for the list in Settings.
pub fn describe(var: &str) -> &'static str {
    use crate::i18n::t;
    match var {
        "{date}" => t("the date, 2026-11-24"),
        "{time}" => t("the time, 14-32-05"),
        "{year}" => t("the year, 2026"),
        "{month}" => t("the month, 11"),
        "{day}" => t("the day, 24"),
        "{hour}" => t("the hour, 14"),
        "{minute}" => t("the minute, 32"),
        "{second}" => t("the second, 05"),
        "{app}" => t("the program you captured, like chrome or WINWORD"),
        "{mode}" => t("how you captured: region, window, screen, screens, scroll or recording"),
        "{width}" => t("the width in pixels"),
        "{height}" => t("the height in pixels"),
        "{user}" => t("your Windows user name"),
        "{pc}" => t("the name of this PC"),
        "{n}" => t("a number that counts up per folder; {n:3} writes 001"),
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 24, 14, 32, 5).unwrap()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("blink_naming_{name}_{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rel(dir: &Path, p: &Path) -> String {
        p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn default_pattern_keeps_the_old_names() {
        let d = tmp("default");
        let meta = Meta::default();
        let new = path_at(&d, DEFAULT_PATTERN, "png", &meta, &now());
        let old = path_at(&d, OLD_DEFAULT_PATTERN, "png", &meta, &now());
        assert_eq!(rel(&d, &new), "Blink_2026-11-24_14-32-05.png");
        assert_eq!(new, old);
    }

    #[test]
    fn variables_and_folders() {
        let d = tmp("vars");
        let meta = Meta { app: Some("chrome".into()), mode: Some("window".into()), width: 1280, height: 720 };
        let p = path_at(&d, "{year}-{month}\\{app}\\{app}_{mode}_{width}x{height}_{day}", "png", &meta, &now());
        assert_eq!(rel(&d, &p), "2026-11/chrome/chrome_window_1280x720_24.png");
        // Forward slashes work too, and variables ignore case.
        let p = path_at(&d, "{YEAR}/{Month}", "jpg", &meta, &now());
        assert_eq!(rel(&d, &p), "2026/11.jpg");
        // Without a known program: "Blink".
        let p = path_at(&d, "{app}", "png", &Meta::default(), &now());
        assert_eq!(rel(&d, &p), "Blink.png");
    }

    #[test]
    fn unknown_braces_and_old_codes_mix() {
        let d = tmp("mix");
        let p = path_at(&d, "{nope}_%Y_{date}", "png", &Meta::default(), &now());
        assert_eq!(rel(&d, &p), "{nope}_2026_2026-11-24.png");
        // An invalid % code does not break saving.
        let p = path_at(&d, "a%Q%", "png", &Meta::default(), &now());
        assert!(rel(&d, &p).starts_with('a'));
        // A % in a program name is not read as a date code.
        let meta = Meta { app: Some("100%Y".into()), ..Meta::default() };
        assert_eq!(rel(&d, &path_at(&d, "{app}", "png", &meta, &now())), "100%Y.png");
    }

    #[test]
    fn names_windows_refuses_are_cleaned() {
        assert_eq!(clean_name("a:b*c?"), "a_b_c_");
        assert_eq!(clean_name("trailing. "), "trailing");
        assert_eq!(clean_name("CON"), "_CON");
        assert_eq!(clean_name("com1.txt"), "_com1.txt");
        assert_eq!(clean_name("COMMENT"), "COMMENT");
        let d = tmp("clean");
        // ".." can never leave the screenshots folder; empty folders are skipped.
        let p = path_at(&d, "..\\..\\\\x", "png", &Meta::default(), &now());
        assert_eq!(rel(&d, &p), "x.png");
        assert_eq!(rel(&d, &path_at(&d, "   ", "png", &Meta::default(), &now())), "Blink_2026-11-24_14-32-05.png");
    }

    #[test]
    fn counter_counts_up_per_folder() {
        let d = tmp("counter");
        let p = path_at(&d, "shot_{n:3}", "png", &Meta::default(), &now());
        assert_eq!(rel(&d, &p), "shot_001.png");
        std::fs::write(&p, b"x").unwrap();
        std::fs::write(d.join("shot_007.jpg"), b"x").unwrap();
        std::fs::write(d.join("shot_abc.png"), b"x").unwrap();
        let p = path_at(&d, "shot_{n:3}", "png", &Meta::default(), &now());
        assert_eq!(rel(&d, &p), "shot_008.png");
        // In another folder it starts again.
        let p = path_at(&d, "{year}\\shot_{n}", "png", &Meta::default(), &now());
        assert_eq!(rel(&d, &p), "2026/shot_1.png");
    }

    #[test]
    fn existing_files_get_a_number() {
        let d = tmp("exists");
        let p = path_at(&d, "same", "png", &Meta::default(), &now());
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(rel(&d, &path_at(&d, "same", "png", &Meta::default(), &now())), "same (2).png");
    }

    #[test]
    fn newest_file_is_found_in_folders_too() {
        let d = tmp("newest");
        assert_eq!(newest_file(&d), None);
        let old = d.join("old.png");
        std::fs::write(&old, b"x").unwrap();
        std::fs::create_dir_all(d.join("2026-11")).unwrap();
        let new = d.join("2026-11").join("new.png");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&new, b"x").unwrap();
        std::fs::write(d.join("notes.txt"), b"x").unwrap();
        assert_eq!(newest_file(&d), Some(new));
    }

    #[test]
    fn meta_goes_through_the_command_line() {
        let meta = Meta { app: Some("Code".into()), mode: Some("region".into()), ..Meta::default() };
        let mut args = vec!["--edit-raw".to_string(), "x.raw".into()];
        args.extend(meta.to_args());
        assert_eq!(Meta::from_args(&args), meta);
        assert_eq!(Meta::from_args(&["--edit-raw".into()]), Meta::default());
    }

    #[test]
    fn every_variable_is_described_and_works() {
        let d = tmp("described");
        for v in VARIABLES {
            assert!(!describe(v).is_empty(), "{v}");
            let p = path_at(&d, &format!("x{v}"), "png", &Meta::default(), &now());
            assert!(!rel(&d, &p).contains('{'), "{v}");
        }
    }
}
