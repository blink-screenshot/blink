//! Full-screen selection overlay on top of a frozen screenshot.
//!
//! One overlay covers the whole virtual desktop (all monitors). Hovering highlights the
//! window under the cursor (click to take it), dragging selects a region, F takes the
//! monitor, A takes all monitors, S switches to scrolling capture. Space cycles through the
//! capture modes (region / window, this screen, all screens, scrolling). The mode bar at the
//! top (see `capture_bar`) shows the active mode, takes a click to switch modes and moves to
//! the bottom of the screen while a dragged region would run under it. Its grip (left side)
//! drags the bar anywhere on the screen, to leave the default spot free for a capture. Holding Ctrl (or
//! being in scrolling mode) snaps to inner panes such as a browser's page area.
//!
//! In text mode (T) the overlay copies text instead of taking a picture. The frozen screen is
//! read on a worker thread as soon as the mode is on; once it is, the text on the screen lights
//! up. Pointing at text picks its line (with Ctrl: its paragraph), dragging picks the text in
//! a region, and a click away from text picks the text of that window. The label next to the
//! selection shows the start of the text that will be copied. A pick made before the reading is
//! done waits for it.

use super::capture::{child_windows, monitor_rects, IRect, Shot, WinInfo};
use super::{monitor_at, wide};
use crate::capture_bar::{self, Hit, Mode};
use crate::i18n::t;
use crate::ocr::{self, OcrError, Page};
use crate::theme::{self, DARK};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    AlphaBlend, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreatePen,
    CreateRoundRectRgn, CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, FillRect, GetDC, GetStockObject,
    InvalidateRect, ReleaseDC, RoundRect, SelectClipRgn, SelectObject, SetStretchBltMode, StretchBlt, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, COLORONCOLOR, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ, NULL_BRUSH, PAINTSTRUCT, PS_SOLID, SRCCOPY,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, LoadCursorW,
    PostMessageW, RegisterClassExW, SetCursor, SetCursorPos, SetForegroundWindow, ShowWindow, TranslateMessage,
    HTCLIENT, IDC_ARROW, IDC_CROSS, IDC_SIZEALL, IDC_WAIT, MSG, SW_SHOW, WM_APP, WM_DPICHANGED, WM_ERASEBKGND,
    WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR,
    WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

#[derive(Clone, Debug)]
pub struct Outcome {
    pub rect: IRect,
    pub scroll: bool,
    /// Text mode: the text that was picked (empty when there was none), or why it could not
    /// be read.
    pub text: Option<Result<String, OcrError>>,
}

/// Posted by the reading thread when the text of the screen is read.
const TEXT_READ_MSG: u32 = WM_APP + 40;

/// The text on the frozen screen (text mode).
enum TextState {
    /// Not asked for yet (another mode).
    Off,
    Reading(Receiver<Result<Page, OcrError>>),
    Ready(Page),
    Failed(OcrError),
}

const CLASS: &str = "BlinkOverlayWindow";

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
/// 0xRRGGBB to a GDI color.
const fn hex(c: u32) -> COLORREF {
    rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}
const ACCENT: COLORREF = hex(theme::hex::ACCENT);
const SCROLL: COLORREF = hex(theme::hex::SIGNAL);
const PANEL: COLORREF = hex(theme::hex::INK);

struct Gdi {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
}

impl Gdi {
    unsafe fn release(&self) {
        SelectObject(self.dc, self.old);
        DeleteObject(self.bmp);
        DeleteDC(self.dc);
    }
}

struct State {
    origin: (i32, i32),
    size: (i32, i32),
    bright: Gdi,
    dim: Gdi,
    back: Gdi,
    scale: f32,
    windows: Vec<WinInfo>,
    children: HashMap<isize, Vec<WinInfo>>,
    mode: Mode,
    /// Layout of the mode bar for the current mode.
    bar: capture_bar::Layout,
    /// The part of the mode bar under the mouse.
    bar_hover: Option<Hit>,
    /// Where the user dragged the bar to (its top-left, client coordinates); `None` is the
    /// default spot. Starts over with every capture.
    bar_pos: Option<(i32, i32)>,
    /// While the grip is held: where the mouse is on the bar, relative to its top-left.
    bar_grab: Option<(i32, i32)>,
    magnifier: bool,
    press: Option<(i32, i32)>,
    dragging: bool,
    cur: (i32, i32),
    hover: Option<IRect>,
    dirty: Vec<IRect>,
    result: Option<Option<Outcome>>,
    hwnd: isize,
    /// The screenshot the overlay shows; valid while `select` runs, which is the only time
    /// this state exists.
    shot: *const Shot,
    text: TextState,
    /// Text mode, pointing at text: the lines it picks (client coordinates in `hover`).
    text_hover: Option<Vec<usize>>,
    /// A text pick made while the screen was still being read: the area, and the lines when
    /// a line or paragraph was picked.
    waiting: Option<(IRect, Option<Vec<usize>>)>,
    /// Everything must be painted again (the lit-up text changed).
    redraw_all: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static MARKS: RefCell<Option<Marks>> = const { RefCell::new(None) };
    static BAR: RefCell<Option<BarImage>> = const { RefCell::new(None) };
}

