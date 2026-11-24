//! Pinned screenshots on Windows, shown by the tray process itself.
//!
//! A pin has to appear the moment you ask for it. Starting a process for it (like the editor)
//! costs a process start, a virus scan of the exe and an OpenGL context first, which made it
//! open noticeably late; the tray is already running, so it shows the pin as a plain Win32
//! window instead, like the capture overlay. The window has no frame: the bar on top
//! (`crate::pin_bar`, drawn with tiny-skia) and the screenshot below it, drawn with GDI.
//!
//! - Drag the bar or the screenshot to move it; drag an edge or a corner to resize it (the
//!   screenshot keeps its shape); `Ctrl` + scroll zooms, a double-click or the zoom in the bar
//!   goes back to 100%.
//! - On top, Copy, Save as, Edit and the opacity are in the bar and in the right-click menu;
//!   `Esc` closes, `Ctrl` + `C` / `S` / `E` / `T` / `0` do the same as the bar.
//! - The screenshot is held once, as BGRA rows the screen can draw directly, and freed when
//!   the pin closes. Shown at another size, it is shrunk smoothly once and kept that way, so a
//!   repaint (hover, focus) is a plain copy. While you resize or zoom, it is drawn fast from
//!   a copy at most twice the size on screen (`pyramid`), and smoothly again when you stop:
//!   shrinking a 4K screenshot smoothly on every mouse move is what made resizing stutter.
//! - The window is only layered (`WS_EX_LAYERED`) below 100% opacity: Windows resizes a
//!   layered window more slowly. Save as and Edit hand a copy to a process of their own, so a file dialog
//!   never holds up the tray.

use super::resident::{spawn_self, PopupMenu};
use super::tray_icons::Icon;
use super::{monitor_at, wide};
use crate::capture_bar;
use crate::i18n::{t, tf};
use crate::imageio;
use crate::pin_bar::{self, Item, State, OPACITIES};
use crate::pin_ui::PinPrefs;
use crate::theme::{self, pal};
use eframe::egui::Color32;
use image::RgbaImage;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC, DeleteObject, EndPaint,
    FillRect, FrameRect, InvalidateRect, ScreenToClient, SelectObject, SetBrushOrgEx, SetDIBitsToDevice,
    SetStretchBltMode, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLORONCOLOR, DIB_RGB_COLORS, HALFTONE,
    HBITMAP, HDC, PAINTSTRUCT, SRCCOPY,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL, VK_ESCAPE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics, GetWindowLongPtrW, KillTimer,
    LoadCursorW, LoadImageW, RegisterClassExW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, CREATESTRUCTW, CS_DBLCLKS, GWLP_USERDATA, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT,
    HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, IMAGE_ICON,
    LR_DEFAULTCOLOR, MINMAXINFO, SM_CXICON, SM_CXSMICON, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOW,
    WA_INACTIVE, WMSZ_BOTTOM, WMSZ_TOP, WMSZ_TOPLEFT, WMSZ_TOPRIGHT, WM_ACTIVATE, WM_CLOSE, WM_CONTEXTMENU,
    WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_KEYDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCALCSIZE, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST,
    WM_NCLBUTTONDBLCLK, WM_PAINT, WM_SIZE, WM_SIZING, WM_TIMER, WNDCLASSEXW, WS_EX_TOPMOST, WS_MINIMIZEBOX, WS_POPUP,
    WS_THICKFRAME,
};

const CLASS: &str = "BlinkPinWindow";
/// From `Win32::UI::Controls` and `System::SystemServices`, which Blink does not otherwise use.
const WM_MOUSELEAVE: u32 = 0x02A3;
const MK_CONTROL: u32 = 0x0008;
/// Timer that takes "Copied" off the Copy button again.
const COPIED_TIMER: usize = 1;
/// Timer after a zoom step with `Ctrl` + scroll: when it fires, the zooming has stopped and
/// the screenshot is drawn smoothly again.
const SETTLE_TIMER: usize = 2;
/// How long after the last zoom step that is.
const SETTLE_MS: u32 = 150;
/// The pyramid stops at a copy this small (either side, pixels).
const PYRAMID_MIN: u32 = 32;

