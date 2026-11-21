//! Windows implementation (raw Win32).

mod capture;
mod http;
mod kbhook;
mod ocr;
mod overlay;
mod rec;
mod resident;
mod scroll;
mod toast;
mod tray_icons;

use super::MonitorInfo;
use crate::i18n::{t, tf};
use std::path::Path;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetCursorPos, GetWindowRect, MessageBoxW, PostMessageW, RegisterWindowMessageW, SetForegroundWindow,
    SetWindowPos, ShowWindow, HWND_TOP, MB_ICONWARNING, MB_OK, SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE, SW_SHOWNORMAL,
};

pub use http::http_get;
pub use ocr::{ocr_engine, open_language_settings};
pub use rec::{copy_files, export_gif, export_recording, open_video, record_test, run_recorder};
pub use resident::run_resident;

pub(crate) const RESIDENT_CLASS: &str = "BlinkResidentWindow";
pub(crate) const RELOAD_MESSAGE: &str = "Blink.ReloadConfig";
/// Asks the tray process to start a region capture (the "Try it" button of the welcome window).
pub(crate) const CAPTURE_MESSAGE: &str = "Blink.CaptureRegion";

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn init_process() {
    // Also declared in the manifest; this covers runs without it (e.g. `cargo run`).
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Maps a file into memory, read only, for the rest of the process: its pages come from the
/// file cache and are shared, instead of being a private copy (the tray keeps its two fonts
/// this way). `None` when it cannot be mapped; read the file instead.
pub fn map_file(path: &Path) -> Option<&'static [u8]> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, FILE_MAP_READ, PAGE_READONLY};
    let file = std::fs::File::open(path).ok()?;
    let len = usize::try_from(file.metadata().ok()?.len()).ok().filter(|&n| n > 0)?;
    unsafe {
        let mapping =
            CreateFileMappingW(file.as_raw_handle() as _, std::ptr::null(), PAGE_READONLY, 0, 0, std::ptr::null());
        if mapping.is_null() {
            return None;
        }
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
        // The view keeps the mapping and the file open by itself.
        CloseHandle(mapping);
        if view.Value.is_null() {
            return None;
        }
        // Safe: the view is `len` bytes (the whole file), readable, and never unmapped, so
        // the slice stays valid for the rest of the process. Nothing in Blink writes to it;
        // Windows does not let the file be changed or shortened while it is mapped.
        Some(std::slice::from_raw_parts(view.Value as *const u8, len))
    }
}

pub fn show_error(title: &str, msg: &str) {
    let t = wide(title);
    let m = wide(msg);
    unsafe {
        MessageBoxW(std::ptr::null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONWARNING);
    }
}

pub fn notify_resident_reload() {
    let class = wide(RESIDENT_CLASS);
    let msg = wide(RELOAD_MESSAGE);
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
        if !hwnd.is_null() {
            let id = RegisterWindowMessageW(msg.as_ptr());
            PostMessageW(hwnd, id, 0, 0);
        }
    }
}

/// Asks the tray process to start a region capture. False when it is not running.
pub fn request_capture() -> bool {
    let class = wide(RESIDENT_CLASS);
    let msg = wide(CAPTURE_MESSAGE);
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
        if hwnd.is_null() {
            return false;
        }
        let id = RegisterWindowMessageW(msg.as_ptr());
        PostMessageW(hwnd, id, 0, 0) != 0
    }
}

/// Windows "Choose your app mode": Light.
pub fn system_uses_light_theme() -> bool {
    let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let name = wide("AppsUseLightTheme");
    let mut v: u32 = 0;
    let mut len = 4u32;
    let r = unsafe {
        windows_sys::Win32::System::Registry::RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            windows_sys::Win32::System::Registry::RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut v as *mut u32).cast(),
            &mut len,
        )
    };
    r == 0 && v == 1
}