/// The rendered mode bar, kept until the mode or the hovered segment changes.
struct BarImage {
    key: (Mode, Option<Hit>, u32),
    gdi: Gdi,
    w: i32,
    h: i32,
}

/// The four corner marks of the selection (square L-shapes, like the square corners of the
/// screenshot itself), pre-rendered with anti-aliasing because plain GDI has none. Rebuilt
/// only when size or color change.
struct Marks {
    key: (i32, i32, COLORREF),
    /// Top-left, top-right, bottom-right, bottom-left; each `arm` x `arm` pixels.
    corners: [Gdi; 4],
}

impl Marks {
    unsafe fn release(&self) {
        for c in &self.corners {
            c.release();
        }
    }
}

/// Shows the overlay and runs a modal loop until the user selects or cancels. It starts in
/// `mode` (region / window, scrolling or text).
pub fn select(shot: &Shot, windows: Vec<WinInfo>, mode: Mode, magnifier: bool) -> Option<Outcome> {
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_CROSS),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc); // fails harmlessly if already registered

        let r = shot.rect;
        let screen = GetDC(std::ptr::null_mut());
        let bright = dib_from_bgra(screen, r.w, r.h, &shot.bgra);
        let dimmed: Vec<u8> = shot.bgra.chunks_exact(4).flat_map(|p| [dim(p[0]), dim(p[1]), dim(p[2]), 255]).collect();
        let dim = dib_from_bgra(screen, r.w, r.h, &dimmed);
        drop(dimmed);
        let back_dc = CreateCompatibleDC(screen);
        let back_bmp = CreateCompatibleBitmap(screen, r.w, r.h);
        let back_old = SelectObject(back_dc, back_bmp);
        ReleaseDC(std::ptr::null_mut(), screen);

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let scale = monitor_at(pt).map_or(1.0, |m| m.2).max(1.0);
        let (Some(bright), Some(dim)) = (bright, dim) else {
            return None;
        };
        let state = State {
            origin: (r.x, r.y),
            size: (r.w, r.h),
            bright,
            dim,
            back: Gdi { dc: back_dc, bmp: back_bmp, old: back_old },
            scale,
            windows,
            children: HashMap::new(),
            mode,
            bar: capture_bar::Layout::new(mode, scale, capture_bar::fonts()),
            bar_hover: None,
            bar_pos: None,
            bar_grab: None,
            magnifier,
            press: None,
            dragging: false,
            cur: (pt.x - r.x, pt.y - r.y),
            hover: None,
            dirty: Vec::new(),
            result: None,
            hwnd: 0,
            shot: shot as *const Shot,
            text: TextState::Off,
            text_hover: None,
            waiting: None,
            redraw_all: false,
        };
        STATE.with(|s| *s.borrow_mut() = Some(state));

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            r.x,
            r.y,
            r.w,
            r.h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            cleanup();
            return None;
        }
        with_state(|s| {
            s.hwnd = hwnd as isize;
            if s.mode == Mode::Text {
                start_reading(s);
            }
            update_hover(s);
        });
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        InvalidateRect(hwnd, std::ptr::null(), 0);

        let mut msg: MSG = std::mem::zeroed();
        loop {
            if with_state(|s| s.result.is_some()).unwrap_or(true) {
                break;
            }
            if GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) <= 0 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let result = with_state(|s| s.result.take().flatten()).flatten();
        DestroyWindow(hwnd);
        cleanup();
        result
    }
}

fn dim(c: u8) -> u8 {
    (c as u32 * 45 / 100 + 6) as u8
}

unsafe fn dib_from_bgra(screen: HDC, w: i32, h: i32, data: &[u8]) -> Option<Gdi> {
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
    if bmp.is_null() || bits.is_null() {
        return None;
    }
    std::ptr::copy_nonoverlapping(data.as_ptr(), bits as *mut u8, data.len());
    let dc = CreateCompatibleDC(screen);
    let old = SelectObject(dc, bmp);
    Some(Gdi { dc, bmp, old })
}

fn cleanup() {
    MARKS.with(|m| {
        if let Some(marks) = m.borrow_mut().take() {
            unsafe { marks.release() };
        }
    });
    BAR.with(|b| {
        if let Some(bar) = b.borrow_mut().take() {
            unsafe { bar.gdi.release() };
        }
    });
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().take() {
            unsafe {
                st.bright.release();
                st.dim.release();
                st.back.release();
            }
        }
    });
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    ((lp & 0xFFFF) as u16 as i16 as i32, ((lp >> 16) & 0xFFFF) as u16 as i16 as i32)
}

/// Selection rectangle in client coordinates (drag or hover).
fn selection(s: &State) -> Option<IRect> {
    if s.dragging {
        let (px, py) = s.press?;
        let (cx, cy) = s.cur;
        Some(IRect::from_ltrb(px.min(cx), py.min(cy), px.max(cx) + 1, py.max(cy) + 1))
    } else {
        s.hover
    }
}