/// Right-click menu ids.
const MENU_COPY: usize = 1;
const MENU_SAVE_AS: usize = 2;
const MENU_EDIT: usize = 3;
const MENU_ON_TOP: usize = 4;
const MENU_ACTUAL: usize = 5;
const MENU_CLOSE: usize = 6;
const MENU_OPACITY: usize = 100;

struct Pin {
    /// What is known about the capture, for the file name of Save as and the editor.
    meta: crate::naming::Meta,
    /// The screenshot as BGRA rows, top down.
    bgra: Vec<u8>,
    w: u32,
    h: u32,
    on_top: bool,
    /// Percent.
    opacity: u8,
    focused: bool,
    hover: Option<Item>,
    pressed: Option<Item>,
    tracking: bool,
    copied: bool,
    /// Pixels per point of the monitor the pin is on.
    scale: f32,
    /// The screenshot at half, a quarter, ... of its size (BGRA, width, height), to draw from
    /// while it is resized. Made the first time it is needed.
    pyramid: Vec<(Vec<u8>, u32, u32)>,
    /// The screenshot shrunk smoothly to the size it has on screen (width, height, bitmap),
    /// so a repaint at that size is a plain copy.
    scaled: Option<(i32, i32, HBITMAP)>,
    /// Being resized or zoomed: draw fast, smoothly again when it stops.
    fast: bool,
}

impl Drop for Pin {
    fn drop(&mut self) {
        if let Some((_, _, bmp)) = self.scaled.take() {
            unsafe {
                DeleteObject(bmp);
            }
        }
    }
}

impl Pin {
    fn bar(&self) -> i32 {
        pin_bar::height(self.scale)
    }

    /// Where the screenshot is in a client area of `cw` x `ch`.
    fn image_rect(&self, cw: i32, ch: i32) -> (i32, i32, i32, i32) {
        let bar = self.bar();
        let (x, y, w, h) = pin_bar::fit(cw, (ch - bar).max(1), self.w, self.h);
        (x, y + bar, w, h)
    }

    fn zoom(&self, cw: i32, ch: i32) -> f32 {
        self.image_rect(cw, ch).2 as f32 / self.w.max(1) as f32
    }

    fn state(&self, cw: i32, ch: i32) -> State {
        State {
            on_top: self.on_top,
            opacity: Some(self.opacity),
            zoom: (self.zoom(cw, ch) * 100.0).round() as u32,
            hover: self.hover,
            pressed: self.pressed,
            copied: self.copied,
        }
    }

    fn layout(&self, cw: i32, ch: i32) -> pin_bar::Layout {
        pin_bar::layout(cw as f32, self.scale, &self.state(cw, ch), capture_bar::fonts())
    }

    /// Picture `i` of the pyramid: 0 is the screenshot itself.
    fn level(&self, i: usize) -> (&[u8], u32, u32) {
        match i.checked_sub(1).and_then(|j| self.pyramid.get(j)) {
            Some((px, w, h)) => (px, *w, *h),
            None => (&self.bgra, self.w, self.h),
        }
    }

    fn build_pyramid(&mut self) {
        if !self.pyramid.is_empty() {
            return;
        }
        let (mut w, mut h) = (self.w, self.h);
        while w / 2 >= PYRAMID_MIN && h / 2 >= PYRAMID_MIN {
            let src = self.pyramid.last().map_or(&self.bgra, |(px, _, _)| px);
            let next = pin_bar::half_bgra(src, w, h);
            (w, h) = (next.1, next.2);
            self.pyramid.push(next);
        }
    }

    fn rgba(&self) -> Option<RgbaImage> {
        let mut px = self.bgra.clone();
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
            p[3] = 255;
        }
        RgbaImage::from_raw(self.w, self.h, px)
    }
}

fn colorref(c: Color32) -> COLORREF {
    c.r() as u32 | (c.g() as u32) << 8 | (c.b() as u32) << 16
}

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetClientRect(hwnd, &mut rc);
    }
    (rc.right - rc.left, rc.bottom - rc.top)
}

fn lparam_point(lp: LPARAM) -> (i32, i32) {
    ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32)
}

