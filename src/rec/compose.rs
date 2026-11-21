//! Draws the pointer, its highlight, click highlights and keys over a frame of a recording.
//!
//! The video is recorded without the pointer; this puts it back when a recording is exported
//! or previewed, from the event log, so each part can be switched off afterwards. The same
//! code draws the clip window's preview and the export, so what you see is what you get.

use super::events::{Button, Frame};
use crate::capture_bar::{draw_text, fill, rrect, text_width, Area, Fonts};
use crate::theme;
use eframe::egui::Color32;
use image::RgbaImage;
use std::collections::HashMap;
use std::path::Path;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PixmapMut, Stroke, Transform};

/// What to draw over the video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlays {
    pub pointer: bool,
    /// A soft circle around the pointer.
    pub pointer_highlight: bool,
    /// A ring where a mouse button was pressed (left and right in different colors).
    pub clicks: bool,
    /// The keys and shortcuts at the bottom (if the recording logged them).
    pub keys: bool,
}

impl Default for Overlays {
    fn default() -> Self {
        Self { pointer: true, pointer_highlight: false, clicks: true, keys: true }
    }
}

impl Overlays {
    #[cfg(test)]
    pub fn none() -> Self {
        Self { pointer: false, pointer_highlight: false, clicks: false, keys: false }
    }
    pub fn any(&self) -> bool {
        self.pointer || self.pointer_highlight || self.clicks || self.keys
    }
}

/// The pointer pictures of a recording (`cursors/<id>.png`), by id.
#[derive(Clone, Debug, Default)]
pub struct Cursors(pub HashMap<u32, RgbaImage>);

impl Cursors {
    pub fn load(dir: &Path) -> Self {
        let mut map = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let path = e.path();
                let id = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok());
                if let (Some(id), Some("png")) = (id, path.extension().and_then(|s| s.to_str())) {
                    if let Ok(img) = image::open(&path) {
                        map.insert(id, img.to_rgba8());
                    }
                }
            }
        }
        Cursors(map)
    }
}

/// Sizes of the highlights relative to the screen: `dpi` is the display scale of the recorded
/// monitor times the video's zoom (video pixels per screen pixel).
pub struct Style {
    pub dpi: f32,
    pub zoom: f32,
}

fn color(c: u32, alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
    p.anti_alias = true;
    p
}

fn circle(pix: &mut PixmapMut, x: f32, y: f32, r: f32, fill_paint: Option<Paint>, ring: Option<(Paint, f32)>) {
    let mut pb = PathBuilder::new();
    pb.push_circle(x, y, r.max(0.5));
    let Some(path) = pb.finish() else { return };
    if let Some(p) = fill_paint {
        pix.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
    if let Some((p, width)) = ring {
        let s = Stroke { width, ..Stroke::default() };
        pix.stroke_path(&path, &p, &s, Transform::identity(), None);
    }
}

/// Blends a straight-alpha picture onto the (opaque) frame at `(x, y)`, scaled by `zoom`.
fn blend_image(frame: &mut RgbaImage, img: &RgbaImage, x: i32, y: i32, zoom: f32) {
    let scaled;
    let img = if (zoom - 1.0).abs() > 0.01 {
        let w = ((img.width() as f32 * zoom).round() as u32).max(1);
        let h = ((img.height() as f32 * zoom).round() as u32).max(1);
        scaled = image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle);
        &scaled
    } else {
        img
    };
    let (fw, fh) = (frame.width() as i32, frame.height() as i32);
    for (sx, sy, p) in img.enumerate_pixels() {
        let (dx, dy) = (x + sx as i32, y + sy as i32);
        if dx < 0 || dy < 0 || dx >= fw || dy >= fh || p[3] == 0 {
            continue;
        }
        let d = frame.get_pixel_mut(dx as u32, dy as u32);
        let a = p[3] as u32;
        for i in 0..3 {
            d[i] = ((p[i] as u32 * a + d[i] as u32 * (255 - a) + 127) / 255) as u8;
        }
        d[3] = 255;
    }
}