fn client_bounds(s: &State) -> IRect {
    IRect { x: 0, y: 0, w: s.size.0, h: s.size.1 }
}

fn to_client(s: &State, r: IRect) -> IRect {
    IRect { x: r.x - s.origin.0, y: r.y - s.origin.1, ..r }
}

/// `r` grown by `d` pixels on every side (shrunk for a negative `d`).
fn grow(r: IRect, d: i32) -> IRect {
    IRect { x: r.x - d, y: r.y - d, w: (r.w + 2 * d).max(0), h: (r.h + 2 * d).max(0) }
}

/// The screens under the corners of `r`, as one rectangle in client coordinates: the outline
/// of a selection stays inside it, so it never falls off the edge of the screen. Inner edges
/// between two monitors are not edges here, so a selection across both keeps its outline.
fn screens_under(s: &State, r: IRect) -> IRect {
    let corners = [(r.x, r.y), (r.right() - 1, r.y), (r.x, r.bottom() - 1), (r.right() - 1, r.bottom() - 1)];
    let mut out: Option<IRect> = None;
    for (x, y) in corners {
        let Some((m, _, _)) = monitor_at(POINT { x: x + s.origin.0, y: y + s.origin.1 }) else { continue };
        let m = to_client(s, IRect::from_rect(m));
        out = Some(match out {
            None => m,
            Some(o) => {
                IRect::from_ltrb(o.x.min(m.x), o.y.min(m.y), o.right().max(m.right()), o.bottom().max(m.bottom()))
            }
        });
    }
    out.unwrap_or_else(|| client_bounds(s)).intersect(&client_bounds(s))
}

/// Monitor containing the cursor, in client coordinates.
fn cursor_monitor_rect(s: &State) -> IRect {
    let pt = POINT { x: s.cur.0 + s.origin.0, y: s.cur.1 + s.origin.1 };
    match monitor_at(pt) {
        Some((m, _, _)) => to_client(s, IRect::from_rect(m)),
        None => client_bounds(s),
    }
}

fn update_hover(s: &mut State) {
    s.text_hover = None;
    let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
    match s.mode {
        Mode::Screen => {
            s.hover = Some(cursor_monitor_rect(s));
            return;
        }
        Mode::AllScreens => {
            s.hover = Some(client_bounds(s));
            return;
        }
        Mode::Text => {
            // On text: its line, or with Ctrl its paragraph. Elsewhere: the window, as in
            // region mode.
            if let TextState::Ready(page) = &s.text {
                let (x, y) = (s.cur.0 as f32, s.cur.1 as f32);
                if let Some(li) = page.line_at(x, y, 4.0 * s.scale) {
                    let lines: Vec<usize> = if ctrl { page.paragraph(li).collect() } else { vec![li] };
                    let r = lines.iter().map(|&l| page.lines[l].rect()).reduce(|a, b| a.union(&b));
                    if let Some(r) = r {
                        let r = r.expand(3.0 * s.scale);
                        let rect = IRect::from_ltrb(
                            r.x.floor() as i32,
                            r.y.floor() as i32,
                            r.right().ceil() as i32,
                            r.bottom().ceil() as i32,
                        );
                        s.hover = Some(rect.intersect(&client_bounds(s)));
                        s.text_hover = Some(lines);
                        return;
                    }
                }
            }
        }
        Mode::Region | Mode::Scroll => {}
    }
    let (x, y) = (s.cur.0 + s.origin.0, s.cur.1 + s.origin.1);
    let deep = s.mode == Mode::Scroll || ctrl;
    let top = s.windows.iter().find(|w| w.rect.contains(x, y)).copied();
    let mut hover = top.map(|w| w.rect);
    if let (true, Some(top)) = (deep, top) {
        let kids = s.children.entry(top.hwnd as isize).or_insert_with(|| child_windows(top.hwnd));
        if let Some(c) = kids
            .iter()
            .filter(|c| c.rect.contains(x, y) && c.rect.intersect(&top.rect) == c.rect)
            .min_by_key(|c| c.rect.area())
        {
            hover = Some(c.rect);
        }
    }
    s.hover = Some(match hover {
        Some(r) => to_client(s, r).intersect(&client_bounds(s)),
        None => cursor_monitor_rect(s),
    });
}

// ---- text --------------------------------------------------------------------------------

