//! Screen grabbing and window geometry.
//!
//! The process is per-monitor-DPI-aware (v2), so every coordinate here is in physical
//! pixels and the screenshot is a 1:1 copy of what is on screen, on every monitor.

use image::RgbaImage;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HDC, HMONITOR, SRCCOPY,
};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetAncestor, GetClassNameW, GetForegroundWindow, GetLayeredWindowAttributes,
    GetSystemMetrics, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    GA_ROOT, GWL_EXSTYLE, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WS_EX_LAYERED,
    WS_EX_TRANSPARENT,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub fn from_ltrb(l: i32, t: i32, r: i32, b: i32) -> Self {
        Self { x: l.min(r), y: t.min(b), w: (r - l).abs(), h: (b - t).abs() }
    }
    pub fn from_rect(r: RECT) -> Self {
        Self::from_ltrb(r.left, r.top, r.right, r.bottom)
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    pub fn intersect(&self, o: &IRect) -> IRect {
        let l = self.x.max(o.x);
        let t = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        if r <= l || b <= t {
            IRect { x: l, y: t, w: 0, h: 0 }
        } else {
            IRect::from_ltrb(l, t, r, b)
        }
    }
    pub fn area(&self) -> i64 {
        self.w as i64 * self.h as i64
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// A captured BGRA bitmap (top-down) positioned in virtual-screen coordinates.
pub struct Shot {
    pub rect: IRect,
    pub bgra: Vec<u8>,
}

impl Shot {
    /// Copies a sub-rectangle (virtual-screen coordinates) out as RGBA.
    pub fn crop_rgba(&self, r: IRect) -> Option<RgbaImage> {
        let r = r.intersect(&self.rect);
        if r.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(r.w as usize * r.h as usize * 4);
        let stride = self.rect.w as usize * 4;
        for y in 0..r.h {
            let sy = (r.y - self.rect.y + y) as usize;
            let sx = (r.x - self.rect.x) as usize;
            let row = &self.bgra[sy * stride + sx * 4..sy * stride + (sx + r.w as usize) * 4];
            for p in row.chunks_exact(4) {
                out.extend_from_slice(&[p[2], p[1], p[0], 255]);
            }
        }
        RgbaImage::from_raw(r.w as u32, r.h as u32, out)
    }

    /// Copies a sub-rectangle (virtual-screen coordinates) out as BGRA, as it is stored.
    pub fn crop_bgra(&self, r: IRect) -> Option<(IRect, Vec<u8>)> {
        let r = r.intersect(&self.rect);
        if r.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(r.w as usize * r.h as usize * 4);
        let stride = self.rect.w as usize * 4;
        for y in 0..r.h {
            let sy = (r.y - self.rect.y + y) as usize;
            let sx = (r.x - self.rect.x) as usize;
            out.extend_from_slice(&self.bgra[sy * stride + sx * 4..sy * stride + (sx + r.w as usize) * 4]);
        }
        Some((r, out))
    }
}

unsafe extern "system" fn enum_monitor(_m: HMONITOR, _dc: HDC, r: *mut RECT, lp: LPARAM) -> BOOL {
    let out = &mut *(lp as *mut Vec<IRect>);
    if !r.is_null() {
        out.push(IRect::from_rect(*r));
    }
    1
}

/// Every monitor, in virtual-screen coordinates.
pub fn monitor_rects() -> Vec<IRect> {
    let mut out: Vec<IRect> = Vec::new();
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(enum_monitor), &mut out as *mut _ as LPARAM);
    }
    out
}

