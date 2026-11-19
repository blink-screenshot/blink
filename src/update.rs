//! Updates from GitHub Releases: check the latest release, download its installer, verify it
//! and run it silently. The installer closes Blink, replaces it and starts it again.
//!
//! The repository is public: no token or account is needed.

use crate::i18n::{t, tf};
use crate::platform;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub const REPO: &str = "blink-screenshot/blink";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// "v1.2.3" / "1.2" -> Version. Pre-releases ("1.2.0-beta") are not offered as updates.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        if s.contains('-') || s.is_empty() {
            return None;
        }
        let mut parts = s.split('.').map(|p| p.parse::<u32>());
        let major = parts.next()?.ok()?;
        let minor = parts.next().unwrap_or(Ok(0)).ok()?;
        let patch = parts.next().unwrap_or(Ok(0)).ok()?;
        Some(Self(major, minor, patch))
    }
    pub fn current() -> Self {
        Self::parse(CURRENT).expect("valid package version")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    /// Lowercase hex SHA-256, when GitHub reports it.
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    pub name: String,
    pub notes: String,
    pub page: String,
    pub asset: Asset,
}

/// Reads a GitHub "release" JSON object.
pub fn parse_release(json: &str) -> Result<Release, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| tf("Unexpected answer from GitHub: {0}", &[&e]))?;
    let tag = v["tag_name"].as_str().unwrap_or_default();
    let version = Version::parse(tag).ok_or_else(|| tf("The latest release ({0}) has no version number.", &[&tag]))?;
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let installer = assets
        .iter()
        .find(|a| {
            let n = a["name"].as_str().unwrap_or_default().to_ascii_lowercase();
            n.starts_with("blink-setup") && n.ends_with(".exe")
        })
        .ok_or_else(|| tf("Release {0} has no installer (Blink-Setup-*.exe).", &[&tag]))?;
    let sha256 = installer["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(|h| h.to_ascii_lowercase())
        .filter(|h| h.len() == 64);
    Ok(Release {
        version,
        name: v["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or(tag).to_string(),
        notes: v["body"].as_str().unwrap_or_default().trim().to_string(),
        page: v["html_url"].as_str().unwrap_or_default().to_string(),
        asset: Asset {
            id: installer["id"].as_u64().ok_or(t("Installer asset without id"))?,
            name: installer["name"].as_str().unwrap_or_default().to_string(),
            size: installer["size"].as_u64().unwrap_or(0),
            sha256,
        },
    })
}

pub enum Check {
    UpToDate,
    Available(Release),
}

fn headers(accept: &str) -> [(&str, &str); 2] {
    [("Accept", accept), ("X-GitHub-Api-Version", "2022-11-28")]
}

/// Asks GitHub for the latest release and compares it with this version.
pub fn check() -> Result<Check, String> {
    check_repo(REPO)
}

fn check_repo(repo: &str) -> Result<Check, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let mut body = Vec::new();
    let status = platform::http_get(&url, &headers("application/vnd.github+json"), &mut |chunk, _| {
        body.extend_from_slice(chunk);
        body.len() < 4 << 20
    })?;
    match status {
        200 => {}
        404 => return Err(t("GitHub shows no published release of Blink yet.").into()),
        403 | 429 => return Err(t("GitHub is limiting requests right now. Try again later.").into()),
        s => return Err(tf("GitHub answered with HTTP {0}.", &[&s])),
    }
    let rel = parse_release(&String::from_utf8_lossy(&body))?;
    Ok(if rel.version > Version::current() { Check::Available(rel) } else { Check::UpToDate })
}

/// Downloads and verifies the installer. `progress(done, total)`; set `cancel` to stop.
pub fn download(rel: &Release, progress: &mut dyn FnMut(u64, u64), cancel: &AtomicBool) -> Result<PathBuf, String> {
    download_from(REPO, rel, progress, cancel)
}

