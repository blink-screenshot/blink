//! Fallbacks for non-Windows builds (development / tests only).
#![allow(dead_code)]

use super::MonitorInfo;

pub fn init_process() {}

/// Not mapped here: the caller reads the file instead.
pub fn map_file(_path: &std::path::Path) -> Option<&'static [u8]> {
    None
}

pub fn run_resident() -> Result<(), String> {
    Err("The tray/capture part of Blink only runs on Windows. Use --open <file> to try the editor.".into())
}

pub fn show_error(title: &str, msg: &str) {
    eprintln!("{title}: {msg}");
}

pub fn notify_resident_reload() {}

pub fn set_autostart(_enabled: bool) -> Result<(), String> {
    Ok(())
}

pub fn cursor_monitor() -> Option<MonitorInfo> {
    None
}

pub fn monitor_info_at(_x: i32, _y: i32) -> Option<MonitorInfo> {
    None
}

pub fn place_editor_window(_hwnd: isize, _w: i32, _h: i32, _maximize: bool) {}

pub fn place_pin_window(_hwnd: isize, _at: Option<(i32, i32)>, _bar_px: i32) {}

pub fn set_window_opacity(_hwnd: isize, _alpha: u8) {}

/// There is no tray to show pins on other systems.
pub fn pin_in_resident(_path: &std::path::Path, _at: Option<(i32, i32)>) -> bool {
    false
}

pub fn open_folder(path: &std::path::Path) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

pub fn resident_running() -> bool {
    false
}

pub fn print_screen_opens_snipping() -> bool {
    false
}

pub fn open_keyboard_settings() {}

pub fn request_capture() -> bool {
    false
}

pub fn system_uses_light_theme() -> bool {
    false
}

pub fn set_title_bar_theme(_hwnd: isize, _dark: bool) {}

/// Development fallback using the `curl` command.
pub fn http_get(
    url: &str,
    headers: &[(&str, &str)],
    body: &mut dyn FnMut(&[u8], Option<u64>) -> bool,
) -> Result<u16, String> {
    let tmp = std::env::temp_dir().join(format!("blink_http_{}", std::process::id()));
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-sS", "-L", "--max-redirs", "5", "--proto", "=https", "-o"]).arg(&tmp).args(["-w", "%{http_code}"]);
    cmd.args(["-A", concat!("Blink-Updater/", env!("CARGO_PKG_VERSION"))]);
    for (k, v) in headers {
        cmd.arg("-H").arg(format!("{k}: {v}"));
    }
    let out = cmd.arg(url).output().map_err(|e| format!("curl: {e}"))?;
    let status: u16 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    if status == 0 {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let data = std::fs::read(&tmp).unwrap_or_default();
    std::fs::remove_file(&tmp).ok();
    for chunk in data.chunks(64 * 1024) {
        if !body(chunk, Some(data.len() as u64)) {
            break;
        }
    }
    Ok(status)
}

pub fn install_dir() -> Option<std::path::PathBuf> {
    None
}

/// No OCR engine outside Windows.
pub fn ocr_engine() -> Result<Box<dyn crate::ocr::Engine>, crate::ocr::OcrError> {
    Err(crate::ocr::OcrError::Unsupported)
}

pub fn open_language_settings() {}

// ---- screen recording (Windows only) ----

const NO_RECORDING: &str = "Screen recording only works on Windows.";

pub fn run_recorder(_dir: &std::path::Path) -> Result<(), String> {
    Err(NO_RECORDING.into())
}

pub fn stop_recording() -> bool {
    false
}

pub fn record_test() -> Result<(), String> {
    Err(NO_RECORDING.into())
}

pub fn open_video(
    _path: &std::path::Path,
    _size: Option<(u32, u32)>,
) -> Result<Box<dyn crate::rec::export::VideoSource + Send>, String> {
    Err(NO_RECORDING.into())
}

pub fn export_recording(
    _dir: &std::path::Path,
    _path: &std::path::Path,
    _format: crate::config::VideoFormat,
    _overlays: crate::rec::compose::Overlays,
    _trim: crate::rec::export::Trim,
    _gif_max_width: u32,
    _progress: &mut dyn FnMut(f32) -> bool,
) -> Result<u64, String> {
    Err(NO_RECORDING.into())
}

pub fn export_gif(_dir: &std::path::Path) -> Result<(), String> {
    Err(NO_RECORDING.into())
}

pub fn gif_result(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("gif.json")
}

pub fn copy_files(_paths: &[std::path::PathBuf]) -> Result<(), String> {
    Err(NO_RECORDING.into())
}