/// Starts reading the text of the frozen screen on a worker thread, one monitor at a time
/// (each at its own size, so small text stays readable on a wide desktop).
fn start_reading(s: &mut State) {
    if !matches!(s.text, TextState::Off) {
        return;
    }
    // Safe: the state, and with it this pointer, only exists while `select` runs.
    let shot = unsafe { &*s.shot };
    let mut parts: Vec<(IRect, Vec<u8>)> = monitor_rects().into_iter().filter_map(|m| shot.crop_bgra(m)).collect();
    if parts.is_empty() {
        parts.extend(shot.crop_bgra(shot.rect));
    }
    let origin = (shot.rect.x, shot.rect.y);
    let hwnd = s.hwnd;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let read = || -> Result<Page, OcrError> {
            let engine = crate::platform::ocr_engine()?;
            let mut page = Page::default();
            for (r, bgra) in parts {
                let mut part = ocr::read_with(engine.as_ref(), &bgra, r.w as u32, r.h as u32)?;
                part.translate((r.x - origin.0) as f32, (r.y - origin.1) as f32);
                page.append(part);
            }
            Ok(page)
        };
        let _ = tx.send(read());
        unsafe {
            PostMessageW(hwnd as HWND, TEXT_READ_MSG, 0, 0);
        }
    });
    s.text = TextState::Reading(rx);
}

/// The reading thread is done: the text lights up, and a pick that waited for it is finished.
fn text_read(s: &mut State) {
    let TextState::Reading(rx) = &s.text else { return };
    let Ok(result) = rx.try_recv() else { return };
    s.text = match result {
        Ok(page) => TextState::Ready(page),
        Err(e) => TextState::Failed(e),
    };
    s.redraw_all = true;
    update_hover(s);
    if let Some((rect, lines)) = s.waiting.take() {
        finish_text(s, rect, lines);
    }
}

/// The text a text-mode pick copies: whole lines, or the words inside `rect`.
fn picked_text(page: &Page, rect: IRect, lines: Option<&[usize]>) -> String {
    match lines {
        Some(lines) => page.lines_text(lines),
        None => {
            let b = ocr::Bounds::new(rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32);
            page.text_of(&page.words_in(&b))
        }
    }
}

/// What the label next to the selection says in text mode: the start of the text that will
/// be copied, or why there is none (yet).
fn text_label(s: &State, sel: IRect) -> String {
    if s.waiting.is_some() {
        return t("Reading text\u{2026}").to_string();
    }
    match &s.text {
        TextState::Off | TextState::Reading(_) => t("Reading text\u{2026}").to_string(),
        TextState::Failed(_) => t("Text recognition is not available").to_string(),
        TextState::Ready(page) => {
            let lines = if s.dragging { None } else { s.text_hover.as_deref() };
            let text = picked_text(page, sel, lines);
            if text.is_empty() {
                t("No text here").to_string()
            } else {
                ocr::preview(&text, 48)
            }
        }
    }
}

/// Ends the overlay with the text of a pick, or waits for the reading to finish.
fn finish_text(s: &mut State, rect: IRect, lines: Option<Vec<usize>>) {
    let text = match &s.text {
        TextState::Ready(page) => Ok(picked_text(page, rect, lines.as_deref())),
        TextState::Failed(e) => Err(e.clone()),
        TextState::Off | TextState::Reading(_) => {
            start_reading(s);
            s.waiting = Some((rect, lines));
            return;
        }
    };
    s.result = Some(Some(Outcome {
        rect: IRect { x: rect.x + s.origin.0, y: rect.y + s.origin.1, ..rect },
        scroll: false,
        text: Some(text),
    }));
}

// ---- corner marks ----------------------------------------------------------------------

/// Renders one corner mark into a premultiplied BGRA buffer. `flip` mirrors the top-left
/// shape horizontally and/or vertically. The outer edges of the mark touch the bitmap edge.
fn render_mark(arm: i32, width: i32, color: COLORREF, flip: (bool, bool)) -> Vec<u8> {
    use tiny_skia::{LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};
    let mut pix = Pixmap::new(arm as u32, arm as u32).expect("mark size");
    let (a, h, w) = (arm as f32, width as f32 / 2.0, width as f32);
    // Centre line: from the end of the horizontal arm, through a sharp corner, to the end of
    // the vertical arm. Butt caps and a miter join keep every corner square.
    let mut pb = PathBuilder::new();
    pb.move_to(a, h);
    pb.line_to(h, h);
    pb.line_to(h, a);
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8((color & 0xff) as u8, (color >> 8 & 0xff) as u8, (color >> 16 & 0xff) as u8, 255);
        paint.anti_alias = true;
        let stroke = Stroke { width: w, line_cap: LineCap::Butt, line_join: LineJoin::Miter, ..Stroke::default() };
        let (sx, tx) = if flip.0 { (-1.0, a) } else { (1.0, 0.0) };
        let (sy, ty) = if flip.1 { (-1.0, a) } else { (1.0, 0.0) };
        pix.stroke_path(&path, &paint, &stroke, Transform::from_row(sx, 0.0, 0.0, sy, tx, ty), None);
    }
    // tiny-skia is premultiplied RGBA; the DIB wants premultiplied BGRA.
    let mut data = pix.take();
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    data
}