fn download_from(
    repo: &str,
    rel: &Release,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/assets/{}", rel.asset.id);
    let dir = std::env::temp_dir().join("BlinkUpdate");
    std::fs::create_dir_all(&dir).map_err(|e| tf("Cannot create {1}: {0}", &[&e, &dir.display()]))?;
    let name = format!("Blink-Setup-{}.exe", rel.version);
    let part = dir.join(format!("{name}.part"));
    let mut file = std::io::BufWriter::new(std::fs::File::create(&part).map_err(|e| e.to_string())?);
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut write_err = None;
    let status = platform::http_get(&url, &headers("application/octet-stream"), &mut |chunk, total| {
        if let Err(e) = file.write_all(chunk) {
            write_err = Some(e.to_string());
            return false;
        }
        hasher.update(chunk);
        done += chunk.len() as u64;
        progress(done, total.unwrap_or(rel.asset.size).max(done));
        !cancel.load(Ordering::Relaxed)
    });
    let fail = |msg: String| {
        std::fs::remove_file(&part).ok();
        Err(msg)
    };
    if cancel.load(Ordering::Relaxed) {
        return fail(t("Download cancelled.").into());
    }
    if let Some(e) = write_err {
        return fail(tf("Could not save the download: {0}", &[&e]));
    }
    match status {
        Ok(200) => {}
        Ok(s) => return fail(tf("Download failed (HTTP {0}).", &[&s])),
        Err(e) => return fail(e),
    }
    if let Err(e) = file.flush() {
        return fail(e.to_string());
    }
    drop(file);
    if rel.asset.size > 0 && done != rel.asset.size {
        return fail(tf("Download incomplete ({0} of {1} bytes).", &[&done, &rel.asset.size]));
    }
    let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if let Some(want) = &rel.asset.sha256 {
        if &hash != want {
            return fail(t("The download does not match the checksum GitHub reports. Not installing it.").into());
        }
    }
    let exe = dir.join(name);
    std::fs::remove_file(&exe).ok();
    std::fs::rename(&part, &exe).map_err(|e| e.to_string())?;
    Ok(exe)
}

/// Starts the installer silently. It closes every Blink window (this one included), installs
/// over the current version and starts Blink again.
pub fn install(installer: &std::path::Path) -> Result<(), String> {
    std::process::Command::new(installer)
        .args(["/S", "/RESTART"])
        .spawn()
        .map(|_| ())
        .map_err(|e| tf("Could not start the installer: {0}", &[&e]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(Version::parse("v0.2.0"), Some(Version(0, 2, 0)));
        assert_eq!(Version::parse("1.10"), Some(Version(1, 10, 0)));
        assert_eq!(Version::parse("v1.0.0-beta"), None);
        assert_eq!(Version::parse("latest"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 3));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert_eq!(Version::current().to_string(), CURRENT);
    }

    #[test]
    fn parses_release_json() {
        let json = r#"{
            "tag_name": "v0.3.0", "name": "", "body": "Fixes\n", "html_url": "https://github.com/x/y/releases/tag/v0.3.0",
            "assets": [
                {"id": 1, "name": "Blink-portable.exe", "size": 10},
                {"id": 2, "name": "Blink-Setup-0.3.0.exe", "size": 1234,
                 "digest": "sha256:ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789"}
            ]
        }"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, Version(0, 3, 0));
        assert_eq!(r.name, "v0.3.0");
        assert_eq!(r.notes, "Fixes");
        assert_eq!(r.asset.id, 2);
        assert_eq!(r.asset.size, 1234);
        assert_eq!(r.asset.sha256.as_deref(), Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"));
        let no_installer = r#"{"tag_name": "v0.3.0", "assets": [{"id": 1, "name": "Blink-portable.exe"}]}"#;
        assert!(parse_release(no_installer).is_err());
    }

    /// Talks to the real GitHub API (needs network): `cargo test github -- --ignored`.
    /// Uses a public repository's release asset to exercise redirects and the checksum.
    #[test]
    #[ignore]
    fn github_download_and_verify() {
        check().unwrap();
        let mut body = Vec::new();
        let url = "https://api.github.com/repos/cli/cli/releases/latest";
        let status = platform::http_get(url, &[("Accept", "application/vnd.github+json")], &mut |c, _| {
            body.extend_from_slice(c);
            true
        })
        .unwrap();
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let a = v["assets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["digest"].as_str().is_some_and(|d| d.starts_with("sha256:")))
            .min_by_key(|a| a["size"].as_u64().unwrap())
            .expect("an asset with a digest")
            .clone();
        let mut rel = Release {
            version: Version(9, 9, 9),
            name: String::new(),
            notes: String::new(),
            page: String::new(),
            asset: Asset {
                id: a["id"].as_u64().unwrap(),
                name: a["name"].as_str().unwrap().into(),
                size: a["size"].as_u64().unwrap(),
                sha256: a["digest"].as_str().unwrap().strip_prefix("sha256:").map(str::to_string),
            },
        };
        let cancel = AtomicBool::new(false);
        let path = download_from("cli/cli", &rel, &mut |_, _| {}, &cancel).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), rel.asset.size);
        std::fs::remove_file(path).ok();
        // A wrong checksum must be refused.
        rel.asset.sha256 = Some("0".repeat(64));
        let err = download_from("cli/cli", &rel, &mut |_, _| {}, &cancel).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
    }
}