/// Blends a premultiplied pixmap onto the (opaque) frame at `(x, y)` with `alpha`.
fn blend_pixmap(frame: &mut RgbaImage, pix: &Pixmap, x: i32, y: i32, alpha: f32) {
    let (fw, fh) = (frame.width() as i32, frame.height() as i32);
    let k = (alpha.clamp(0.0, 1.0) * 256.0) as u32;
    for py in 0..pix.height() as i32 {
        for px in 0..pix.width() as i32 {
            let (dx, dy) = (x + px, y + py);
            if dx < 0 || dy < 0 || dx >= fw || dy >= fh {
                continue;
            }
            let Some(s) = pix.pixel(px as u32, py as u32) else { continue };
            let sa = s.alpha() as u32 * k / 256;
            if sa == 0 {
                continue;
            }
            let d = frame.get_pixel_mut(dx as u32, dy as u32);
            let src = [s.red() as u32 * k / 256, s.green() as u32 * k / 256, s.blue() as u32 * k / 256];
            for i in 0..3 {
                d[i] = (src[i] + (d[i] as u32 * (255 - sa) + 127) / 255).min(255) as u8;
            }
        }
    }
}

/// One rounded label with the keys, as on a keyboard: light text on a dark panel.
fn key_pill(text: &str, em: f32, fonts: &Fonts) -> Option<Pixmap> {
    let font = &fonts.semibold;
    let tw = text_width(font, em, text);
    let h = (em * 2.1).round();
    let a = Area::new(0.0, 0.0, (tw + em * 1.6).round(), h);
    let mut pix = Pixmap::new(a.w as u32, a.h as u32)?;
    fill(&mut pix, rrect(a, [theme::radius(h) as f32 * em / 13.0; 4]), theme::INK, 0.88);
    let line = (em / 18.0).max(1.0);
    let border = theme::DARK.border;
    if let Some(path) =
        rrect(a.inset(line / 2.0, line / 2.0, line / 2.0, line / 2.0), [theme::radius(h) as f32 * em / 13.0; 4])
    {
        let mut p = Paint::default();
        p.set_color_rgba8(border.r(), border.g(), border.b(), 255);
        p.anti_alias = true;
        pix.stroke_path(&path, &p, &Stroke { width: line, ..Stroke::default() }, Transform::identity(), None);
    }
    draw_text(&mut pix, font, em, (a.w - tw) / 2.0, a.cy(), Color32::WHITE, text);
    Some(pix)
}