/// Gives a window's title bar the dark or light look, matching Blink's theme instead of the
/// Windows setting (otherwise a white title bar can sit on top of the dark interface).
pub fn set_title_bar_theme(hwnd: isize, dark: bool) {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    // DWMWA_USE_IMMERSIVE_DARK_MODE: 20 since Windows 10 20H1, 19 before that.
    let v: i32 = dark as i32;
    unsafe {
        for attr in [20u32, 19] {
            let r =
                DwmSetWindowAttribute(hwnd as HWND, attr, (&v as *const i32).cast(), std::mem::size_of::<i32>() as u32);
            if r == 0 {
                break;
            }
        }
    }
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "Blink";

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let key_path = wide(RUN_KEY);
    let name = wide(RUN_VALUE);
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, key_path.as_ptr(), 0, KEY_SET_VALUE, &mut key) != 0 {
            return Err(t("Could not open the Windows autostart registry key.").into());
        }
        let res = if enabled {
            let value = wide(&format!("\"{}\"", exe.display()));
            RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, value.as_ptr() as *const u8, (value.len() * 2) as u32)
        } else {
            let r = RegDeleteValueW(key, name.as_ptr());
            if r == 2 {
                0 // ERROR_FILE_NOT_FOUND: already off
            } else {
                r
            }
        };
        RegCloseKey(key);
        if res != 0 {
            return Err(tf("Could not update autostart (error {0}).", &[&res]));
        }
    }
    Ok(())
}

/// Whether the "start with Windows" registry entry exists (e.g. set by the installer).
pub(crate) fn autostart_enabled() -> bool {
    let key_path = wide(RUN_KEY);
    let name = wide(RUN_VALUE);
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, key_path.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) != 0 {
            return false;
        }
        let found = RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == 0;
        RegCloseKey(key);
        found
    }
}

pub(crate) fn monitor_at(pt: POINT) -> Option<(RECT, RECT, f32)> {
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        if mon.is_null() {
            return None;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Some((mi.rcMonitor, mi.rcWork, dx as f32 / 96.0))
    }
}

pub(crate) fn cursor_pos() -> POINT {
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut pt);
    }
    pt
}

pub fn cursor_monitor() -> Option<MonitorInfo> {
    let (_, work, scale) = monitor_at(cursor_pos())?;
    Some(MonitorInfo {
        work_left: work.left,
        work_top: work.top,
        work_width: work.right - work.left,
        work_height: work.bottom - work.top,
        scale,
    })
}

/// Centers the (outer) editor window on the monitor under the cursor and activates it.
pub fn place_editor_window(hwnd: isize, w: i32, h: i32, maximize: bool) {
    let hwnd = hwnd as HWND;
    unsafe {
        if let Some(m) = cursor_monitor() {
            let mut r: RECT = std::mem::zeroed();
            let (w, h) = if GetWindowRect(hwnd, &mut r) != 0 && r.right > r.left {
                (r.right - r.left, r.bottom - r.top)
            } else {
                (w, h)
            };
            let x = m.work_left + (m.work_width - w).max(0) / 2;
            let y = m.work_top + (m.work_height - h).max(0) / 2;
            SetWindowPos(hwnd, HWND_TOP, x, y, 0, 0, SWP_NOZORDER | SWP_NOSIZE);
        }
        if maximize {
            ShowWindow(hwnd, SW_MAXIMIZE);
        }
        SetForegroundWindow(hwnd);
    }
}

/// Whether the tray process is running (it writes the hotkey status the settings show).
pub fn resident_running() -> bool {
    let class = wide(RESIDENT_CLASS);
    unsafe { !FindWindowW(class.as_ptr(), std::ptr::null()).is_null() }
}

/// Windows 11 (and recent 10) setting "Use the Print screen key to open screen capture".
pub fn print_screen_opens_snipping() -> bool {
    let key = wide("Control Panel\\Keyboard");
    let name = wide("PrintScreenKeyForSnippingEnabled");
    let mut v: u32 = 0;
    let mut len = 4u32;
    unsafe {
        windows_sys::Win32::System::Registry::RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            windows_sys::Win32::System::Registry::RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut v as *mut u32).cast(),
            &mut len,
        );
    }
    v == 1
}

/// Opens Windows Settings at the keyboard accessibility page (Print Screen setting).
pub fn open_keyboard_settings() {
    open_folder(Path::new("ms-settings:easeofaccess-keyboard"));
}

/// Opens a web link in the default browser. Only call it with links checked by
/// `qr::is_web_link` (ShellExecute also runs programs and app protocols).
pub(crate) fn open_url(url: &str) {
    let op = wide("open");
    let u = wide(url);
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), u.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

pub fn open_folder(path: &Path) {
    let op = wide("open");
    let p = wide(&path.display().to_string());
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), p.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

/// Folder of the installed copy (written by the installer), if Blink is installed.
pub fn install_dir() -> Option<std::path::PathBuf> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};
    let key = wide("Software\\Blink");
    let name = wide("InstallDir");
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    (r == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]).into())
}