pub fn virtual_screen() -> IRect {
    unsafe {
        IRect {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            w: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            h: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// Grabs a rectangle of the screen (virtual-screen coordinates).
pub fn grab(r: IRect) -> Option<Shot> {
    if r.is_empty() {
        return None;
    }
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return None;
        }
        let mem = CreateCompatibleDC(screen);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: r.w,
            biHeight: -r.h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let mut result = None;
        if !bmp.is_null() && !bits.is_null() {
            let old = SelectObject(mem, bmp);
            if BitBlt(mem, 0, 0, r.w, r.h, screen, r.x, r.y, SRCCOPY | CAPTUREBLT) != 0 {
                let len = r.w as usize * r.h as usize * 4;
                let data = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
                result = Some(Shot { rect: r, bgra: data });
            }
            SelectObject(mem, old);
            DeleteObject(bmp);
        }
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        result
    }
}

pub fn grab_rgba(r: IRect) -> Option<RgbaImage> {
    grab(r).and_then(|s| s.crop_rgba(s.rect))
}

#[derive(Clone, Copy, Debug)]
pub struct WinInfo {
    pub hwnd: HWND,
    pub rect: IRect,
}

/// Visible window bounds without the invisible resize border / drop shadow.
/// (Using GetWindowRect here is what makes many tools capture a strip of desktop
/// around each window.)
pub fn window_bounds(hwnd: HWND) -> Option<IRect> {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        let ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut r as *mut RECT as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        ) == 0;
        if !ok && GetWindowRect(hwnd, &mut r) == 0 {
            return None;
        }
        let ir = IRect::from_rect(r);
        (!ir.is_empty()).then_some(ir)
    }
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
    }
    cloaked != 0
}

/// Shell surfaces that are never "the app window" the user means: taskbar, desktop, Start,
/// search, task view, the tray overflow, menus.
pub fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
            | "Progman"
            | "WorkerW"
            | "#32768"
            | "SysFader"
            | "Windows.UI.Core.CoreWindow"
            | "XamlExplorerHostIslandWindow"
            | "ForegroundStaging"
            | "MultitaskingViewFrame"
            | "TaskSwitcherWnd"
            | "BlinkOverlayWindow"
            | "BlinkResidentWindow"
    )
}

fn is_capturable(hwnd: HWND) -> bool {
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return false;
        }
        if is_cloaked(hwnd) {
            return false; // e.g. apps on other virtual desktops, suspended UWP apps
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TRANSPARENT != 0 {
            return false; // click-through overlays (game bars, GPU overlays)
        }
        if ex & WS_EX_LAYERED != 0 {
            let mut alpha: u8 = 255;
            let mut flags: u32 = 0;
            if GetLayeredWindowAttributes(hwnd, std::ptr::null_mut(), &mut alpha, &mut flags) != 0
                && flags & 2 != 0
                && alpha == 0
            {
                return false; // fully transparent
            }
        }
        let class = class_name(hwnd);
        !matches!(class.as_str(), "Progman" | "WorkerW" | "BlinkOverlayWindow")
    }
}

unsafe extern "system" fn enum_top(hwnd: HWND, lp: LPARAM) -> BOOL {
    let out = &mut *(lp as *mut Vec<WinInfo>);
    if is_capturable(hwnd) {
        if let Some(rect) = window_bounds(hwnd) {
            out.push(WinInfo { hwnd, rect });
        }
    }
    1
}

/// Top-level windows in z-order (topmost first).
pub fn top_level_windows() -> Vec<WinInfo> {
    let mut out: Vec<WinInfo> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_top), &mut out as *mut _ as LPARAM);
    }
    out
}

unsafe extern "system" fn enum_child(hwnd: HWND, lp: LPARAM) -> BOOL {
    let out = &mut *(lp as *mut Vec<WinInfo>);
    if IsWindowVisible(hwnd) != 0 {
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut r) != 0 {
            let rect = IRect::from_rect(r);
            if rect.w >= 24 && rect.h >= 24 {
                out.push(WinInfo { hwnd, rect });
            }
        }
    }
    1
}

/// All visible descendant windows (controls, browser content areas, ...).
pub fn child_windows(parent: HWND) -> Vec<WinInfo> {
    let mut out: Vec<WinInfo> = Vec::new();
    unsafe {
        EnumChildWindows(parent, Some(enum_child), &mut out as *mut _ as LPARAM);
    }
    out
}

/// The top-level app window `hwnd` belongs to, if it is one worth capturing (not a shell
/// surface, not one of Blink's own windows, visible, not minimized).
pub fn app_window(hwnd: HWND) -> Option<HWND> {
    unsafe {
        if hwnd.is_null() || IsWindow(hwnd) == 0 {
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        let hwnd = if root.is_null() { hwnd } else { root };
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == GetCurrentProcessId()
            || IsWindowVisible(hwnd) == 0
            || IsIconic(hwnd) != 0
            || is_cloaked(hwnd)
            || is_shell_class(&class_name(hwnd))
        {
            return None;
        }
        window_bounds(hwnd).map(|_| hwnd)
    }
}

pub fn foreground_app_window() -> Option<HWND> {
    app_window(unsafe { GetForegroundWindow() })
}
