//! Screen recording on Windows: the recorder process, screen capture, Media Foundation and the
//! hooks. Everything that does not need Windows (the event log, the key filter, drawing the
//! pointer back in, trimming, the GIF encoder) is in `crate::rec` and tested there.

mod dxgi;
mod hooks;
mod mf;
mod recorder;

use crate::config::VideoFormat;
use crate::rec::compose::Overlays;
use crate::rec::export::{Trim, VideoSource};
use crate::rec::store::{self, Outcome, Rec, Status};
use std::io::Write;
use std::path::{Path, PathBuf};

pub use recorder::{run as run_recorder, RECORDER_CLASS, STOP_MSG};

/// COM for this thread (Media Foundation and DXGI need it). Harmless when it already has it.
pub(crate) fn com_init() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
}

/// Appends a line to `recordings\recorder.log`, so a failed recording can be explained. Kept
/// below 1 MB.
pub(crate) fn log(msg: &str) {
    let path = store::root().join("recorder.log");
    std::fs::create_dir_all(store::root()).ok();
    if std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        std::fs::remove_file(&path).ok();
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let _ = writeln!(f, "{now} [{}] {msg}", std::process::id());
    }
}

/// Writes a panic (Blink is built with `panic = "abort"`, so the process ends right after) to
/// `recorder.log`, and for the recorder also to `result.json`, so the tray can say what went
/// wrong instead of "stopped unexpectedly".
pub(crate) fn log_panics(rec: Option<Rec>) {
    std::panic::set_hook(Box::new(move |info| {
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        log(&format!("crash: {what} at {at}"));
        if let Some(r) = &rec {
            let _ = r.write_result(&Outcome::failed(format!("Blink crashed ({what} at {at}).")));
        }
    }));
}

/// The recorder's message window, if a recording runs.
fn recorder_window() -> Option<windows_sys::Win32::Foundation::HWND> {
    let class = super::wide(RECORDER_CLASS);
    let hwnd = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(class.as_ptr(), std::ptr::null()) };
    (!hwnd.is_null()).then_some(hwnd)
}

/// Asks the running recording to stop and save. False if none runs.
pub fn stop_recording() -> bool {
    match recorder_window() {
        Some(h) => unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(h, STOP_MSG, 0, 0) != 0 },
        None => false,
    }
}

/// Opens a video for reading (the clip window). `size`: the size it should have.
pub fn open_video(path: &Path, size: Option<(u32, u32)>) -> Result<Box<dyn VideoSource + Send>, String> {
    log_panics(None);
    com_init();
    mf::Mp4Reader::open(path, size).map(|r| Box::new(r) as Box<dyn VideoSource + Send>).map_err(|e| media_error(&e))
}

/// Why something with video failed, for people.
fn media_error(e: &mf::MfError) -> String {
    match e {
        mf::MfError::NoMediaFoundation => crate::rec::no_media_foundation().to_string(),
        mf::MfError::Failed(s) => s.clone(),
    }
}