/// Draws the four logo corner marks just outside `sel`, so they never cover the capture.
/// The marks have square corners, matching the (square) screenshot.
/// Draws the corner marks in the corners of `outer`, the selection grown by the marks' width
/// (and kept on the screen).
unsafe fn draw_marks(dc: HDC, outer: IRect, color: COLORREF, scale: f32) {
    let width = (3.0 * scale).round().max(3.0) as i32;
    let sel = grow(outer, -width);
    let arm = ((sel.w.min(sel.h) / 3).clamp((8.0 * scale) as i32, (30.0 * scale) as i32) + width).max(width * 2);
    let key = (arm, width, color);
    MARKS.with(|m| {
        let mut m = m.borrow_mut();
        if m.as_ref().is_none_or(|c| c.key != key) {
            if let Some(old) = m.take() {
                old.release();
            }
            let screen = GetDC(std::ptr::null_mut());
            let mut made = Vec::with_capacity(4);
            for flip in [(false, false), (true, false), (true, true), (false, true)] {
                let data = render_mark(arm, width, color, flip);
                match dib_from_bgra(screen, arm, arm, &data) {
                    Some(g) => made.push(g),
                    None => break,
                }
            }
            ReleaseDC(std::ptr::null_mut(), screen);
            match <[Gdi; 4]>::try_from(made) {
                Ok(corners) => *m = Some(Marks { key, corners }),
                Err(rest) => {
                    for g in rest {
                        g.release();
                    }
                    return;
                }
            }
        }
        let Some(marks) = m.as_ref() else { return };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let spots = [
            (outer.x, outer.y),
            (outer.right() - arm, outer.y),
            (outer.right() - arm, outer.bottom() - arm),
            (outer.x, outer.bottom() - arm),
        ];
        for (g, (x, y)) in marks.corners.iter().zip(spots) {
            AlphaBlend(dc, x, y, arm, arm, g.dc, 0, 0, arm, arm, blend);
        }
    });
}

// ---- painting --------------------------------------------------------------------------

unsafe fn fill(dc: HDC, r: IRect, c: COLORREF) {
    let b = CreateSolidBrush(c);
    let rc = RECT { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() };
    FillRect(dc, &rc, b);
    DeleteObject(b);
}

/// Copies a premultiplied RGBA pixmap into a DIB that `AlphaBlend` can draw.
unsafe fn gdi_from_pixmap(pix: &tiny_skia::Pixmap) -> Option<Gdi> {
    let mut data = pix.data().to_vec();
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let screen = GetDC(std::ptr::null_mut());
    let g = dib_from_bgra(screen, pix.width() as i32, pix.height() as i32, &data);
    ReleaseDC(std::ptr::null_mut(), screen);
    g
}

unsafe fn blend(dc: HDC, g: &Gdi, x: i32, y: i32, w: i32, h: i32) {
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    AlphaBlend(dc, x, y, w, h, g.dc, 0, 0, w, h, blend);
}

/// Draws a pixmap once (for small things that change all the time, like the size label).
unsafe fn blend_pixmap(dc: HDC, pix: &tiny_skia::Pixmap, x: i32, y: i32) {
    if let Some(g) = gdi_from_pixmap(pix) {
        blend(dc, &g, x, y, pix.width() as i32, pix.height() as i32);
        g.release();
    }
}

/// Top-left of the mode bar in client coordinates: where the user dragged it to, else centered
/// at the top of the monitor with the cursor, or at the bottom while a dragged region would
/// run under it.
fn bar_origin(s: &State) -> (i32, i32) {
    if let Some(pos) = s.bar_pos {
        return pos;
    }
    let mon = cursor_monitor_rect(s);
    let (w, h) = (s.bar.w, s.bar.h);
    let gap = (16.0 * s.scale) as i32;
    let x = mon.x + (mon.w - w) / 2;
    let top = mon.y + gap;
    if s.dragging {
        if let Some(sel) = selection(s) {
            let room = (8.0 * s.scale) as i32;
            let hits = |y: i32| {
                let area = IRect { x: x - room, y: y - room, w: w + 2 * room, h: h + 2 * room };
                !area.intersect(&sel).is_empty()
            };
            let bottom = mon.bottom() - h - gap;
            if hits(top) && !hits(bottom) {
                return (x, bottom);
            }
        }
    }
    (x, top)
}

/// Moves the bar so the mouse stays at the same spot on it, keeping the bar on the
/// screen.
fn drag_bar(s: &mut State, x: i32, y: i32) {
    let Some((gx, gy)) = s.bar_grab else { return };
    let (w, h) = (s.bar.w, s.bar.h);
    let nx = (x - gx).clamp(0, (s.size.0 - w).max(0));
    let ny = (y - gy).clamp(0, (s.size.1 - h).max(0));
    s.bar_pos = Some((nx, ny));
}

/// What on the mode bar is at `(x, y)` (client coordinates): `None` when not on the bar,
/// `Some(None)` on the bar but not on a button.
fn bar_at(s: &State, x: i32, y: i32) -> Option<Option<Hit>> {
    let (bx, by) = bar_origin(s);
    let (lx, ly) = ((x - bx) as f32, (y - by) as f32);
    s.bar.contains(lx, ly).then(|| s.bar.hit(lx, ly))
}

