//! Icons for the tray menu: simple line icons on a 24 x 24 grid (the style of Fluent /
//! Lucide icons), rasterized at the menu's DPI and handed to Windows as 32-bit ARGB bitmaps.
//! The capture modes that are on the overlay's mode bar use the bar's own icons (see
//! `capture_bar::mode_icon`), so a mode looks the same in both places.

use crate::capture_bar::{self, Mode};
use eframe::egui::Color32;
use tiny_skia::{LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};
use windows_sys::Win32::Graphics::Gdi::{
    CreateDIBSection, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};

#[derive(Clone, Copy, Debug)]
pub enum Icon {
    Region,
    Fullscreen,
    AllScreens,
    Window,
    Scrolling,
    /// Copy text from the screen.
    Text,
    OpenFile,
    Clipboard,
    Folder,
    Settings,
    Exit,
    Update,
    Edit,
    Save,
    /// A sparkle, for "See what's new".
    WhatsNew,
    /// A clock, for History.
    History,
    /// Record the screen (the mode bar's icon).
    Record,
    /// Stop recording: a square.
    Stop,
    /// Pin a screenshot to the screen: a push pin.
    Pin,
}

fn rrect(pb: &mut PathBuilder, l: f32, t: f32, r: f32, b: f32, rad: f32) {
    pb.move_to(l + rad, t);
    pb.line_to(r - rad, t);
    pb.quad_to(r, t, r, t + rad);
    pb.line_to(r, b - rad);
    pb.quad_to(r, b, r - rad, b);
    pb.line_to(l + rad, b);
    pb.quad_to(l, b, l, b - rad);
    pb.line_to(l, t + rad);
    pb.quad_to(l, t, l + rad, t);
    pb.close();
}

fn line(pb: &mut PathBuilder, pts: &[(f32, f32)]) {
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
}