/// Exports the recording in `dir` to `path` (the clip window).
#[allow(clippy::too_many_arguments)]
pub fn export_recording(
    dir: &Path,
    path: &Path,
    format: VideoFormat,
    overlays: Overlays,
    trim: Trim,
    gif_max_width: u32,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<u64, String> {
    let r = recorder::export_file(&Rec::at(dir), path, format, overlays, Some(trim), gif_max_width, trim.end, progress);
    if r.is_err() {
        std::fs::remove_file(path).ok();
    }
    r
}

/// `Blink --export-gif <dir>`: the "As GIF" button of the notification. Writes the GIF next to
/// the MP4 with the settings' overlays, and its outcome to `<dir>/gif.json`.
pub fn export_gif(dir: &Path) -> Result<(), String> {
    log_panics(None);
    let rec = Rec::at(dir);
    let spec = rec.read_spec()?;
    let outcome = match recorder::save(&rec, &spec, VideoFormat::Gif, 0, &mut |_| true) {
        Ok((path, bytes)) => {
            let duration = rec.read_result().map_or(0, |o| o.duration_ms);
            Outcome {
                path: Some(path),
                bytes,
                duration_ms: duration,
                format: Some(VideoFormat::Gif),
                ..Outcome::simple(Status::Saved)
            }
        }
        Err(e) => {
            log(&format!("GIF export failed: {e}"));
            Outcome::failed(e)
        }
    };
    let json = serde_json::to_string_pretty(&outcome).map_err(|e| e.to_string())?;
    std::fs::write(gif_result(dir), json).map_err(|e| e.to_string())
}

/// Where `export_gif` writes its outcome.
pub fn gif_result(dir: &Path) -> PathBuf {
    dir.join("gif.json")
}

/// Puts files on the clipboard the way Explorer does (CF_HDROP), so pasting in Teams, Slack or
/// a folder attaches the file itself.
pub fn copy_files(paths: &[PathBuf]) -> Result<(), String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::DROPFILES;

    // DROPFILES, then the paths as UTF-16, each ended by a 0, and one more 0 at the end.
    let header = std::mem::size_of::<DROPFILES>();
    let mut names: Vec<u16> = Vec::new();
    for p in paths {
        names.extend(p.display().to_string().encode_utf16());
        names.push(0);
    }
    names.push(0);
    let len = header + names.len() * 2;
    unsafe {
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        if !opened {
            return Err(crate::i18n::t("The clipboard is in use by another app.").into());
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, len);
        let mut ok = false;
        if !mem.is_null() {
            let p = GlobalLock(mem) as *mut u8;
            if !p.is_null() {
                let df = DROPFILES {
                    pFiles: header as u32,
                    pt: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
                    fNC: 0,
                    fWide: 1,
                };
                std::ptr::copy_nonoverlapping(&df as *const DROPFILES as *const u8, p, header);
                std::ptr::copy_nonoverlapping(names.as_ptr() as *const u8, p.add(header), names.len() * 2);
                GlobalUnlock(mem);
                ok = !SetClipboardData(CF_HDROP as u32, mem).is_null();
            }
            if !ok {
                GlobalFree(mem);
            }
        }
        // "Copy", not "move", when pasted into a folder.
        let effect_name = super::wide("Preferred DropEffect");
        let fmt = RegisterClipboardFormatW(effect_name.as_ptr());
        let eff = GlobalAlloc(GMEM_MOVEABLE, 4);
        if fmt != 0 && !eff.is_null() {
            let p = GlobalLock(eff) as *mut u32;
            if !p.is_null() {
                *p = 1; // DROPEFFECT_COPY
                GlobalUnlock(eff);
                if SetClipboardData(fmt, eff).is_null() {
                    GlobalFree(eff);
                }
            }
        }
        CloseClipboard();
        if ok {
            Ok(())
        } else {
            Err(crate::i18n::t("Could not put the file on the clipboard.").into())
        }
    }
}

/// `Blink --record-test`: records three seconds of the monitor under the mouse without a
/// countdown, saves it as set in Settings and says how it went. For testing on a new machine;
/// `recorder.log` has the details.
pub fn record_test() -> Result<(), String> {
    let c = crate::config::Config::load();
    let pt = super::cursor_pos();
    let (mon, _, scale) = super::monitor_at(pt).ok_or("no monitor")?;
    let m = super::capture::IRect::from_rect(mon);
    let rec = store::create(&store::root(), store::now_ms()).map_err(|e| e.to_string())?;
    let mut settings = c.recording.clone();
    settings.countdown = false;
    let spec = store::Spec {
        x: m.x,
        y: m.y,
        w: m.w,
        h: m.h,
        monitor: [m.x, m.y, m.w, m.h],
        scale,
        settings,
        filename_pattern: c.filename_pattern.clone(),
        hotkey: Some(c.hotkeys.record),
    };
    rec.write_spec(&spec)?;
    log("--record-test");
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(4));
        stop_recording();
    });
    recorder::run(&rec.dir)?;
    let o = rec.read_result().ok_or("no result")?;
    let msg = format!(
        "{:?}\n{}\n{} ms, {} bytes\n{}\n\nLog: {}",
        o.status,
        o.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        o.duration_ms,
        o.bytes,
        o.error.unwrap_or_default(),
        store::root().join("recorder.log").display()
    );
    super::show_error("Blink recording test", &msg);
    Ok(())
}
