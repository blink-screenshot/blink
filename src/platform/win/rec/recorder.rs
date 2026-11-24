//! The recorder: its own process (`Blink --record <folder>`), started by the tray once you have
//! picked what to record. A crash here costs one recording, never the tray, and the tray never
//! loads Direct3D or Media Foundation.
//!
//! Threads:
//! - this one: the windows (countdown, the frame around the area, the bar) and the message
//!   window the tray posts "stop" to;
//! - the capture thread: grabs the screen at a constant frame rate, writes the clean master
//!   MP4 and the event log, and after stop exports the MP4 or GIF with the pointer drawn in;
//! - the hook thread (see `hooks`): mouse clicks and keys.
//!
//! The frame, the bar and the countdown are left out of the recording with
//! SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) (Windows 10 2004+). On older Windows the
//! frame lies outside the area anyway.

use super::super::capture::{pointer_now, pointer_picture, IRect};
use super::super::wide;
use super::dxgi::{Duplication, Gdi, Grab, GrabError, Grabber};
use super::hooks::{self, Input};
use super::mf::{Mf, MfError, Mp4Reader, Mp4Sink, Mp4Writer};
use super::{com_init, log};
use crate::capture_bar;
use crate::config::{VideoFormat, GIF_FPS};
use crate::i18n::{t, tf};
use crate::rec::bar::{self, Hit};
use crate::rec::compose::Overlays;
use crate::rec::events::{self, Event, Header, Kind, Timeline};
use crate::rec::export::{self, Job, Trim, VideoSource};
use crate::rec::keys::{self, KeyMode, Shown};
use crate::rec::store::{Outcome, Rec, Spec, Status};
use crate::rec::yuv::Nv12;
use crate::theme;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiny_skia::Pixmap;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, ReleaseCapture, SetCapture, TrackMouseEvent, UnregisterHotKey, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, KillTimer,
    LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassExW, SetCursor, SetTimer, SetWindowDisplayAffinity,
    ShowWindow, TranslateMessage, UpdateLayeredWindow, HTCLIENT, IDC_ARROW, IDC_SIZEALL, MSG, SW_HIDE,
    SW_SHOWNOACTIVATE, ULW_ALPHA, WDA_EXCLUDEFROMCAPTURE, WM_APP, WM_DESTROY, WM_HOTKEY, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_SETCURSOR, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

/// Class of the recorder's message window: the tray finds it to say "stop".
pub const RECORDER_CLASS: &str = "BlinkRecorderWindow";
const LAYER_CLASS: &str = "BlinkRecorderLayer";
/// Posted by the tray (or the bar): stop and save.
pub const STOP_MSG: u32 = WM_APP + 60;
/// Posted by the capture thread when it is done.
const DONE_MSG: u32 = WM_APP + 61;
const WM_MOUSELEAVE: u32 = 0x02A3;

const TIMER_TICK: usize = 1;
const TIMER_COUNTDOWN: usize = 2;
const TIMER_CONFIRM: usize = 3;
const HOTKEY_ESC: i32 = 1;

/// `stop` values.
const RUN: u8 = 0;
const STOP: u8 = 1;
const DISCARD: u8 = 2;
const CANCEL: u8 = 3;

/// What the windows and the capture thread share.
struct Shared {
    /// Start recording (after the countdown).
    go: AtomicBool,
    paused: AtomicBool,
    stop: AtomicU8,
    /// Length so far, in ms.
    ms: AtomicU32,
    /// Saving: per mille; `u32::MAX` while recording.
    progress: AtomicU32,
    /// Where the bar is (screen pixels), so clicks on it are not shown as clicks.
    bar: [AtomicI32; 4],
}

impl Shared {
    fn bar_contains(&self, x: i32, y: i32) -> bool {
        let r: Vec<i32> = self.bar.iter().map(|a| a.load(Ordering::Relaxed)).collect();
        x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]
    }
}

