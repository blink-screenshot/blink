//! Editor settings that carry over to the next editor window: the last tool, each tool's
//! style, the redaction mode and the background / mockup options.
//! Stored as JSON in %APPDATA%\Blink\editor.json.

use crate::annot::{RedactMode, ShapeKind, Style};
use crate::config::config_dir;
use crate::frame::FrameSettings;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorPrefs {
    pub tool: Option<String>,
    /// Style per tool id.
    pub styles: BTreeMap<String, Style>,
    pub redact_mode: Option<RedactMode>,
    pub frame: FrameSettings,
    pub shape: Option<ShapeKind>,
    pub emoji: Option<String>,
    /// The background / mockup panel is folded away.
    pub panel_collapsed: bool,
}

fn path() -> std::path::PathBuf {
    config_dir().join("editor.json")
}

impl EditorPrefs {
    pub fn load() -> Self {
        std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Best effort: a failed write only means the next editor starts with defaults.
    pub fn save(&self) {
        let Ok(json) = serde_json::to_string_pretty(self) else { return };
        if std::fs::create_dir_all(config_dir()).is_err() {
            return;
        }
        // Unique temp name: several editor windows may save at the same time.
        let tmp = path().with_extension(format!("json.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, path()).is_err() {
            std::fs::remove_file(&tmp).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut p = EditorPrefs { tool: Some("text".into()), ..Default::default() };
        let style = Style { italic: true, both_ends: true, ..Default::default() };
        p.styles.insert("text".into(), style);
        p.frame.padding = 0.25;
        let json = serde_json::to_string(&p).unwrap();
        let back: EditorPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        // Old or partial files still load.
        let partial: EditorPrefs = serde_json::from_str(r#"{"styles":{"rect":{"width":9.0}}}"#).unwrap();
        assert_eq!(partial.styles["rect"].width, 9.0);
        assert!(partial.styles["rect"].bold);
    }
}
