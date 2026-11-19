//! The changelog, built into the exe from CHANGELOG.md, so Settings can show what changed
//! in every version without asking GitHub.

/// One version's section of CHANGELOG.md.
pub struct Entry {
    pub version: &'static str,
    pub date: &'static str,
    /// The Markdown under the version heading (`### ` headings and `- ` bullets).
    pub notes: String,
}

const CHANGELOG: &str = include_str!("../CHANGELOG.md");

/// Every version in the changelog, newest first (the order of the file).
pub fn entries() -> Vec<Entry> {
    parse(CHANGELOG)
}

fn parse(text: &'static str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## [") {
            if let Some((version, after)) = rest.split_once(']') {
                let date = after.trim_start_matches([' ', '-']).trim();
                out.push(Entry { version, date, notes: String::new() });
                continue;
            }
        }
        if let Some(e) = out.last_mut() {
            e.notes.push_str(line);
            e.notes.push('\n');
        }
    }
    for e in &mut out {
        e.notes = e.notes.trim().to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_dates_and_notes() {
        let e = parse(
            "# Changelog\n\nIntro.\n\n## [1.1.0] - 2026-01-02\n\n### New\n\n- A\n\n## [1.0.0] - 2026-01-01\n\n- B\n",
        );
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].version, e[0].date), ("1.1.0", "2026-01-02"));
        assert_eq!(e[0].notes, "### New\n\n- A");
        assert_eq!(e[1].notes, "- B");
    }

    #[test]
    fn the_current_version_is_in_the_changelog() {
        // A release without its CHANGELOG.md section would show nothing under "What's new".
        let current = env!("CARGO_PKG_VERSION");
        let e = entries();
        assert_eq!(e.first().map(|e| e.version), Some(current));
        assert!(!e[0].notes.is_empty());
    }
}