/// Draws the mode bar with its top-left at `(x, y)`, from a cached image when nothing on it
/// changed.
unsafe fn draw_bar(s: &State, dc: HDC, x: i32, y: i32) {
    let key = (s.mode, s.bar_hover, s.scale.to_bits());
    BAR.with(|b| {
        let mut b = b.borrow_mut();
        if b.as_ref().is_none_or(|c| c.key != key) {
            if let Some(old) = b.take() {
                old.gdi.release();
            }
            let Some(pix) = s.bar.render(s.bar_hover, capture_bar::fonts()) else { return };
            let Some(gdi) = gdi_from_pixmap(&pix) else { return };
            *b = Some(BarImage { key, gdi, w: pix.width() as i32, h: pix.height() as i32 });
        }
        if let Some(c) = b.as_ref() {
            let m = s.bar.margin;
            blend(dc, &c.gdi, x - m, y - m, c.w, c.h);
        }
    });
}

/// Everything drawn on top of the dimmed background, computed (and optionally drawn).
unsafe fn decorations(s: &State, dc: HDC, draw: bool) -> Vec<IRect> {
    let mut regions = Vec::new();
    let sc = s.scale;
    let fonts = capture_bar::fonts();
    // Thin outline (just outside the selection) plus four square corner marks. Where the
    // selection touches the edge of the screen, both are pushed inside it so they stay
    // visible; what is captured does not change.
    let bw = sc.round().max(1.0) as i32;
    let mark_w = (3.0 * sc).round().max(3.0) as i32;
    if let Some(sel) = selection(s) {
        let c = if s.mode == Mode::Scroll { SCROLL } else { ACCENT };
        let screens = screens_under(s, sel);
        let line = grow(sel, bw).intersect(&screens);
        let marks = grow(sel, mark_w).intersect(&screens);
        if draw {
            BitBlt(dc, sel.x, sel.y, sel.w, sel.h, s.bright.dc, sel.x, sel.y, SRCCOPY);
            fill(dc, IRect { h: bw, ..line }, c);
            fill(dc, IRect { y: line.bottom() - bw, h: bw, ..line }, c);
            fill(dc, IRect { w: bw, ..line }, c);
            fill(dc, IRect { x: line.right() - bw, w: bw, ..line }, c);
            draw_marks(dc, marks, c, sc);
        }
        regions.push(marks);
        // Size label above the selection (inside if there is no room), in the same color as
        // the marks like the crop tool in the editor.
        let text = if s.mode == Mode::Text { text_label(s, sel) } else { format!("{} × {}", sel.w, sel.h) };
        if let Some(label) = capture_bar::label(&text, s.mode.color(), None, theme::INK, sc, fonts) {
            let (lw, lh) = (label.width() as i32, label.height() as i32);
            let gap = mark_w + (6.0 * sc) as i32;
            let mut ly = sel.y - lh - gap;
            if ly < 0 {
                ly = sel.y + gap;
            }
            let lx = sel.x.max(0).min(s.size.0 - lw);
            if draw {
                blend_pixmap(dc, &label, lx, ly);
            }
            regions.push(IRect { x: lx, y: ly, w: lw, h: lh });
        }
    }

    // The mode bar, with room for its shadow.
    let (bx, by) = bar_origin(s);
    if draw {
        draw_bar(s, dc, bx, by);
    }
    let m = s.bar.margin;
    regions.push(IRect { x: bx - m, y: by - m, w: s.bar.w + 2 * m, h: s.bar.h + 2 * m });

    if s.magnifier {
        let mon = cursor_monitor_rect(s);
        let cells = 15;
        let zoom = (8.0 * sc).round() as i32;
        let size = cells * zoom;
        let off = (24.0 * sc) as i32;
        let (cx, cy) = s.cur;
        let mut mx = cx + off;
        let mut my = cy + off;
        if mx + size > mon.right() {
            mx = cx - off - size;
        }
        if my + size + (30.0 * sc) as i32 > mon.bottom() {
            my = cy - off - size - (30.0 * sc) as i32;
        }
        let frame = IRect { x: mx - 2, y: my - 2, w: size + 4, h: size + 4 };
        if draw {
            // Rounded like the rest of the interface. GDI clips the picture without
            // anti-aliasing, so a smooth rim in the accent color is drawn over its edge.
            let rad = 2 * (theme::radius(120.0) as f32 * sc).round() as i32;
            let brush = CreateSolidBrush(PANEL);
            let pen = CreatePen(PS_SOLID, 1, PANEL);
            let ob = SelectObject(dc, brush);
            let op = SelectObject(dc, pen);
            RoundRect(dc, frame.x + 1, frame.y + 1, frame.right() - 1, frame.bottom() - 1, rad, rad);
            SelectObject(dc, ob);
            SelectObject(dc, op);
            DeleteObject(brush);
            DeleteObject(pen);
            let clip = CreateRoundRectRgn(mx, my, mx + size + 1, my + size + 1, rad - 4, rad - 4);
            SelectClipRgn(dc, clip);
            SetStretchBltMode(dc, COLORONCOLOR);
            StretchBlt(dc, mx, my, size, size, s.bright.dc, cx - cells / 2, cy - cells / 2, cells, cells, SRCCOPY);
            let mid = IRect { x: mx + (cells / 2) * zoom, y: my + (cells / 2) * zoom, w: zoom, h: zoom };
            let pen = CreatePen(PS_SOLID, 1, ACCENT);
            let op = SelectObject(dc, pen);
            let ob = SelectObject(dc, GetStockObject(NULL_BRUSH));
            windows_sys::Win32::Graphics::Gdi::Rectangle(dc, mid.x, mid.y, mid.right(), mid.bottom());
            SelectObject(dc, op);
            SelectObject(dc, ob);
            DeleteObject(pen);
            SelectClipRgn(dc, std::ptr::null_mut());
            DeleteObject(clip);
            let rim_w = (2.5 * sc).max(2.0);
            if let Some(rim) = capture_bar::rim(frame.w, frame.h, rad as f32 / 2.0, theme::ACCENT, rim_w) {
                blend_pixmap(dc, &rim, frame.x, frame.y);
            }
        }
        regions.push(frame);
        let coords = format!("{}, {}", cx + s.origin.0, cy + s.origin.1);
        if let Some(label) = capture_bar::label(&coords, DARK.panel, Some(DARK.border), DARK.muted, sc, fonts) {
            let (lx, ly) = (mx, my + size + (6.0 * sc) as i32);
            if draw {
                blend_pixmap(dc, &label, lx, ly);
            }
            regions.push(IRect { x: lx, y: ly, w: label.width() as i32, h: label.height() as i32 });
        }
    }
    regions
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    with_state(|s| {
        let p = ps.rcPaint;
        let (w, h) = (p.right - p.left, p.bottom - p.top);
        let back = s.back.dc;
        BitBlt(back, p.left, p.top, w, h, s.dim.dc, p.left, p.top, SRCCOPY);
        // Text mode: the text that was read lights up on the dimmed screen.
        if let (Mode::Text, TextState::Ready(page)) = (s.mode, &s.text) {
            let paint_rect = IRect::from_ltrb(p.left, p.top, p.right, p.bottom);
            let pad = (2.0 * s.scale).round() as i32;
            for line in &page.lines {
                let r = line.rect();
                let r = IRect::from_ltrb(
                    r.x.floor() as i32 - pad,
                    r.y.floor() as i32 - pad,
                    r.right().ceil() as i32 + pad,
                    r.bottom().ceil() as i32 + pad,
                )
                .intersect(&paint_rect);
                if !r.is_empty() {
                    BitBlt(back, r.x, r.y, r.w, r.h, s.bright.dc, r.x, r.y, SRCCOPY);
                }
            }
        }
        decorations(s, back, true);
        BitBlt(hdc, p.left, p.top, w, h, back, p.left, p.top, SRCCOPY);
    });
    EndPaint(hwnd, &ps);
}