/// The windows of the recorder (this thread only).
struct Ui {
    shared: Arc<Shared>,
    msg_hwnd: HWND,
    bar_hwnd: HWND,
    frame_hwnd: HWND,
    countdown_hwnd: HWND,
    area: IRect,
    monitor: IRect,
    scale: f32,
    /// Top-left of the bar's panel.
    bar_pos: (i32, i32),
    bar_hover: Option<Hit>,
    bar_press: Option<Hit>,
    /// While the grip is held: the mouse relative to the bar's top-left.
    bar_grab: Option<(i32, i32)>,
    confirm_discard: bool,
    count: u32,
    done: bool,
    tracking: bool,
    excluded: bool,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

/// Runs `f` on the window state. `None` when there is none, or when it is in use already:
/// ShowWindow, UpdateLayeredWindow and the like send messages to our own window procedures
/// right away, while the code that called them still holds the state. Those messages need
/// nothing from it and go to DefWindowProc.
fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Runs a recording from its working folder. Always writes `result.json`.
pub fn run(dir: &std::path::Path) -> Result<(), String> {
    let rec = Rec::at(dir);
    super::log_panics(Some(rec.clone()));
    let spec = rec.read_spec()?;
    log(&format!(
        "recording {}x{} at ({}, {}) on monitor {:?}, scale {}, {} fps, keys {:?}",
        spec.w, spec.h, spec.x, spec.y, spec.monitor, spec.scale, spec.settings.fps, spec.settings.keys
    ));
    let outcome = match record(&rec, &spec) {
        Ok(o) => o,
        Err(e) => {
            log(&format!("failed: {e}"));
            Outcome::failed(e)
        }
    };
    log(&format!("result: {:?}", outcome));
    rec.write_result(&outcome)
}

fn record(rec: &Rec, spec: &Spec) -> Result<Outcome, String> {
    // Media Foundation first: on an N edition, say so before anything else happens.
    match Mf::get() {
        Ok(_) => {}
        Err(MfError::NoMediaFoundation) => {
            return Ok(Outcome { no_media_foundation: true, ..Outcome::failed("Media Foundation is missing".into()) })
        }
        Err(e) => return Err(e.to_string()),
    }
    let [mx, my, mw, mh] = spec.monitor;
    let monitor = IRect { x: mx, y: my, w: mw, h: mh };
    let area = IRect { x: spec.x, y: spec.y, w: spec.w, h: spec.h }.intersect(&monitor);
    if area.w < 16 || area.h < 16 {
        return Err(t("The area is too small to record.").into());
    }
    let shared = Arc::new(Shared {
        go: AtomicBool::new(false),
        paused: AtomicBool::new(false),
        stop: AtomicU8::new(RUN),
        ms: AtomicU32::new(0),
        progress: AtomicU32::new(u32::MAX),
        bar: Default::default(),
    });

    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        for (class, proc) in [(RECORDER_CLASS, msg_proc as WndProc), (LAYER_CLASS, layer_proc as WndProc)] {
            let name = wide(class);
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc),
                hInstance: hinst,
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                lpszClassName: name.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassExW(&wc);
        }
        let msg_name = wide(RECORDER_CLASS);
        let msg_hwnd = CreateWindowExW(
            0,
            msg_name.as_ptr(),
            msg_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        let layer = |clickable: bool| {
            let name = wide(LAYER_CLASS);
            let mut ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            if !clickable {
                ex |= WS_EX_TRANSPARENT;
            }
            CreateWindowExW(
                ex,
                name.as_ptr(),
                name.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinst,
                std::ptr::null(),
            )
        };
        let (bar_hwnd, frame_hwnd, countdown_hwnd) = (layer(true), layer(false), layer(false));
        if msg_hwnd.is_null() || bar_hwnd.is_null() || frame_hwnd.is_null() || countdown_hwnd.is_null() {
            return Err(t("Could not create the recording windows.").into());
        }
        // Keep our own windows out of the recording.
        let mut excluded = true;
        for h in [bar_hwnd, frame_hwnd, countdown_hwnd] {
            excluded &= SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE) != 0;
        }
        log(&format!("windows left out of the recording: {excluded}"));
        let scale = spec.scale.max(1.0);
        UI.with(|u| {
            *u.borrow_mut() = Some(Ui {
                shared: shared.clone(),
                msg_hwnd,
                bar_hwnd,
                frame_hwnd,
                countdown_hwnd,
                area,
                monitor,
                scale,
                bar_pos: (0, 0),
                bar_hover: None,
                bar_press: None,
                bar_grab: None,
                confirm_discard: false,
                count: 3,
                done: false,
                tracking: false,
                excluded,
            })
        });

        // The capture thread sets everything up during the countdown and waits for `go`.
        let thread = {
            let (rec, spec, shared) = (rec.clone(), spec.clone(), shared.clone());
            let target = msg_hwnd as isize;
            std::thread::spawn(move || {
                let out = capture(&rec, &spec, area, monitor, &shared);
                PostMessageW(target as HWND, DONE_MSG, 0, 0);
                out
            })
        };

