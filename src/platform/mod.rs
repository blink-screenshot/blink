//! Platform layer. Capturing, the tray icon, hotkeys and the selection overlay are
//! Windows-only (raw Win32, no extra runtime). The editor and settings UI are portable,
//! which also lets them be developed and tested on other systems.

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
mod other;
#[cfg(not(windows))]
pub use other::*;

/// Work area of the monitor under the cursor, in physical pixels, plus its scale factor.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct MonitorInfo {
    pub work_left: i32,
    pub work_top: i32,
    pub work_width: i32,
    pub work_height: i32,
    pub scale: f32,
}