fn bitmap_info(w: i32, h: i32) -> BITMAPINFO {
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    // Negative: rows top down.
    bmi.bmiHeader.biHeight = -h;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    bmi
}

fn register() -> bool {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return true;
    }
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide(CLASS);
        // Resource 1 is the app icon (taskbar and Alt+Tab).
        let icon = |size: i32| LoadImageW(hinst, 1 as _, IMAGE_ICON, size, size, LR_DEFAULTCOLOR) as _;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hIcon: icon(GetSystemMetrics(SM_CXICON)),
            hIconSm: icon(GetSystemMetrics(SM_CXSMICON)),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        let ok = RegisterClassExW(&wc) != 0;
        DONE.store(ok, Ordering::Relaxed);
        ok
    }
}

/// Pins `img` to the screen. `at`: where it was taken (its top left, physical pixels), so the
/// pin covers that spot; otherwise it goes in the middle of the monitor under the mouse.
pub fn open(img: &RgbaImage, at: Option<(i32, i32)>, meta: &crate::naming::Meta) -> Result<(), String> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(t("The image is empty.").into());
    }
    if !register() {
        return Err("RegisterClassExW".into());
    }
    let mut bgra = img.as_raw().clone();
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let prefs = PinPrefs::load();
    let anchor = at.map_or_else(super::cursor_pos, |(x, y)| POINT { x, y });
    let Some((_, work, scale)) = monitor_at(anchor) else { return Err("monitor".into()) };
    let pin = Box::new(Pin {
        meta: meta.clone(),
        bgra,
        w,
        h,
        on_top: prefs.on_top,
        opacity: 100,
        focused: true,
        hover: None,
        pressed: None,
        tracking: false,
        copied: false,
        scale,
        pyramid: Vec::new(),
        scaled: None,
        fast: false,
    });
    // At 100%, unless that does not fit 90% of the work area.
    let bar = pin.bar();
    let (ww, wh) = (work.right - work.left, work.bottom - work.top);
    let fit = ((ww as f32 * 0.9) / w as f32).min((wh as f32 * 0.9 - bar as f32) / h as f32).min(1.0);
    let (iw, ih) = ((w as f32 * fit).round() as i32, (h as f32 * fit).round() as i32);
    let (win_w, win_h) = (iw.max(pin_bar::min_width(scale)), ih + bar);
    let (x, y) = match at {
        // The image itself over the spot it was taken; the bar above it.
        Some((x, y)) => (x - (win_w - iw) / 2, y - bar),
        None => (work.left + (ww - win_w) / 2, work.top + (wh - win_h) / 2),
    };
    let x = x.min(work.right - win_w).max(work.left);
    let y = y.min(work.bottom - win_h).max(work.top);
    let title = wide(&tf("Pinned screenshot {0} \u{d7} {1}", &[&w, &h]));
    // Not layered (see the top of this file) until the opacity goes below 100%.
    let ex = if pin.on_top { WS_EX_TOPMOST } else { 0 };
    unsafe {
        let class = wide(CLASS);
        let hwnd = CreateWindowExW(
            ex,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX,
            x,
            y,
            win_w,
            win_h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            Box::into_raw(pin) as *const std::ffi::c_void,
        );
        if hwnd.is_null() {
            return Err("CreateWindowExW".into());
        }
        // Square corners on Windows 11: the corners of the screenshot are not cut off.
        let square: u32 = 1; // DWMWCP_DONOTROUND
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &square as *const u32 as *const std::ffi::c_void,
            4,
        );
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
    Ok(())
}