        if spec.settings.countdown {
            RegisterHotKey(msg_hwnd, HOTKEY_ESC, 0, 0x1B);
            with_ui(|ui| show_countdown(ui));
            SetTimer(msg_hwnd, TIMER_COUNTDOWN, 1000, None);
        } else {
            with_ui(start_recording);
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let outcome = thread.join().map_err(|_| t("The recording stopped unexpectedly.").to_string())?;
        for h in [bar_hwnd, frame_hwnd, countdown_hwnd, msg_hwnd] {
            DestroyWindow(h);
        }
        UI.with(|u| u.borrow_mut().take());
        outcome
    }
}

type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

// ---- windows ---------------------------------------------------------------------------------

/// Shows a premultiplied RGBA pixmap as the whole content of a layered window at `(x, y)`.
unsafe fn show_pixmap(hwnd: HWND, pix: &Pixmap, x: i32, y: i32) {
    let (w, h) = (pix.width() as i32, pix.height() as i32);
    let screen = GetDC(std::ptr::null_mut());
    let mem = CreateCompatibleDC(screen);
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..std::mem::zeroed()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if !bmp.is_null() && !bits.is_null() {
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        for (d, s) in dst.chunks_exact_mut(4).zip(pix.data().chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
        let old = SelectObject(mem, bmp);
        let pos = POINT { x, y };
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(hwnd, screen, &pos, &size, mem, &src, 0, &blend, ULW_ALPHA);
        SelectObject(mem, old);
        DeleteObject(bmp);
    }
    DeleteDC(mem);
    ReleaseDC(std::ptr::null_mut(), screen);
}

fn show_countdown(ui: &mut Ui) {
    let fonts = capture_bar::fonts();
    if let Some(pix) = bar::countdown(ui.count, ui.scale, fonts) {
        let (w, h) = (pix.width() as i32, pix.height() as i32);
        let x = ui.area.x + (ui.area.w - w) / 2;
        let y = ui.area.y + (ui.area.h - h) / 2;
        unsafe {
            show_pixmap(ui.countdown_hwnd, &pix, x, y);
            ShowWindow(ui.countdown_hwnd, SW_SHOWNOACTIVATE);
        }
    }
}

/// The frame: a thin red line just outside the area (inside where the area touches the edge
/// of the monitor).
fn show_frame(ui: &Ui) {
    let bw = (2.0 * ui.scale).round().max(2.0) as i32;
    let outer = IRect { x: ui.area.x - bw, y: ui.area.y - bw, w: ui.area.w + 2 * bw, h: ui.area.h + 2 * bw }
        .intersect(&ui.monitor);
    if !ui.excluded
        && outer != (IRect { x: ui.area.x - bw, y: ui.area.y - bw, w: ui.area.w + 2 * bw, h: ui.area.h + 2 * bw })
    {
        // It would be inside the recording on a Windows that cannot leave it out.
        return;
    }
    let Some(mut pix) = Pixmap::new(outer.w as u32, outer.h as u32) else { return };
    let c = theme::RECORD;
    let paint = {
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(c.r(), c.g(), c.b(), 255);
        p
    };
    let (w, h) = (outer.w as f32, outer.h as f32);
    let b = bw as f32;
    for r in [
        tiny_skia::Rect::from_xywh(0.0, 0.0, w, b),
        tiny_skia::Rect::from_xywh(0.0, h - b, w, b),
        tiny_skia::Rect::from_xywh(0.0, 0.0, b, h),
        tiny_skia::Rect::from_xywh(w - b, 0.0, b, h),
    ]
    .into_iter()
    .flatten()
    {
        pix.fill_rect(r, &paint, tiny_skia::Transform::identity(), None);
    }
    unsafe {
        show_pixmap(ui.frame_hwnd, &pix, outer.x, outer.y);
        ShowWindow(ui.frame_hwnd, SW_SHOWNOACTIVATE);
    }
}

fn bar_state(ui: &Ui) -> bar::State {
    let s = &ui.shared;
    let p = s.progress.load(Ordering::Relaxed);
    bar::State {
        ms: s.ms.load(Ordering::Relaxed),
        paused: s.paused.load(Ordering::Relaxed),
        confirm_discard: ui.confirm_discard,
        saving: (p != u32::MAX).then(|| p as f32 / 1000.0),
    }
}

/// Where the bar goes first: under the area, right-aligned; above it when there is no room;
/// inside the bottom-right corner when the area fills the monitor.
fn bar_default_pos(ui: &Ui, w: i32, h: i32) -> (i32, i32) {
    let gap = (10.0 * ui.scale) as i32;
    let m = ui.monitor;
    let x = (ui.area.right() - w).clamp(m.x, (m.right() - w).max(m.x));
    if ui.area.bottom() + gap + h <= m.bottom() {
        (x, ui.area.bottom() + gap)
    } else if ui.area.y - gap - h >= m.y {
        (x, ui.area.y - gap - h)
    } else {
        (x - gap, ui.area.bottom() - h - gap)
    }
}

fn show_bar(ui: &mut Ui) {
    let fonts = capture_bar::fonts();
    let state = bar_state(ui);
    let layout = bar::Layout::new(&state, ui.scale, fonts);
    let Some(pix) = layout.render(&state, ui.bar_hover, fonts) else { return };
    let m = layout.margin;
    let (x, y) = ui.bar_pos;
    for (a, v) in ui.shared.bar.iter().zip([x, y, layout.w, layout.h]) {
        a.store(v, Ordering::Relaxed);
    }
    unsafe {
        show_pixmap(ui.bar_hwnd, &pix, x - m, y - m);
        ShowWindow(ui.bar_hwnd, SW_SHOWNOACTIVATE);
    }
}

fn start_recording(ui: &mut Ui) {
    unsafe {
        ShowWindow(ui.countdown_hwnd, SW_HIDE);
        KillTimer(ui.msg_hwnd, TIMER_COUNTDOWN);
        UnregisterHotKey(ui.msg_hwnd, HOTKEY_ESC);
    }
    let fonts = capture_bar::fonts();
    let state = bar_state(ui);
    let layout = bar::Layout::new(&state, ui.scale, fonts);
    ui.bar_pos = bar_default_pos(ui, layout.w, layout.h);
    show_frame(ui);
    show_bar(ui);
    ui.shared.go.store(true, Ordering::SeqCst);
    unsafe {
        SetTimer(ui.msg_hwnd, TIMER_TICK, 250, None);
    }
}

/// What on the bar is at screen point `(x, y)`: `None` off the bar.
fn bar_hit(ui: &Ui, x: i32, y: i32) -> Option<Option<Hit>> {
    let state = bar_state(ui);
    let layout = bar::Layout::new(&state, ui.scale, capture_bar::fonts());
    let (lx, ly) = ((x - ui.bar_pos.0) as f32, (y - ui.bar_pos.1) as f32);
    layout.contains(lx, ly).then(|| layout.hit(lx, ly))
}

fn press(ui: &mut Ui, hit: Hit) {
    let s = &ui.shared;
    if s.progress.load(Ordering::Relaxed) != u32::MAX {
        return;
    }
    match hit {
        Hit::Pause => {
            let p = !s.paused.load(Ordering::SeqCst);
            s.paused.store(p, Ordering::SeqCst);
        }
        Hit::Stop => {
            s.stop.store(STOP, Ordering::SeqCst);
            unsafe { ShowWindow(ui.frame_hwnd, SW_HIDE) };
        }
        Hit::Discard if ui.confirm_discard => {
            s.stop.store(DISCARD, Ordering::SeqCst);
            unsafe {
                ShowWindow(ui.frame_hwnd, SW_HIDE);
                ShowWindow(ui.bar_hwnd, SW_HIDE);
            }
        }
        Hit::Discard => {
            ui.confirm_discard = true;
            unsafe { SetTimer(ui.msg_hwnd, TIMER_CONFIRM, 3000, None) };
        }
        Hit::Grip => {}
    }
    show_bar(ui);
}

unsafe fn cursor() -> (i32, i32) {
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    (pt.x, pt.y)
}

unsafe extern "system" fn layer_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let is_bar = with_ui(|ui| ui.bar_hwnd == hwnd).unwrap_or(false);
    if !is_bar {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_SETCURSOR if (lp & 0xFFFF) as u32 == HTCLIENT => {
            let (x, y) = cursor();
            let grip =
                with_ui(|ui| ui.bar_grab.is_some() || bar_hit(ui, x, y) == Some(Some(Hit::Grip))).unwrap_or(false);
            SetCursor(LoadCursorW(std::ptr::null_mut(), if grip { IDC_SIZEALL } else { IDC_ARROW }));
            1
        }
        WM_MOUSEMOVE => {
            let (x, y) = cursor();
            with_ui(|ui| {
                if !ui.tracking {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut tme);
                    ui.tracking = true;
                }
                if let Some((gx, gy)) = ui.bar_grab {
                    let state = bar_state(ui);
                    let l = bar::Layout::new(&state, ui.scale, capture_bar::fonts());
                    let m = ui.monitor;
                    ui.bar_pos = (
                        (x - gx).clamp(m.x, (m.right() - l.w).max(m.x)),
                        (y - gy).clamp(m.y, (m.bottom() - l.h).max(m.y)),
                    );
                    show_bar(ui);
                    return;
                }
                let hover = bar_hit(ui, x, y).flatten();
                if hover != ui.bar_hover {
                    ui.bar_hover = hover;
                    show_bar(ui);
                }
            });
            0
        }
        WM_MOUSELEAVE => {
            with_ui(|ui| {
                ui.tracking = false;
                if ui.bar_hover.is_some() && ui.bar_grab.is_none() {
                    ui.bar_hover = None;
                    show_bar(ui);
                }
            });
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = cursor();
            with_ui(|ui| {
                let hit = bar_hit(ui, x, y).flatten();
                if hit == Some(Hit::Grip) {
                    ui.bar_grab = Some((x - ui.bar_pos.0, y - ui.bar_pos.1));
                }
                ui.bar_press = hit;
            });
            SetCapture(hwnd);
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            let (x, y) = cursor();
            with_ui(|ui| {
                let pressed = ui.bar_press.take();
                if ui.bar_grab.take().is_some() {
                    return;
                }
                let hit = bar_hit(ui, x, y).flatten();
                if let Some(h) = hit.filter(|h| Some(*h) == pressed) {
                    press(ui, h);
                }
            });
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn msg_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        STOP_MSG => {
            with_ui(|ui| {
                if ui.shared.go.load(Ordering::SeqCst) {
                    press(ui, Hit::Stop);
                } else {
                    // Still counting down: nothing recorded yet.
                    ui.shared.stop.store(CANCEL, Ordering::SeqCst);
                    ShowWindow(ui.countdown_hwnd, SW_HIDE);
                }
            });
            0
        }
        WM_HOTKEY if wp as i32 == HOTKEY_ESC => {
            with_ui(|ui| {
                ui.shared.stop.store(CANCEL, Ordering::SeqCst);
                ShowWindow(ui.countdown_hwnd, SW_HIDE);
                KillTimer(ui.msg_hwnd, TIMER_COUNTDOWN);
                UnregisterHotKey(ui.msg_hwnd, HOTKEY_ESC);
            });
            0
        }
        WM_TIMER => {
            match wp {
                TIMER_COUNTDOWN => {
                    with_ui(|ui| {
                        if ui.shared.stop.load(Ordering::SeqCst) != RUN {
                            return;
                        }
                        ui.count = ui.count.saturating_sub(1);
                        if ui.count == 0 {
                            start_recording(ui);
                        } else {
                            show_countdown(ui);
                        }
                    });
                }
                TIMER_CONFIRM => {
                    KillTimer(hwnd, TIMER_CONFIRM);
                    with_ui(|ui| {
                        ui.confirm_discard = false;
                        show_bar(ui);
                    });
                }
                TIMER_TICK => {
                    with_ui(|ui| {
                        if !ui.done && ui.shared.stop.load(Ordering::SeqCst) != DISCARD {
                            show_bar(ui);
                        }
                    });
                }
                _ => {}
            }
            0
        }
        DONE_MSG => {
            with_ui(|ui| {
                ui.done = true;
                ShowWindow(ui.bar_hwnd, SW_HIDE);
                ShowWindow(ui.frame_hwnd, SW_HIDE);
                ShowWindow(ui.countdown_hwnd, SW_HIDE);
            });
            PostQuitMessage(0);
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---- capturing ---------------------------------------------------------------------------------

/// Milliseconds since `epoch`.
fn ms_since(epoch: Instant, at: Instant) -> u64 {
    at.saturating_duration_since(epoch).as_millis() as u64
}

/// The capture thread: records until stopped, then exports.
fn capture(rec: &Rec, spec: &Spec, area: IRect, monitor: IRect, shared: &Shared) -> Result<Outcome, String> {
    com_init();
    // The screen: Desktop Duplication, else GDI.
    let mut grabber: Box<dyn Grabber> = match Duplication::new(monitor, area) {
        Ok(d) => Box::new(d),
        Err(e) => {
            log(&format!("Desktop Duplication not available ({e}); using GDI"));
            Box::new(Gdi::new(area)?)
        }
    };
    let gdi = grabber.name() == "GDI BitBlt";
    let fps = if gdi { spec.settings.fps.min(15) } else { spec.settings.fps }.clamp(5, 60);
    log(&format!("capturing with {} at {fps} fps", grabber.name()));
    // Half size on a sharp screen (200%: it then looks like the same screen at 100%), unless
    // the settings ask for every pixel; and smaller still where the encoder needs it.
    let hidpi = if !spec.settings.full_resolution && spec.scale >= 1.75 { 2 } else { 1 };
    let shrink = crate::rec::shrink_factor_from(area.w as u32, area.h as u32, hidpi);
    let zoom = 1.0 / shrink as f32;
    let (vw, vh) = (area.w as u32 / shrink, area.h as u32 / shrink);
    if shrink > 1 {
        log(&format!("recording at 1/{shrink} size: {vw}x{vh}"));
    }
    let bitrate = export::master_bitrate(vw, vh, fps);
    let writer = match Mp4Writer::create(&rec.master(), vw, vh, fps, bitrate, fps) {
        Ok(w) => w,
        Err(MfError::NoMediaFoundation) => {
            return Ok(Outcome { no_media_foundation: true, ..Outcome::failed("Media Foundation is missing".into()) })
        }
        Err(e) => return Err(tf("Could not start the video encoder: {0}", &[&e])),
    };
    let (w, h) = (writer.w, writer.h);
    let keys_mode = spec.keys();
    let header = Header { v: events::VERSION, w, h, fps, scale: spec.scale, zoom, keys: keys_mode };
    // Screen pixels to video pixels.
    let to_video = |x: i32, y: i32| (((x - area.x) as f32 * zoom) as i32, ((y - area.y) as f32 * zoom) as i32);
    let mut small: Vec<u8> = Vec::new();
    let file = std::fs::File::create(rec.events()).map_err(|e| e.to_string())?;
    let mut log_out = events::Writer::new(BufWriter::new(file), &header).map_err(|e| e.to_string())?;

    // Wait for the countdown.
    while !shared.go.load(Ordering::SeqCst) {
        if shared.stop.load(Ordering::SeqCst) != RUN {
            drop(writer);
            drop(log_out);
            for p in [rec.master(), rec.events()] {
                std::fs::remove_file(p).ok();
            }
            return Ok(Outcome::simple(Status::Cancelled));
        }
        std::thread::sleep(Duration::from_millis(15));
    }

    // The encoder runs on a thread of its own, so a slow frame never holds up the next
    // capture. Frames go there in a few buffers that come back once written; when none is
    // free, the encoder is behind and this frame is left out (the one before it lasts longer).
    const BUFFERS: usize = 3;
    let (frame_tx, frame_rx) = std::sync::mpsc::sync_channel::<(u32, Nv12)>(BUFFERS);
    let (back_tx, back_rx) = std::sync::mpsc::channel::<Nv12>();
    for _ in 0..BUFFERS {
        let _ = back_tx.send(Nv12::default());
    }
    let encoder_failed = Arc::new(AtomicBool::new(false));
    let encoder = {
        let failed = encoder_failed.clone();
        std::thread::spawn(move || -> Result<(), String> {
            com_init();
            let mut writer = writer;
            let mut error = None;
            while let Ok((t, frame)) = frame_rx.recv() {
                if error.is_none() {
                    if let Err(e) = writer.write(&frame, t) {
                        failed.store(true, Ordering::SeqCst);
                        error = Some(tf("Writing the video failed: {0}", &[&e]));
                    }
                }
                let _ = back_tx.send(frame);
            }
            writer.finish().map_err(|e| tf("Closing the video failed: {0}", &[&e]))?;
            error.map_or(Ok(()), Err)
        })
    };
    // Sleep in steps of 1 ms instead of Windows' default 15.6, for an even frame rate.
    unsafe { windows_sys::Win32::Media::timeBeginPeriod(1) };
    let (mut written, mut dropped, mut busy_ms) = (0u32, 0u32, 0u128);

    let (tx, rx) = std::sync::mpsc::channel::<Input>();
    let hook = hooks::start(keys_mode, tx);
    let epoch = Instant::now();
    let mut timeline = Timeline::start(0);
    let mut buf: Vec<u8> = Vec::new();
    let mut have_frame = false;
    let mut next_k: u64 = 0;
    let mut last_t: u32 = 0;
    let mut shapes: HashMap<isize, (u32, i32, i32)> = HashMap::new();
    let mut shape_now: Option<u32> = None;
    let mut pos_now: Option<(i32, i32)> = None;
    let mut flushed = Instant::now();
    let push = |out: &mut events::Writer<BufWriter<std::fs::File>>, t: u32, kind: Kind| {
        let _ = out.push(&Event { t, kind });
    };
    let stop_key = spec.hotkey;

    loop {
        let stop = shared.stop.load(Ordering::SeqCst);
        if stop != RUN || encoder_failed.load(Ordering::SeqCst) {
            break;
        }
        let now = ms_since(epoch, Instant::now());
        let paused = shared.paused.load(Ordering::SeqCst);
        if paused != timeline.paused() {
            if paused {
                timeline.pause(now);
                push(&mut log_out, timeline.at(now), Kind::Pause);
            } else {
                timeline.resume(now);
                push(&mut log_out, timeline.at(now), Kind::Resume);
            }
        }
        if paused {
            while rx.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let target = (next_k * 1000 / fps as u64) as u32;
        let t = timeline.at(now);
        if t < target {
            std::thread::sleep(Duration::from_millis((target - t).min(20) as u64));
            continue;
        }
        let started = Instant::now();
        // Fell behind (a slow encoder): skip to the frame of now.
        if t > target + 1000 / fps {
            next_k = t as u64 * fps as u64 / 1000;
        }
        let frame_t = (next_k * 1000 / fps as u64) as u32;
        next_k += 1;

        match grabber.grab(if have_frame { 0 } else { 200 }, &mut buf) {
            Ok(Grab::Frame) => have_frame = true,
            Ok(Grab::Same) => {}
            Err(GrabError::Lost) => {
                log("desktop duplication lost; setting it up again");
                std::thread::sleep(Duration::from_millis(100));
                match Duplication::new(monitor, area) {
                    Ok(d) => grabber = Box::new(d),
                    Err(e) => {
                        log(&format!("could not set it up again ({e}); using GDI"));
                        grabber = Box::new(Gdi::new(area)?);
                    }
                }
            }
            Err(GrabError::Failed(e)) => {
                log(&format!("{} failed ({e}); using GDI", grabber.name()));
                grabber = Box::new(Gdi::new(area)?);
            }
        }
        if !have_frame {
            continue;
        }
        match back_rx.try_recv() {
            Ok(mut frame) => {
                if shrink > 1 {
                    let (sw, sh) = crate::rec::shrink_bgra(&buf, area.w as u32, area.h as u32, shrink, &mut small);
                    frame.set_from_bgra(&small, sw as usize * 4, sw, sh);
                } else {
                    frame.set_from_bgra(&buf, area.w as usize * 4, area.w as u32, area.h as u32);
                }
                if frame_tx.send((frame_t, frame)).is_err() {
                    break;
                }
                written += 1;
            }
            Err(_) => dropped += 1,
        }
        busy_ms += started.elapsed().as_millis();
        last_t = frame_t;

        // The pointer: its shape (each new one saved once) and where it is.
        match pointer_now() {
            None => {
                if shape_now != Some(0) {
                    shape_now = Some(0);
                    push(&mut log_out, frame_t, Kind::Cursor { id: 0, hx: 0, hy: 0 });
                }
            }
            Some((handle, x, y)) => {
                let next_id = shapes.len() as u32 + 1;
                let (id, hx, hy) = *shapes.entry(handle).or_insert_with(|| match pointer_picture(handle) {
                    Some((img, hx, hy)) => {
                        let _ = img.save(rec.cursors().join(format!("{next_id}.png")));
                        (next_id, hx, hy)
                    }
                    None => (0, 0, 0),
                });
                if shape_now != Some(id) {
                    shape_now = Some(id);
                    push(&mut log_out, frame_t, Kind::Cursor { id, hx, hy });
                }
                let p = to_video(x, y);
                if pos_now != Some(p) {
                    pos_now = Some(p);
                    push(&mut log_out, frame_t, Kind::Pos { x: p.0, y: p.1 });
                }
            }
        }

        // Clicks and keys since the last frame.
        while let Ok(input) = rx.try_recv() {
            match input {
                Input::Down { b, x, y, mods, at } => {
                    if shared.bar_contains(x, y) {
                        continue;
                    }
                    let t = timeline.at(ms_since(epoch, at));
                    let (vx, vy) = to_video(x, y);
                    push(&mut log_out, t, Kind::Down { b, x: vx, y: vy, mods });
                    if let Some(label) = keys::click_label(keys_mode, b, mods) {
                        push(&mut log_out, t, Kind::Key { label, typed: false });
                    }
                }
                Input::Up { b, at } => {
                    push(&mut log_out, timeline.at(ms_since(epoch, at)), Kind::Up { b });
                }
                Input::Key { key, at } => {
                    let is_stop = stop_key.is_some_and(|h| {
                        h.vk == key.vk
                            && (h.ctrl, h.alt, h.shift, h.win)
                                == (key.mods.ctrl, key.mods.alt, key.mods.shift, key.mods.win)
                    });
                    if is_stop {
                        continue;
                    }
                    let t = timeline.at(ms_since(epoch, at));
                    match keys::filter(keys_mode, &key) {
                        Some(Shown::Combo(label)) => push(&mut log_out, t, Kind::Key { label, typed: false }),
                        Some(Shown::Typed(label)) => push(&mut log_out, t, Kind::Key { label, typed: true }),
                        None => {}
                    }
                }
            }
        }
        shared.ms.store(frame_t, Ordering::Relaxed);
        if flushed.elapsed() > Duration::from_secs(1) {
            let _ = log_out.flush();
            flushed = Instant::now();
        }
    }
    drop(hook);
    unsafe { windows_sys::Win32::Media::timeEndPeriod(1) };
    let stop = shared.stop.load(Ordering::SeqCst);
    let duration = last_t + 1000 / fps;
    drop(frame_tx);
    let encoded = encoder.join().map_err(|_| t("The recording stopped unexpectedly.").to_string())?;
    let _ = log_out.flush();
    drop(log_out);
    log(&format!(
        "recorded {duration} ms: {written} frames, {dropped} left out because the encoder was busy, {:.1} ms per frame to capture and convert",
        busy_ms as f64 / (written + dropped).max(1) as f64
    ));
    encoded?;

    if stop == DISCARD || stop == CANCEL || !have_frame {
        for p in [rec.master(), rec.events()] {
            std::fs::remove_file(p).ok();
        }
        std::fs::remove_dir_all(rec.cursors()).ok();
        let status = if stop == DISCARD { Status::Discarded } else { Status::Cancelled };
        return Ok(Outcome::simple(status));
    }

    // Save: the MP4 or GIF with the pointer, clicks and keys drawn in.
    shared.progress.store(0, Ordering::Relaxed);
    let format = spec.settings.format;
    let saved = save(rec, spec, format, duration, &mut |p| {
        shared.progress.store((p.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
        true
    });
    Ok(match saved {
        Ok((path, bytes)) => Outcome {
            path: Some(path),
            format: Some(format),
            duration_ms: duration,
            bytes,
            ..Outcome::simple(Status::Saved)
        },
        Err(e) => {
            log(&format!("export failed: {e}"));
            Outcome {
                duration_ms: duration,
                format: Some(format),
                error: Some(e),
                ..Outcome::simple(Status::ExportFailed)
            }
        }
    })
}

/// Exports a finished recording to the recordings folder as `format`, with the overlays the
/// settings ask for. Returns the file and its size.
pub fn save(
    rec: &Rec,
    spec: &Spec,
    format: VideoFormat,
    duration: u32,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<(std::path::PathBuf, u64), String> {
    let s = &spec.settings;
    std::fs::create_dir_all(&s.output_dir).map_err(|e| e.to_string())?;
    let path = crate::config::next_free_path(&s.output_dir, &spec.filename_pattern, format.ext());
    // The pattern may put it in a folder of its own.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let overlays = Overlays {
        pointer: true,
        pointer_highlight: s.pointer_highlight,
        clicks: s.click_highlight,
        keys: s.keys != KeyMode::Off,
    };
    let result = export_file(rec, &path, format, overlays, None, s.gif_max_width, duration, progress);
    if result.is_err() {
        std::fs::remove_file(&path).ok();
    }
    result.map(|b| (path, b))
}

/// Exports the recording in `rec` to `path`. Used after stop, for "As GIF" and by the clip
/// window.
#[allow(clippy::too_many_arguments)]
pub fn export_file(
    rec: &Rec,
    path: &std::path::Path,
    format: VideoFormat,
    overlays: Overlays,
    trim: Option<Trim>,
    gif_max_width: u32,
    duration_hint: u32,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<u64, String> {
    com_init();
    let clip = rec.clip()?;
    let (w, h) = (clip.log.header.w, clip.log.header.h);
    let mut reader = Mp4Reader::open(&rec.master(), Some((w, h))).map_err(|e| e.to_string())?;
    let duration = if reader.duration() > 0 { reader.duration() } else { duration_hint };
    let trim = trim.unwrap_or(Trim::whole(duration));
    let fonts = capture_bar::fonts();
    match format {
        VideoFormat::Mp4 => {
            let fps = clip.log.header.fps;
            let writer = Mp4Writer::create(path, w, h, fps, export::mp4_bitrate(w, h, fps), fps * 2)
                .map_err(|e| e.to_string())?;
            let mut sink = Mp4Sink { writer, path: path.to_path_buf() };
            let job = Job { trim, overlays, fps };
            export::export(&mut reader, &clip, &job, &mut sink, fonts, progress)
        }
        VideoFormat::Gif => {
            let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
            let mut sink = crate::rec::gif::GifSink::new(BufWriter::new(file)).max_width(Some(gif_max_width.max(64)));
            let job = Job { trim, overlays, fps: GIF_FPS };
            export::export(&mut reader, &clip, &job, &mut sink, fonts, progress)
        }
    }
}