fn arc(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32, from_deg: f32, to_deg: f32) {
    let n = 24;
    for i in 0..=n {
        let a = (from_deg + (to_deg - from_deg) * i as f32 / n as f32).to_radians();
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
}

/// Path of an icon in 24 x 24 grid units (stroked, not filled).
fn path(icon: Icon) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    match icon {
        // Drawn by the mode bar (see `bitmap`).
        Icon::Region | Icon::Fullscreen | Icon::AllScreens | Icon::Scrolling | Icon::Text | Icon::Record => {
            return None
        }
        Icon::Stop => {
            rrect(&mut pb, 6.0, 6.0, 18.0, 18.0, 2.5);
        }
        Icon::Pin => {
            // A push pin: the cap, the body widening into its base, and the needle.
            rrect(&mut pb, 7.5, 2.5, 16.5, 5.5, 1.5);
            line(
                &mut pb,
                &[
                    (9.0, 5.5),
                    (9.0, 10.5),
                    (5.5, 14.0),
                    (5.5, 17.0),
                    (18.5, 17.0),
                    (18.5, 14.0),
                    (15.0, 10.5),
                    (15.0, 5.5),
                ],
            );
            line(&mut pb, &[(12.0, 17.0), (12.0, 22.0)]);
        }
        Icon::Window => {
            rrect(&mut pb, 2.0, 4.0, 22.0, 20.0, 2.0);
            line(&mut pb, &[(2.0, 9.0), (22.0, 9.0)]);
            line(&mut pb, &[(5.5, 6.5), (5.6, 6.5)]);
            line(&mut pb, &[(8.5, 6.5), (8.6, 6.5)]);
        }
        Icon::OpenFile => {
            rrect(&mut pb, 3.0, 3.0, 21.0, 21.0, 2.0);
            if let Some(c) = PathBuilder::from_circle(9.0, 9.0, 2.0) {
                pb.push_path(&c);
            }
            line(&mut pb, &[(21.0, 15.0), (16.0, 10.0), (5.0, 21.0)]);
        }
        Icon::Clipboard => {
            rrect(&mut pb, 4.0, 4.0, 20.0, 22.0, 2.0);
            rrect(&mut pb, 8.0, 2.0, 16.0, 6.0, 1.0);
            line(&mut pb, &[(8.0, 12.0), (16.0, 12.0)]);
            line(&mut pb, &[(8.0, 16.0), (13.5, 16.0)]);
        }
        Icon::Folder => {
            pb.move_to(4.0, 20.0);
            pb.line_to(20.0, 20.0);
            pb.quad_to(22.0, 20.0, 22.0, 18.0);
            pb.line_to(22.0, 9.0);
            pb.quad_to(22.0, 7.0, 20.0, 7.0);
            pb.line_to(12.5, 7.0);
            pb.line_to(10.0, 4.0);
            pb.line_to(4.0, 4.0);
            pb.quad_to(2.0, 4.0, 2.0, 6.0);
            pb.line_to(2.0, 18.0);
            pb.quad_to(2.0, 20.0, 4.0, 20.0);
            pb.close();
        }
        Icon::Settings => {
            // Gear: eight teeth around a ring, with a hole.
            let (cx, cy) = (12.0, 12.0);
            let teeth = 8;
            for i in 0..teeth * 4 {
                let a = (i as f32 / (teeth * 4) as f32 * 360.0 - 90.0 + 360.0 / (teeth * 8) as f32).to_radians();
                let r = if (i / 2) % 2 == 0 { 9.8 } else { 7.4 };
                let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
                if i == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
            pb.close();
            if let Some(c) = PathBuilder::from_circle(cx, cy, 3.0) {
                pb.push_path(&c);
            }
        }
        Icon::Update => {
            line(&mut pb, &[(12.0, 3.0), (12.0, 15.0)]);
            line(&mut pb, &[(7.0, 10.0), (12.0, 15.0), (17.0, 10.0)]);
            line(&mut pb, &[(4.0, 16.0), (4.0, 20.0), (20.0, 20.0), (20.0, 16.0)]);
        }
        Icon::Edit => {
            // Pencil.
            line(&mut pb, &[(4.0, 20.0), (4.5, 15.5), (15.5, 4.5), (19.5, 8.5), (8.5, 19.5), (4.0, 20.0)]);
            line(&mut pb, &[(13.0, 7.0), (17.0, 11.0)]);
        }
        Icon::Save => {
            // Floppy disk.
            pb.move_to(5.0, 3.0);
            pb.line_to(16.5, 3.0);
            pb.line_to(21.0, 7.5);
            pb.line_to(21.0, 19.0);
            pb.quad_to(21.0, 21.0, 19.0, 21.0);
            pb.line_to(5.0, 21.0);
            pb.quad_to(3.0, 21.0, 3.0, 19.0);
            pb.line_to(3.0, 5.0);
            pb.quad_to(3.0, 3.0, 5.0, 3.0);
            pb.close();
            line(&mut pb, &[(7.5, 3.0), (7.5, 8.0), (15.0, 8.0), (15.0, 3.0)]);
            rrect(&mut pb, 7.0, 13.0, 17.0, 21.0, 1.0);
        }
        Icon::WhatsNew => {
            // A four-pointed sparkle with a small one beside it.
            let star = |pb: &mut PathBuilder, cx: f32, cy: f32, r: f32| {
                let k = r * 0.28;
                pb.move_to(cx, cy - r);
                pb.quad_to(cx + k, cy - k, cx + r, cy);
                pb.quad_to(cx + k, cy + k, cx, cy + r);
                pb.quad_to(cx - k, cy + k, cx - r, cy);
                pb.quad_to(cx - k, cy - k, cx, cy - r);
                pb.close();
            };
            star(&mut pb, 10.0, 13.0, 8.0);
            star(&mut pb, 19.0, 5.0, 3.0);
        }
        Icon::Exit => {
            line(&mut pb, &[(12.0, 2.5), (12.0, 11.5)]);
            arc(&mut pb, 12.0, 13.0, 8.5, -52.0, 232.0);
        }
        Icon::History => {
            // A clock whose rim runs back into an arrow (like Lucide's "history").
            arc(&mut pb, 12.0, 12.0, 8.5, 180.0, -132.0);
            pb.line_to(3.5, 8.5);
            line(&mut pb, &[(3.5, 3.5), (3.5, 8.5), (8.5, 8.5)]);
            line(&mut pb, &[(12.0, 7.5), (12.0, 12.0), (15.5, 14.0)]);
        }
    }
    pb.finish()
}

/// Renders an icon at `size` pixels into a premultiplied 32-bit bottom-up DIB for menus.
pub fn bitmap(icon: Icon, size: u32, color: Color32) -> HBITMAP {
    let size = size.max(8);
    let Some(mut pm) = Pixmap::new(size, size) else { return std::ptr::null_mut() };
    let mode = match icon {
        Icon::Region => Some(Mode::Region),
        Icon::Fullscreen => Some(Mode::Screen),
        Icon::AllScreens => Some(Mode::AllScreens),
        Icon::Scrolling => Some(Mode::Scroll),
        Icon::Text => Some(Mode::Text),
        Icon::Record => Some(Mode::Record),
        _ => None,
    };
    if let Some(mode) = mode {
        // The same line width as the other menu icons: 1.9 of 24 is 1.425 of 18.
        capture_bar::mode_icon(&mut pm, mode, 0.0, 0.0, size as f32, color, 1.9 * 18.0 / 24.0);
    } else if let Some(p) = path(icon) {
        let s = size as f32 / 24.0;
        let mut paint = Paint::default();
        let [r, g, b, a] = color.to_srgba_unmultiplied();
        paint.set_color_rgba8(r, g, b, a);
        paint.anti_alias = true;
        let stroke = Stroke { width: 1.9, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
        pm.stroke_path(&p, &paint, &stroke, Transform::from_scale(s, s), None);
    }
    dib(&pm)
}

/// A screenshot preview for the History menu: fitted in a box of `w` x `h` pixels, centered,
/// with a thin outline in `color` so light screenshots stand out on a light menu.
pub fn thumbnail(img: &image::RgbaImage, w: u32, h: u32, color: Color32) -> HBITMAP {
    let (w, h) = (w.max(8), h.max(8));
    let Some(mut pm) = Pixmap::new(w, h) else { return std::ptr::null_mut() };
    let s = ((w - 2) as f32 / img.width() as f32).min((h - 2) as f32 / img.height() as f32);
    let tw = ((img.width() as f32 * s).round() as u32).clamp(1, w - 2);
    let th = ((img.height() as f32 * s).round() as u32).clamp(1, h - 2);
    let small = image::imageops::resize(img, tw, th, image::imageops::FilterType::Triangle);
    let (ox, oy) = ((w - tw) / 2, (h - th) / 2);
    let data = pm.data_mut();
    for (x, y, p) in small.enumerate_pixels() {
        let i = (((oy + y) * w + ox + x) * 4) as usize;
        let a = p[3] as u32;
        let pre = |c: u8| (c as u32 * a / 255) as u8;
        data[i..i + 4].copy_from_slice(&[pre(p[0]), pre(p[1]), pre(p[2]), p[3]]);
    }
    let mut pb = PathBuilder::new();
    pb.push_rect(
        tiny_skia::Rect::from_xywh(ox as f32 - 0.5, oy as f32 - 0.5, tw as f32 + 1.0, th as f32 + 1.0).unwrap(),
    );
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        let [r, g, b, _] = color.to_srgba_unmultiplied();
        paint.set_color_rgba8(r, g, b, 90);
        pm.stroke_path(&path, &paint, &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
    }
    dib(&pm)
}

/// The tray icon while recording: a red dot with a thin dark rim, so it stands out on a light
/// and on a dark taskbar.
pub fn recording_icon(size: u32) -> windows_sys::Win32::UI::WindowsAndMessaging::HICON {
    use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};
    let size = size.max(16);
    let Some(mut pm) = Pixmap::new(size, size) else { return std::ptr::null_mut() };
    let c = size as f32 / 2.0;
    let mut pb = PathBuilder::new();
    pb.push_circle(c, c, size as f32 * 0.36);
    if let Some(dot) = pb.finish() {
        let mut paint = Paint::default();
        let r = crate::theme::RECORD;
        paint.set_color_rgba8(r.r(), r.g(), r.b(), 255);
        paint.anti_alias = true;
        pm.fill_path(&dot, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
        paint.set_color_rgba8(0, 0, 0, 90);
        let stroke = Stroke { width: (size as f32 / 16.0).max(1.0), ..Default::default() };
        pm.stroke_path(&dot, &paint, &stroke, Transform::identity(), None);
    }
    unsafe {
        let color = dib(&pm);
        // Every pixel's transparency comes from the color bitmap; the mask only has to exist.
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, std::ptr::null());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

/// A premultiplied 32-bit bottom-up DIB (what menus expect) of a pixmap.
fn dib(pm: &Pixmap) -> HBITMAP {
    let (w, h) = (pm.width(), pm.height());
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: h as i32, // bottom-up: what menus expect
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let screen = GetDC(std::ptr::null_mut());
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(std::ptr::null_mut(), screen);
        if bmp.is_null() || bits.is_null() {
            return std::ptr::null_mut();
        }
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        let row = w as usize * 4;
        for (y, src) in pm.data().chunks_exact(row).enumerate() {
            let d = &mut dst[(h as usize - 1 - y) * row..][..row];
            for (o, p) in d.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                o.copy_from_slice(&[p[2], p[1], p[0], p[3]]); // RGBA -> BGRA (premultiplied)
            }
        }
        bmp
    }
}