/// Draws `f` over `frame` (opaque RGBA, the size of the video).
pub fn draw(frame: &mut RgbaImage, f: &Frame, cursors: &Cursors, opts: &Overlays, style: &Style, fonts: &Fonts) {
    let s = style.dpi.clamp(0.5, 4.0);
    let (w, h) = (frame.width(), frame.height());
    {
        let Some(mut pix) = PixmapMut::from_bytes(frame.as_mut(), w, h) else { return };
        // The frame is opaque, so its straight RGBA is also valid premultiplied RGBA.
        if let (true, Some(p)) = (opts.pointer_highlight, f.pointer) {
            let (x, y) = (p.x as f32, p.y as f32);
            circle(&mut pix, x, y, 24.0 * s, Some(color(theme::hex::REC_HIGHLIGHT, 0.38)), None);
        }
        if opts.clicks {
            for c in &f.clicks {
                let k = (c.age as f32 / super::events::CLICK_MS as f32).clamp(0.0, 1.0);
                let ease = 1.0 - (1.0 - k) * (1.0 - k);
                let hex = match c.button {
                    Button::Left => theme::hex::REC_CLICK_LEFT,
                    Button::Right => theme::hex::REC_CLICK_RIGHT,
                    Button::Middle => theme::hex::REC_HIGHLIGHT,
                };
                let r = (7.0 + 23.0 * ease) * s;
                let fade = 1.0 - k;
                circle(
                    &mut pix,
                    c.x as f32,
                    c.y as f32,
                    r,
                    Some(color(hex, 0.28 * fade)),
                    Some((color(hex, 0.95 * fade), 3.0 * s)),
                );
            }
        }
    }
    if let (true, Some(p)) = (opts.pointer, f.pointer) {
        if let Some(img) = cursors.0.get(&p.id) {
            let z = style.zoom;
            let x = (p.x as f32 - p.hx as f32 * z).round() as i32;
            let y = (p.y as f32 - p.hy as f32 * z).round() as i32;
            blend_image(frame, img, x, y, z);
        }
    }
    if opts.keys && !f.keys.is_empty() {
        // Smaller on a small video, so a few keys still fit.
        let mut em = 19.0 * s;
        let gap = 8.0 * s;
        let width = |pills: &[(Pixmap, f32)]| {
            pills.iter().map(|(p, _)| p.width() as f32).sum::<f32>() + gap * pills.len().saturating_sub(1) as f32
        };
        let mut pills = loop {
            let pills: Vec<_> =
                f.keys.iter().filter_map(|k| key_pill(&k.label, em, fonts).map(|p| (p, k.alpha()))).collect();
            if width(&pills) <= w as f32 * 0.9 || em < 9.0 {
                break pills;
            }
            em *= 0.8;
        };
        // Still too wide at the smallest size: the oldest labels go first.
        while pills.len() > 1 && width(&pills) > w as f32 * 0.9 {
            pills.remove(0);
        }
        let total = width(&pills);
        let mut x = (w as f32 - total) / 2.0;
        for (pix, alpha) in &pills {
            let y = h as f32 - pix.height() as f32 - 28.0 * s.min(h as f32 / 400.0).max(0.4);
            blend_pixmap(frame, pix, x.round() as i32, y.round() as i32, *alpha);
            x += pix.width() as f32 + gap;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rec::events::{Click, KeyLabel, Pointer};

    const GREY: [u8; 4] = [90, 90, 90, 255];

    fn frame() -> RgbaImage {
        RgbaImage::from_pixel(400, 300, image::Rgba(GREY))
    }

    fn arrow() -> Cursors {
        // A 6 x 6 black square with a white dot as hotspot at (1, 1).
        let mut img = RgbaImage::from_pixel(6, 6, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        Cursors(HashMap::from([(7, img)]))
    }

    fn style() -> Style {
        Style { dpi: 1.0, zoom: 1.0 }
    }

    fn changed(a: &RgbaImage, b: &RgbaImage) -> Vec<(u32, u32)> {
        a.enumerate_pixels().filter(|(x, y, p)| *p != b.get_pixel(*x, *y)).map(|(x, y, _)| (x, y)).collect()
    }

    #[test]
    fn draws_the_pointer_at_its_hotspot() {
        let f = Frame { pointer: Some(Pointer { x: 100, y: 50, id: 7, hx: 1, hy: 1 }), ..Frame::default() };
        let mut img = frame();
        let fonts = crate::capture_bar::fonts();
        draw(&mut img, &f, &arrow(), &Overlays::default(), &style(), fonts);
        assert_eq!(img.get_pixel(100, 50).0, [255, 255, 255, 255], "the hotspot lands on the position");
        assert_eq!(img.get_pixel(99, 49).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(105, 55).0, GREY, "nothing beyond the picture");
        // Switched off: nothing changes. An unknown picture: nothing either.
        let mut off = frame();
        draw(&mut off, &f, &arrow(), &Overlays { pointer: false, ..Overlays::default() }, &style(), fonts);
        assert!(changed(&off, &frame()).is_empty());
        let mut unknown = frame();
        let f2 = Frame { pointer: Some(Pointer { id: 99, ..f.pointer.unwrap() }), ..Frame::default() };
        draw(&mut unknown, &f2, &arrow(), &Overlays::default(), &style(), fonts);
        assert!(changed(&unknown, &frame()).is_empty());
    }

    #[test]
    fn a_pointer_on_the_edge_is_clipped() {
        let f = Frame { pointer: Some(Pointer { x: 399, y: 299, id: 7, hx: 0, hy: 0 }), ..Frame::default() };
        let mut img = frame();
        draw(&mut img, &f, &arrow(), &Overlays::default(), &style(), crate::capture_bar::fonts());
        assert_eq!(img.get_pixel(399, 299).0, [0, 0, 0, 255]);
        let f = Frame { pointer: Some(Pointer { x: -50, y: -50, id: 7, hx: 0, hy: 0 }), ..Frame::default() };
        let mut img = frame();
        draw(&mut img, &f, &arrow(), &Overlays::default(), &style(), crate::capture_bar::fonts());
        assert!(changed(&img, &frame()).is_empty());
    }

    #[test]
    fn clicks_are_rings_around_the_spot_in_their_own_color() {
        let fonts = crate::capture_bar::fonts();
        let ring = |button| {
            let f = Frame { clicks: vec![Click { x: 200, y: 150, button, age: 120 }], ..Frame::default() };
            let mut img = frame();
            draw(&mut img, &f, &Cursors::default(), &Overlays::default(), &style(), fonts);
            img
        };
        let left = ring(Button::Left);
        let right = ring(Button::Right);
        let touched = changed(&left, &frame());
        assert!(!touched.is_empty());
        // Everything drawn stays within the largest ring around the click.
        assert!(touched.iter().all(|(x, y)| (*x as i32 - 200).abs() <= 33 && (*y as i32 - 150).abs() <= 33));
        // Left and right differ.
        assert_ne!(left, right);
        // Switched off: nothing.
        let f = Frame { clicks: vec![Click { x: 200, y: 150, button: Button::Left, age: 120 }], ..Frame::default() };
        let mut img = frame();
        draw(&mut img, &f, &Cursors::default(), &Overlays { clicks: false, ..Overlays::default() }, &style(), fonts);
        assert!(changed(&img, &frame()).is_empty());
    }

    #[test]
    fn the_pointer_highlight_is_a_soft_circle() {
        let f = Frame { pointer: Some(Pointer { x: 50, y: 60, id: 0, hx: 0, hy: 0 }), ..Frame::default() };
        let opts = Overlays { pointer: false, pointer_highlight: true, ..Overlays::default() };
        let mut img = frame();
        draw(&mut img, &f, &Cursors::default(), &opts, &style(), crate::capture_bar::fonts());
        let c = img.get_pixel(50, 60).0;
        assert_ne!(c, GREY);
        assert!(c[0] > GREY[0] && c[1] > GREY[1], "lighter, yellowish: {c:?}");
        assert_eq!(img.get_pixel(50 + 30, 60).0, GREY);
    }

    #[test]
    fn keys_show_at_the_bottom_center_and_fade() {
        let fonts = crate::capture_bar::fonts();
        let keys = |age| Frame { keys: vec![KeyLabel { label: "Ctrl + C".into(), age }], ..Frame::default() };
        let mut img = frame();
        draw(&mut img, &keys(10), &Cursors::default(), &Overlays::default(), &style(), fonts);
        let touched = changed(&img, &frame());
        assert!(!touched.is_empty());
        assert!(touched.iter().all(|(_, y)| *y > 200), "in the bottom third");
        let (min_x, max_x) = touched.iter().fold((u32::MAX, 0), |(a, b), (x, _)| (a.min(*x), b.max(*x)));
        assert!((200 - min_x as i32 - (max_x as i32 - 200)).abs() <= 2, "centered: {min_x}..{max_x}");
        // Half faded: closer to the background than fully shown.
        let mut half = frame();
        let age = crate::rec::events::KEY_SHOW_MS + crate::rec::events::KEY_FADE_MS / 2;
        draw(&mut half, &keys(age), &Cursors::default(), &Overlays::default(), &style(), fonts);
        let (x, y) = touched[touched.len() / 2];
        let d = |img: &RgbaImage| (img.get_pixel(x, y)[0] as i32 - GREY[0] as i32).abs();
        assert!(d(&half) < d(&img));
        // Off: nothing.
        let mut off = frame();
        draw(
            &mut off,
            &keys(10),
            &Cursors::default(),
            &Overlays { keys: false, ..Overlays::default() },
            &style(),
            fonts,
        );
        assert!(changed(&off, &frame()).is_empty());
    }

    #[test]
    fn many_keys_shrink_to_fit_a_small_video() {
        let fonts = crate::capture_bar::fonts();
        let f = Frame {
            keys: (0..3).map(|i| KeyLabel { label: format!("Ctrl + Shift + Alt + F{i}"), age: 0 }).collect(),
            ..Frame::default()
        };
        let mut img = RgbaImage::from_pixel(320, 180, image::Rgba(GREY));
        draw(&mut img, &f, &Cursors::default(), &Overlays::default(), &Style { dpi: 2.0, zoom: 1.0 }, fonts);
        let blank = RgbaImage::from_pixel(320, 180, image::Rgba(GREY));
        let touched = changed(&img, &blank);
        assert!(!touched.is_empty());
        assert!(touched.iter().all(|(x, _)| *x >= 8 && *x < 312), "fits inside the frame");
    }

    #[test]
    fn loads_cursor_pictures_by_id() {
        let dir = std::env::temp_dir().join(format!("blink-cursors-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        RgbaImage::from_pixel(3, 4, image::Rgba([1, 2, 3, 255])).save(dir.join("12.png")).unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let c = Cursors::load(&dir);
        assert_eq!(c.0.len(), 1);
        assert_eq!(c.0[&12].dimensions(), (3, 4));
        std::fs::remove_dir_all(&dir).ok();
    }
}