/// Invalidates the old and new decoration areas only (keeps 4K/multi-monitor smooth).
fn refresh(hwnd: HWND) {
    let all = with_state(|s| std::mem::take(&mut s.redraw_all)).unwrap_or(false);
    if all {
        unsafe {
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }
    let regions = with_state(|s| {
        let new = unsafe { decorations(s, s.back.dc, false) };
        let mut all = std::mem::replace(&mut s.dirty, new.clone());
        all.extend(new);
        all
    })
    .unwrap_or_default();
    for r in regions {
        let rc = RECT { left: r.x - 4, top: r.y - 4, right: r.right() + 4, bottom: r.bottom() + 4 };
        unsafe {
            InvalidateRect(hwnd, &rc, 0);
        }
    }
}

/// Switches mode: the selection follows (this screen, all screens, ...) and the mode bar
/// shows the new mode.
fn set_mode(s: &mut State, mode: Mode) {
    if s.mode != mode {
        // The lit-up text comes and goes with text mode.
        s.redraw_all = s.mode == Mode::Text || mode == Mode::Text;
        s.mode = mode;
        s.bar = capture_bar::Layout::new(mode, s.scale, capture_bar::fonts());
        if mode == Mode::Text {
            start_reading(s);
        }
    }
    update_hover(s);
}

fn finish(s: &mut State, rect: Option<IRect>) {
    if s.mode == Mode::Text {
        let Some(r) = rect.filter(|r| !r.is_empty()) else {
            s.result = Some(None);
            return;
        };
        // A click on text (not a drag) copies the lines it points at.
        let lines = if !s.dragging && Some(r) == s.hover { s.text_hover.clone() } else { None };
        return finish_text(s, r, lines);
    }
    s.result = Some(rect.filter(|r| !r.is_empty()).map(|r| Outcome {
        rect: IRect { x: r.x + s.origin.0, y: r.y + s.origin.1, ..r },
        scroll: s.mode == Mode::Scroll,
        text: None,
    }));
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_DPICHANGED => 0, // keep covering the whole virtual screen, never rescale
        TEXT_READ_MSG => {
            with_state(text_read);
            refresh(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            with_state(|s| {
                s.cur = (x, y);
                if s.bar_grab.is_some() {
                    drag_bar(s, x, y);
                    s.bar_hover = Some(Hit::Grip);
                } else if let Some((px, py)) = s.press {
                    if !s.dragging && ((x - px).abs() > 3 || (y - py).abs() > 3) {
                        s.dragging = true;
                    }
                    s.bar_hover = None;
                } else {
                    update_hover(s);
                    s.bar_hover = bar_at(s, x, y).flatten();
                }
            });
            refresh(hwnd);
            0
        }
        WM_SETCURSOR if (lp & 0xFFFF) as u32 == HTCLIENT => {
            // An arrow over the mode bar (it has buttons), the cross hair everywhere else.
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            let cursor = with_state(|s| {
                if s.waiting.is_some() {
                    return IDC_WAIT;
                }
                if s.bar_grab.is_some() {
                    return IDC_SIZEALL;
                }
                match if s.press.is_none() { bar_at(s, pt.x - s.origin.0, pt.y - s.origin.1) } else { None } {
                    Some(Some(Hit::Grip)) => IDC_SIZEALL,
                    Some(_) => IDC_ARROW,
                    None => IDC_CROSS,
                }
            })
            .unwrap_or(IDC_CROSS);
            SetCursor(LoadCursorW(std::ptr::null_mut(), cursor));
            1
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lp);
            // Waiting for the text of a pick: only Esc or a right-click (cancel) count now.
            if with_state(|s| s.waiting.is_some()).unwrap_or(false) {
                return 0;
            }
            // A click on the mode bar switches modes (or cancels) and never starts a capture;
            // on the grip it picks the bar up.
            let capture = with_state(|s| match bar_at(s, x, y) {
                Some(hit) => {
                    match hit {
                        Some(Hit::Mode(m)) => set_mode(s, m),
                        Some(Hit::Close) => s.result = Some(None),
                        Some(Hit::Grip) => {
                            let (bx, by) = bar_origin(s);
                            s.bar_grab = Some((x - bx, y - by));
                            s.bar_hover = Some(Hit::Grip);
                        }
                        None => {}
                    }
                    s.bar_grab.is_some()
                }
                None => {
                    s.press = Some((x, y));
                    s.dragging = false;
                    true
                }
            });
            // Hold on to the mouse for a capture drag and for a bar drag, so both carry on
            // when the pointer leaves the window.
            if capture == Some(true) {
                SetCapture(hwnd);
            }
            refresh(hwnd);
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            with_state(|s| {
                if s.bar_grab.take().is_some() {
                    // Let go of the bar; it stays where it is.
                    let (x, y) = s.cur;
                    s.bar_hover = bar_at(s, x, y).flatten();
                } else if s.press.is_some() {
                    let sel = selection(s);
                    finish(s, sel);
                }
            });
            // A text pick may wait for the reading: show that.
            refresh(hwnd);
            0
        }
        WM_RBUTTONUP => {
            with_state(|s| s.result = Some(None));
            0
        }
        WM_KEYDOWN => {
            let vk = wp as u32;
            let mut nudge = None;
            let waiting = with_state(|s| s.waiting.is_some()).unwrap_or(false);
            if waiting && vk != 0x1B {
                return 0;
            }
            with_state(|s| match vk {
                0x1B => s.result = Some(None), // Esc
                0x0D => {
                    let sel = selection(s);
                    finish(s, sel)
                }
                0x46 => {
                    let m = cursor_monitor_rect(s);
                    finish(s, Some(m))
                }
                0x41 => {
                    let all = client_bounds(s);
                    finish(s, Some(all))
                }
                0x53 => {
                    let next = if s.mode == Mode::Scroll { Mode::Region } else { Mode::Scroll };
                    set_mode(s, next);
                }
                0x54 => {
                    let next = if s.mode == Mode::Text { Mode::Region } else { Mode::Text };
                    set_mode(s, next);
                }
                // Space: next mode. Ignore auto-repeat so holding it down does not spin through.
                0x20 if lp & (1 << 30) == 0 => set_mode(s, s.mode.next()),
                0x11 => update_hover(s), // Ctrl pressed: inner panes, or a paragraph of text
                0x25 => nudge = Some((-1, 0)),
                0x26 => nudge = Some((0, -1)),
                0x27 => nudge = Some((1, 0)),
                0x28 => nudge = Some((0, 1)),
                _ => {}
            });
            if let Some((dx, dy)) = nudge {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                SetCursorPos(pt.x + dx, pt.y + dy);
            }
            refresh(hwnd);
            0
        }
        WM_KEYUP => {
            if wp as u32 == 0x11 {
                with_state(|s| update_hover(s));
                refresh(hwnd);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