unsafe fn pin_of<'a>(hwnd: HWND) -> Option<&'a mut Pin> {
    (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Pin).as_mut()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp as *const CREATESTRUCTW;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_NCDESTROY {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Pin;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if !p.is_null() {
            drop(Box::from_raw(p));
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let Some(pin) = pin_of(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
    match msg {
        // No frame: the client area is the whole window (WS_THICKFRAME only for resizing).
        WM_NCCALCSIZE if wp != 0 => 0,
        WM_NCHITTEST => hit_test(hwnd, pin, lp),
        WM_GETMINMAXINFO => {
            let mm = &mut *(lp as *mut MINMAXINFO);
            mm.ptMinTrackSize = POINT { x: pin_bar::min_width(pin.scale), y: pin.bar() + (40.0 * pin.scale) as i32 };
            0
        }
        WM_SIZING => {
            keep_shape(pin, wp as u32, &mut *(lp as *mut RECT));
            1
        }
        WM_SIZE => {
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        // Dragging an edge (or moving: the size stays, so the smooth copy is used as is).
        WM_ENTERSIZEMOVE => {
            pin.fast = true;
            0
        }
        WM_EXITSIZEMOVE => {
            settle(hwnd, pin);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hwnd, pin);
            0
        }
        WM_ACTIVATE => {
            pin.focused = (wp & 0xFFFF) as u32 != WA_INACTIVE;
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_MOUSEMOVE => {
            if !pin.tracking {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut tme);
                pin.tracking = true;
            }
            let (x, y) = lparam_point(lp);
            set_hover(hwnd, pin, item_at(hwnd, pin, x, y));
            0
        }
        WM_MOUSELEAVE => {
            pin.tracking = false;
            set_hover(hwnd, pin, None);
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_point(lp);
            pin.pressed = item_at(hwnd, pin, x, y);
            SetCapture(hwnd);
            invalidate_bar(hwnd, pin);
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            let (x, y) = lparam_point(lp);
            let pressed = pin.pressed.take();
            invalidate_bar(hwnd, pin);
            if let Some(item) = pressed.filter(|&i| item_at(hwnd, pin, x, y) == Some(i)) {
                run(hwnd, item);
            }
            0
        }
        // A double-click on the screenshot (a "caption" for dragging): back to 100%.
        WM_NCLBUTTONDBLCLK if wp == HTCAPTION as usize => {
            zoom_to(hwnd, pin, 1.0);
            0
        }
        // A quick second click on a button is just another click.
        WM_LBUTTONDBLCLK => {
            let (x, y) = lparam_point(lp);
            pin.pressed = item_at(hwnd, pin, x, y);
            SetCapture(hwnd);
            invalidate_bar(hwnd, pin);
            0
        }
        WM_CONTEXTMENU => {
            context_menu(hwnd);
            0
        }
        WM_MOUSEWHEEL => {
            if (wp & 0xFFFF) as u32 & MK_CONTROL != 0 {
                let delta = ((wp >> 16) & 0xFFFF) as i16;
                let (cw, ch) = client_size(hwnd);
                let step = if delta > 0 { 1.1 } else { 1.0 / 1.1 };
                // Fast while the wheel turns, smooth once it stops.
                pin.fast = true;
                SetTimer(hwnd, SETTLE_TIMER, SETTLE_MS, None);
                zoom_to(hwnd, pin, pin.zoom(cw, ch) * step);
            }
            0
        }
        WM_KEYDOWN => {
            let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
            match (ctrl, wp as u32) {
                (_, k) if k == VK_ESCAPE as u32 => {
                    DestroyWindow(hwnd);
                }
                (true, 0x43) => run(hwnd, Item::Copy),   // C
                (true, 0x53) => run(hwnd, Item::SaveAs), // S
                (true, 0x45) => run(hwnd, Item::Edit),   // E
                (true, 0x54) => run(hwnd, Item::OnTop),  // T
                (true, 0x30) => zoom_to(hwnd, pin, 1.0), // 0
                _ => {}
            }
            0
        }
        WM_TIMER if wp == SETTLE_TIMER => {
            settle(hwnd, pin);
            0
        }
        WM_TIMER if wp == COPIED_TIMER => {
            KillTimer(hwnd, COPIED_TIMER);
            pin.copied = false;
            invalidate_bar(hwnd, pin);
            0
        }
        WM_DPICHANGED => {
            pin.scale = (wp & 0xFFFF) as f32 / 96.0;
            let r = &*(lp as *const RECT);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// The bar item under a client point.
fn item_at(hwnd: HWND, pin: &Pin, x: i32, y: i32) -> Option<Item> {
    if y >= pin.bar() {
        return None;
    }
    let (cw, ch) = client_size(hwnd);
    pin.layout(cw, ch).hit(x as f32, y as f32)
}

/// Edges and corners resize, the bar's items are buttons, everything else moves the window.
unsafe fn hit_test(hwnd: HWND, pin: &Pin, lp: LPARAM) -> LRESULT {
    let (sx, sy) = lparam_point(lp);
    let mut pt = POINT { x: sx, y: sy };
    ScreenToClient(hwnd, &mut pt);
    let (cw, ch) = client_size(hwnd);
    let b = (6.0 * pin.scale).round() as i32;
    let (l, r, t, bo) = (pt.x < b, pt.x >= cw - b, pt.y < b, pt.y >= ch - b);
    let hit = match (l, r, t, bo) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, ..) => HTLEFT,
        (_, true, ..) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (.., true) => HTBOTTOM,
        _ if item_at(hwnd, pin, pt.x, pt.y).is_some() => HTCLIENT,
        _ => HTCAPTION,
    };
    hit as LRESULT
}

/// While resizing: the screenshot keeps its shape, so the window follows the dragged edge.
fn keep_shape(pin: &Pin, edge: u32, r: &mut RECT) {
    let bar = pin.bar();
    let ratio = pin.h as f32 / pin.w as f32;
    let min_w = pin_bar::min_width(pin.scale);
    if edge == WMSZ_TOP || edge == WMSZ_BOTTOM {
        let w = (((r.bottom - r.top - bar) as f32 / ratio).round() as i32).max(min_w);
        r.right = r.left + w;
    } else {
        let h = bar + ((r.right - r.left) as f32 * ratio).round() as i32;
        if edge == WMSZ_TOPLEFT || edge == WMSZ_TOPRIGHT {
            r.top = r.bottom - h;
        } else {
            r.bottom = r.top + h;
        }
    }
}

/// Resizing or zooming has stopped: draws the screenshot smoothly again.
fn settle(hwnd: HWND, pin: &mut Pin) {
    unsafe {
        KillTimer(hwnd, SETTLE_TIMER);
    }
    if pin.fast {
        pin.fast = false;
        let (cw, ch) = client_size(hwnd);
        let r = RECT { left: 0, top: pin.bar(), right: cw, bottom: ch };
        unsafe {
            InvalidateRect(hwnd, &r, 0);
        }
    }
}

/// Resizes the window to show the screenshot at `zoom` (1 = one pixel per screen pixel).
fn zoom_to(hwnd: HWND, pin: &Pin, zoom: f32) {
    let z = zoom.clamp(0.05, 8.0);
    let (iw, ih) = ((pin.w as f32 * z).round() as i32, (pin.h as f32 * z).round() as i32);
    let (w, h) = (iw.max(pin_bar::min_width(pin.scale)), ih + pin.bar());
    unsafe {
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn set_hover(hwnd: HWND, pin: &mut Pin, hover: Option<Item>) {
    if pin.hover != hover {
        pin.hover = hover;
        invalidate_bar(hwnd, pin);
    }
}

/// Repaints only the bar (hover, pressed): the screenshot under it stays as it is.
fn invalidate_bar(hwnd: HWND, pin: &Pin) {
    let (cw, _) = client_size(hwnd);
    let r = RECT { left: 0, top: 0, right: cw, bottom: pin.bar() };
    unsafe {
        InvalidateRect(hwnd, &r, 0);
    }
}

unsafe fn paint(hwnd: HWND, pin: &mut Pin) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let (cw, ch) = client_size(hwnd);
    if cw <= 0 || ch <= 0 {
        EndPaint(hwnd, &ps);
        return;
    }
    // Drawn off screen first, so nothing flickers while resizing.
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, cw, ch);
    let old = SelectObject(mem, bmp);
    let all = RECT { left: 0, top: 0, right: cw, bottom: ch };
    let brush = CreateSolidBrush(colorref(pal().canvas));
    FillRect(mem, &all, brush);
    DeleteObject(brush);

    let bar = pin.bar();
    let st = pin.state(cw, ch);
    let layout = pin_bar::layout(cw as f32, pin.scale, &st, capture_bar::fonts());
    if let Some(mut pix) = pin_bar::render(&layout, cw as u32, pin.scale, &st, capture_bar::fonts()) {
        pin_bar::to_bgra(&mut pix);
        let bmi = bitmap_info(cw, bar);
        SetDIBitsToDevice(
            mem,
            0,
            0,
            cw as u32,
            bar as u32,
            0,
            0,
            0,
            bar as u32,
            pix.data().as_ptr().cast(),
            &bmi,
            DIB_RGB_COLORS,
        );
    }
    // The screenshot, unless only the bar changed.
    if ps.rcPaint.bottom > bar {
        let (x, y, w, h) = pin.image_rect(cw, ch);
        draw_image(hdc, mem, pin, x, y, w, h);
    }
    // A thin edge, so the pin never looks like part of the screen below it: the accent while
    // it has the focus.
    let edge = CreateSolidBrush(colorref(if pin.focused { theme::ACCENT } else { pal().border }));
    FrameRect(mem, &all, edge);
    DeleteObject(edge);

    BitBlt(hdc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    DeleteObject(bmp);
    DeleteDC(mem);
    EndPaint(hwnd, &ps);
}

/// Draws the screenshot at `w` x `h` at `x`, `y` of `dst`; `screen` is the window's DC.
unsafe fn draw_image(screen: HDC, dst: HDC, pin: &mut Pin, x: i32, y: i32, w: i32, h: i32) {
    // 100%: pixel for pixel.
    if (w, h) == (pin.w as i32, pin.h as i32) {
        let bmi = bitmap_info(w, h);
        SetDIBitsToDevice(dst, x, y, pin.w, pin.h, 0, 0, 0, pin.h, pin.bgra.as_ptr().cast(), &bmi, DIB_RGB_COLORS);
        return;
    }
    if !matches!(pin.scaled, Some((sw, sh, _)) if (sw, sh) == (w, h)) {
        pin.build_pyramid();
        let level = pin_bar::pyramid_level(pin.w, pin.h, w, h, pin.pyramid.len() + 1);
        let fast = pin.fast;
        let (px, lw, lh) = pin.level(level);
        let bmi = bitmap_info(lw as i32, lh as i32);
        let stretch = |to: HDC, x: i32, y: i32, mode| {
            SetStretchBltMode(to, mode);
            SetBrushOrgEx(to, 0, 0, std::ptr::null_mut());
            StretchDIBits(
                to,
                x,
                y,
                w,
                h,
                0,
                0,
                lw as i32,
                lh as i32,
                px.as_ptr().cast(),
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        };
        if fast {
            // From a copy at most twice the size: quick enough for every mouse move.
            stretch(dst, x, y, COLORONCOLOR);
            return;
        }
        // Smoothly, once for this size.
        let bmp = CreateCompatibleBitmap(screen, w, h);
        if bmp.is_null() {
            stretch(dst, x, y, HALFTONE);
            return;
        }
        let dc = CreateCompatibleDC(screen);
        let old = SelectObject(dc, bmp);
        stretch(dc, 0, 0, HALFTONE);
        SelectObject(dc, old);
        DeleteDC(dc);
        if let Some((_, _, prev)) = pin.scaled.replace((w, h, bmp)) {
            DeleteObject(prev);
        }
    }
    if let Some((_, _, bmp)) = pin.scaled {
        let dc = CreateCompatibleDC(screen);
        let old = SelectObject(dc, bmp);
        BitBlt(dst, x, y, w, h, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        DeleteDC(dc);
    }
}

/// Writes the screenshot to a temporary file for another Blink process (it deletes it).
fn hand_off(pin: &Pin) -> Option<std::path::PathBuf> {
    let img = pin.rgba()?;
    let path = imageio::temp_raw_path();
    match imageio::write_raw(&path, &img) {
        Ok(()) => Some(path),
        Err(e) => {
            std::fs::remove_file(&path).ok();
            super::show_error("Blink", &e.to_string());
            None
        }
    }
}

fn spawn_meta(args: &[&str], meta: &crate::naming::Meta) {
    let extra = meta.to_args();
    let mut all: Vec<&str> = args.to_vec();
    all.extend(extra.iter().map(String::as_str));
    spawn_self(&all);
}

/// What a bar item (or its key or menu item) does.
fn run(hwnd: HWND, item: Item) {
    let Some(pin) = (unsafe { pin_of(hwnd) }) else { return };
    match item {
        Item::OnTop => {
            pin.on_top = !pin.on_top;
            let after = if pin.on_top { HWND_TOPMOST } else { HWND_NOTOPMOST };
            unsafe {
                SetWindowPos(hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            // The next pin starts the same way.
            PinPrefs { on_top: pin.on_top }.save();
            invalidate_bar(hwnd, pin);
        }
        Item::Copy => match pin.rgba().map(|img| imageio::copy_to_clipboard(&img)) {
            Some(Ok(())) => {
                pin.copied = true;
                unsafe {
                    SetTimer(hwnd, COPIED_TIMER, 1500, None);
                }
                invalidate_bar(hwnd, pin);
            }
            Some(Err(e)) => super::show_error("Blink", &tf("Copy failed: {0}", &[&e])),
            None => {}
        },
        Item::SaveAs => {
            if let Some(path) = hand_off(pin) {
                spawn_meta(&["--save-raw-as", &path.display().to_string()], &pin.meta);
            }
        }
        Item::Edit => {
            if let Some(path) = hand_off(pin) {
                spawn_meta(&["--edit-raw", &path.display().to_string()], &pin.meta);
            }
        }
        Item::Zoom => zoom_to(hwnd, pin, 1.0),
        Item::Opacity => opacity_menu(hwnd),
        Item::Close => unsafe {
            DestroyWindow(hwnd);
        },
    }
}

fn set_opacity(hwnd: HWND, pct: u8) {
    let Some(pin) = (unsafe { pin_of(hwnd) }) else { return };
    pin.opacity = pct;
    // Layered only below 100%.
    super::set_window_opacity(hwnd as isize, (pct as u32 * 255 / 100) as u8);
    invalidate_bar(hwnd, pin);
}

fn opacity_menu(hwnd: HWND) {
    let Some(current) = (unsafe { pin_of(hwnd) }).map(|p| p.opacity) else { return };
    unsafe {
        let mut m = PopupMenu::new();
        m.title(t("Opacity"));
        for pct in OPACITIES {
            m.add_check(MENU_OPACITY + pct as usize, &format!("{pct}%"), pct == current);
        }
        let id = m.track(hwnd);
        if id > MENU_OPACITY {
            set_opacity(hwnd, (id - MENU_OPACITY) as u8);
        }
    }
}

fn context_menu(hwnd: HWND) {
    let Some(pin) = (unsafe { pin_of(hwnd) }) else { return };
    let (on_top, opacity) = (pin.on_top, pin.opacity);
    unsafe {
        let mut m = PopupMenu::new();
        m.add(MENU_COPY, Icon::Clipboard, t("Copy"));
        m.add(MENU_SAVE_AS, Icon::Save, t("Save as..."));
        m.add(MENU_EDIT, Icon::Edit, t("Open in editor"));
        m.separator();
        m.add_check(MENU_ON_TOP, t("On top"), on_top);
        m.add_check(MENU_ACTUAL, t("Actual size (100%)"), false);
        m.separator();
        for pct in OPACITIES {
            m.add_check(MENU_OPACITY + pct as usize, &tf("Opacity {0}%", &[&pct]), pct == opacity);
        }
        m.separator();
        m.add_check(MENU_CLOSE, t("Close"), false);
        match m.track(hwnd) {
            MENU_COPY => run(hwnd, Item::Copy),
            MENU_SAVE_AS => run(hwnd, Item::SaveAs),
            MENU_EDIT => run(hwnd, Item::Edit),
            MENU_ON_TOP => run(hwnd, Item::OnTop),
            MENU_ACTUAL => run(hwnd, Item::Zoom),
            MENU_CLOSE => run(hwnd, Item::Close),
            id if id > MENU_OPACITY => set_opacity(hwnd, (id - MENU_OPACITY) as u8),
            _ => {}
        }
    }
}
