//! The bar on top of a pinned screenshot: On top, Copy, Save as, Edit on the left; the zoom,
//! the opacity and close on the right. When the window is narrow, the least used items make
//! room first (they are also in the right-click menu).
//!
//! Drawn with anti-aliasing into a pixmap like the capture overlay's mode bar
//! (`capture_bar`), so the Windows pin window only has to copy it to the screen. The layout
//! does not depend on Windows: it is tested here, and previewed with
//! `BLINK_PIN_PREVIEW=<dir> cargo test pin_bar -- --nocapture`.

use crate::capture_bar::{draw_text, fill, rrect, stroke, text_width, Area, Fonts};
use crate::i18n::t;
use crate::theme::{self, pal};
use tiny_skia::{PathBuilder, Pixmap};

/// Height of the bar, in points.
const BAR: f32 = 36.0;
/// Space before the first and after the last item.
const EDGE: f32 = 8.0;
/// Space between two items: enough that the bar does not feel cramped.
const GAP: f32 = 8.0;
const BUTTON_H: f32 = 26.0;
/// Text padding inside a button.
const BUTTON_PAD: f32 = 12.0;
const EM: f32 = 13.0;
const BOX: f32 = 15.0;

/// Opacities offered, in percent.
pub const OPACITIES: [u8; 4] = [100, 80, 60, 40];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    OnTop,
    Copy,
    SaveAs,
    Edit,
    /// Shows the zoom; a click goes back to 100%.
    Zoom,
    Opacity,
    Close,
}

/// What the bar shows, besides its items.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub on_top: bool,
    /// Percent; `None` where the opacity cannot be changed.
    pub opacity: Option<u8>,
    /// Percent.
    pub zoom: u32,
    pub hover: Option<Item>,
    pub pressed: Option<Item>,
    /// "Copied" shows on the Copy button for a moment.
    pub copied: bool,
}

/// The bar's height in pixels at `scale` (pixels per point): whole pixels, so the image under
/// it starts on a pixel and 100% is sharp.
pub fn height(scale: f32) -> i32 {
    (BAR * scale).round() as i32
}

/// The narrowest window, in pixels: room for On top, Copy and close.
pub fn min_width(scale: f32) -> i32 {
    (260.0 * scale).round() as i32
}

/// Where each item is, in pixels of the bar.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub items: Vec<(Item, Area)>,
}

impl Layout {
    pub fn hit(&self, x: f32, y: f32) -> Option<Item> {
        self.items.iter().find(|(_, a)| a.contains(x, y)).map(|(i, _)| *i)
    }
}

fn label(item: Item, st: &State) -> String {
    match item {
        Item::OnTop => t("On top").into(),
        Item::Copy if st.copied => t("Copied").into(),
        Item::Copy => t("Copy").into(),
        Item::SaveAs => t("Save as...").into(),
        Item::Edit => t("Edit").into(),
        Item::Zoom => format!("{}%", st.zoom),
        Item::Opacity => format!("{}%", st.opacity.unwrap_or(100)),
        Item::Close => String::new(),
    }
}

fn item_width(item: Item, st: &State, fonts: &Fonts, scale: f32) -> f32 {
    let text = |s: &str| text_width(&fonts.regular, EM * scale, s);
    match item {
        Item::OnTop => (BOX + 7.0 + 8.0) * scale + text(&label(item, st)),
        // As wide as "Copied", so the bar does not move when it shows.
        Item::Copy => 2.0 * BUTTON_PAD * scale + text(t("Copy")).max(text(t("Copied"))),
        Item::SaveAs | Item::Edit => 2.0 * BUTTON_PAD * scale + text(&label(item, st)),
        Item::Zoom => 16.0 * scale + text("100%"),
        // A half-filled circle, the text and a small arrow.
        Item::Opacity => (2.0 * BUTTON_PAD + 12.0 + 18.0) * scale + text("100%"),
        Item::Close => BUTTON_H * scale,
    }
}

/// Lays out the bar for a window `width` pixels wide.
pub fn layout(width: f32, scale: f32, st: &State, fonts: &Fonts) -> Layout {
    let h = height(scale) as f32;
    let bh = (BUTTON_H * scale).round();
    let y = ((h - bh) / 2.0).round();
    let mut left = vec![Item::OnTop, Item::Copy, Item::SaveAs, Item::Edit];
    let mut right = vec![Item::Close, Item::Zoom];
    if st.opacity.is_some() {
        right.insert(1, Item::Opacity);
    }
    let need = |left: &[Item], right: &[Item]| {
        let n = left.len() + right.len();
        let items: f32 = left.iter().chain(right).map(|&i| item_width(i, st, fonts, scale)).sum();
        2.0 * EDGE * scale + items + (n.saturating_sub(1) as f32 + 1.0) * GAP * scale
    };
    // Least used first: they are also in the right-click menu.
    for drop in [Item::Zoom, Item::Opacity, Item::Edit, Item::SaveAs] {
        if need(&left, &right) <= width {
            break;
        }
        left.retain(|&i| i != drop);
        right.retain(|&i| i != drop);
    }
    let mut items = Vec::new();
    let mut x = EDGE * scale;
    for i in left {
        let w = item_width(i, st, fonts, scale);
        items.push((i, Area::new(x, y, w, bh)));
        x += w + GAP * scale;
    }
    let mut x = width - EDGE * scale;
    for i in right {
        let w = item_width(i, st, fonts, scale);
        x -= w;
        items.push((i, Area::new(x, y, w, bh)));
        x -= GAP * scale;
    }
    Layout { items }
}

/// Where the image goes in an area of `aw` x `ah` pixels below the bar: as large as fits,
/// never stretched, centred. `(x, y, w, h)`, `y` from the top of that area.
pub fn fit(aw: i32, ah: i32, w: u32, h: u32) -> (i32, i32, i32, i32) {
    let k = (aw as f32 / w.max(1) as f32).min(ah as f32 / h.max(1) as f32);
    let (iw, ih) = (((w as f32 * k).round() as i32).max(1), ((h as f32 * k).round() as i32).max(1));
    ((aw - iw) / 2, (ah - ih) / 2, iw, ih)
}

/// Half the size of a BGRA picture (`w` x `h`, rows top down), each pixel the average of the
/// 2 x 2 it covers (an odd last row or column is averaged with itself). For resizing a pin
/// smoothly: stretching from a picture at most twice the size on screen is fast and still
/// looks right, where shrinking a large screenshot in one go is slow or grainy.
pub fn half_bgra(src: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let (hw, hh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u8; hw * hh * 4];
    for y in 0..hh {
        let (r0, r1) = (2 * y, (2 * y + 1).min(h - 1));
        for x in 0..hw {
            let (c0, c1) = (2 * x, (2 * x + 1).min(w - 1));
            for c in 0..4 {
                let px = |r: usize, col: usize| src[(r * w + col) * 4 + c] as u32;
                let sum = px(r0, c0) + px(r0, c1) + px(r1, c0) + px(r1, c1);
                out[(y * hw + x) * 4 + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    (out, hw as u32, hh as u32)
}

/// Which picture of a pyramid (0 = the screenshot, each next one half the size) to stretch
/// to `tw` x `th` on screen: the smallest that is still at least that large, so it is never
/// enlarged unless the screenshot itself is.
pub fn pyramid_level(w: u32, h: u32, tw: i32, th: i32, levels: usize) -> usize {
    let (mut lw, mut lh, mut level) = (w.max(1), h.max(1), 0);
    while level + 1 < levels && lw.div_ceil(2) as i32 >= tw && lh.div_ceil(2) as i32 >= th {
        lw = lw.div_ceil(2);
        lh = lh.div_ceil(2);
        level += 1;
    }
    level
}

/// Draws the bar, `width` pixels wide.
pub fn render(l: &Layout, width: u32, scale: f32, st: &State, fonts: &Fonts) -> Option<Pixmap> {
    let h = height(scale) as u32;
    let mut pix = Pixmap::new(width.max(1), h.max(1))?;
    let p = pal();
    fill(&mut pix, rrect(Area { x: 0.0, y: 0.0, w: width as f32, h: h as f32 }, [0.0; 4]), p.panel, 1.0);
    let radius = theme::radius(BUTTON_H * scale) as f32;
    let em = EM * scale;
    for &(item, a) in &l.items {
        let hover = st.hover == Some(item);
        let pressed = hover && st.pressed == Some(item);
        let bg = if pressed {
            Some(p.control_active)
        } else if hover {
            Some(p.control_hover)
        } else {
            match item {
                Item::Copy | Item::SaveAs | Item::Edit | Item::Opacity => Some(p.control),
                _ => None,
            }
        };
        if let Some(bg) = bg {
            fill(&mut pix, rrect(a, [radius; 4]), bg, 1.0);
        }
        let text = label(item, st);
        match item {
            Item::OnTop => {
                let s = (BOX * scale).round();
                let b = Area::new(a.x + 6.0 * scale, a.cy() - s / 2.0, s, s);
                let r = 4.0 * scale;
                if st.on_top {
                    fill(&mut pix, rrect(b, [r; 4]), theme::ACCENT, 1.0);
                    let mut pb = PathBuilder::new();
                    pb.move_to(b.x + s * 0.25, b.y + s * 0.52);
                    pb.line_to(b.x + s * 0.43, b.y + s * 0.70);
                    pb.line_to(b.x + s * 0.76, b.y + s * 0.32);
                    stroke(&mut pix, pb.finish(), theme::INK, 1.8 * scale);
                } else {
                    fill(&mut pix, rrect(b, [r; 4]), p.input_bg, 1.0);
                    let inner = Area { x: b.x + 0.5, y: b.y + 0.5, w: b.w - 1.0, h: b.h - 1.0 };
                    stroke(&mut pix, rrect(inner, [r; 4]), p.muted, 1.0 * scale);
                }
                draw_text(&mut pix, &fonts.regular, em, b.x + s + 7.0 * scale, a.cy(), p.text, &text);
            }
            Item::Close => {
                let c = if hover { p.text } else { p.muted };
                let d = 4.5 * scale;
                let (cx, cy) = (a.x + a.w / 2.0, a.cy());
                let mut pb = PathBuilder::new();
                pb.move_to(cx - d, cy - d);
                pb.line_to(cx + d, cy + d);
                pb.move_to(cx - d, cy + d);
                pb.line_to(cx + d, cy - d);
                stroke(&mut pix, pb.finish(), c, 1.5 * scale);
            }
            Item::Zoom => {
                let c = if hover { p.text } else { p.muted };
                let w = text_width(&fonts.regular, em, &text);
                draw_text(&mut pix, &fonts.regular, em, a.x + (a.w - w) / 2.0, a.cy(), c, &text);
            }
            Item::Opacity => {
                // A circle, half filled: see-through.
                let (cx, r) = (a.x + (BUTTON_PAD + 6.0) * scale, 6.0 * scale);
                stroke(&mut pix, PathBuilder::from_circle(cx, a.cy(), r), p.text, 1.3 * scale);
                let mut half = PathBuilder::new();
                half.move_to(cx, a.cy() - r);
                for i in 0..=12 {
                    let ang = -std::f32::consts::FRAC_PI_2 - std::f32::consts::PI * i as f32 / 12.0;
                    half.line_to(cx + r * ang.cos(), a.cy() + r * ang.sin());
                }
                half.close();
                fill(&mut pix, half.finish(), p.text, 1.0);
                let x = a.x + (BUTTON_PAD + 18.0) * scale;
                draw_text(&mut pix, &fonts.regular, em, x, a.cy(), p.text, &text);
                // A small arrow: it opens a menu.
                let (ax, ay, d) = (a.x + a.w - (BUTTON_PAD + 3.0) * scale, a.cy(), 3.0 * scale);
                let mut pb = PathBuilder::new();
                pb.move_to(ax - d, ay - d / 2.0);
                pb.line_to(ax, ay + d / 2.0);
                pb.line_to(ax + d, ay - d / 2.0);
                stroke(&mut pix, pb.finish(), p.muted, 1.4 * scale);
            }
            Item::Copy | Item::SaveAs | Item::Edit => {
                let w = text_width(&fonts.regular, em, &text);
                draw_text(&mut pix, &fonts.regular, em, a.x + (a.w - w) / 2.0, a.cy(), p.text, &text);
            }
        }
    }
    Some(pix)
}

/// Premultiplied RGBA (tiny-skia) to BGRA for Windows, in place. The bar is opaque.
pub fn to_bgra(pix: &mut Pixmap) {
    for px in pix.data_mut().chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> State {
        State { on_top: true, opacity: Some(100), zoom: 100, hover: None, pressed: None, copied: false }
    }

    #[test]
    fn half_size_averages() {
        // 3 x 2: two columns of 2 x 2, the last one of 1 x 2 averaged with itself.
        #[rustfmt::skip]
        let src: Vec<u8> = [
            [0u8, 0, 0, 255], [100, 100, 100, 255], [10, 20, 30, 255],
            [100, 100, 100, 255], [0, 0, 0, 255], [30, 40, 50, 255],
        ].concat();
        let (out, w, h) = half_bgra(&src, 3, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, [50, 50, 50, 255, 20, 30, 40, 255]);
        let (out, w, h) = half_bgra(&[9, 8, 7, 6], 1, 1);
        assert_eq!((w, h, out), (1, 1, vec![9, 8, 7, 6]));
    }

    #[test]
    fn pyramid_never_enlarges_a_smaller_level() {
        // 4000 x 2000, levels 2000 x 1000, 1000 x 500, 500 x 250.
        assert_eq!(pyramid_level(4000, 2000, 4000, 2000, 4), 0);
        assert_eq!(pyramid_level(4000, 2000, 2001, 1000, 4), 0);
        assert_eq!(pyramid_level(4000, 2000, 2000, 1000, 4), 1);
        assert_eq!(pyramid_level(4000, 2000, 1200, 600, 4), 1);
        assert_eq!(pyramid_level(4000, 2000, 700, 350, 4), 2);
        assert_eq!(pyramid_level(4000, 2000, 100, 50, 4), 3);
        assert_eq!(pyramid_level(4000, 2000, 100, 50, 2), 1);
        // Zoomed in past 100%: the screenshot itself.
        assert_eq!(pyramid_level(400, 200, 800, 400, 3), 0);
    }

    fn fonts() -> &'static Fonts {
        crate::capture_bar::fonts()
    }

    #[test]
    fn items_fit_and_never_overlap() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for width in [260.0, 340.0, 480.0, 900.0] {
                let w = width * scale;
                let l = layout(w, scale, &st(), fonts());
                for (i, (_, a)) in l.items.iter().enumerate() {
                    assert!(a.x >= 0.0 && a.x + a.w <= w, "{scale} {width}: inside");
                    for (_, b) in &l.items[i + 1..] {
                        let apart = a.x + a.w <= b.x || b.x + b.w <= a.x;
                        assert!(apart, "{scale} {width}: {a:?} overlaps {b:?}");
                    }
                }
                let has = |it: Item| l.items.iter().any(|(i, _)| *i == it);
                assert!(has(Item::OnTop) && has(Item::Copy) && has(Item::Close), "{scale} {width}");
            }
        }
    }

    #[test]
    fn wide_bars_show_everything_and_narrow_ones_drop_the_least_used() {
        let all = layout(900.0, 1.0, &st(), fonts());
        assert_eq!(all.items.len(), 7);
        let narrow = layout(min_width(1.0) as f32, 1.0, &st(), fonts());
        let items: Vec<Item> = narrow.items.iter().map(|(i, _)| *i).collect();
        assert!(!items.contains(&Item::Zoom));
        assert!(items.contains(&Item::Copy));
        // Without opacity (not on Windows) there is no opacity item.
        let none = layout(900.0, 1.0, &State { opacity: None, ..st() }, fonts());
        assert!(none.items.iter().all(|(i, _)| *i != Item::Opacity));
    }

    #[test]
    fn hits_the_item_under_the_mouse() {
        let l = layout(900.0, 1.0, &st(), fonts());
        for &(item, a) in &l.items {
            assert_eq!(l.hit(a.x + a.w / 2.0, a.cy()), Some(item));
        }
        // The empty middle of the bar is for dragging the window.
        assert_eq!(l.hit(600.0, 18.0), None);
    }

    #[test]
    fn the_gap_between_buttons_is_generous() {
        let l = layout(900.0, 2.0, &st(), fonts());
        let left: Vec<Area> = l.items.iter().take(4).map(|(_, a)| *a).collect();
        for pair in left.windows(2) {
            assert!(pair[1].x - (pair[0].x + pair[0].w) >= GAP * 2.0 - 1.0);
        }
    }

    #[test]
    fn fit_keeps_the_aspect_ratio() {
        assert_eq!(fit(800, 400, 800, 400), (0, 0, 800, 400));
        assert_eq!(fit(400, 400, 800, 400), (0, 100, 400, 200));
        // A small image in a window made wider for the bar: centred, not stretched.
        assert_eq!(fit(300, 40, 100, 40), (100, 0, 100, 40));
    }

    #[test]
    fn renders_at_every_scale() {
        for scale in [1.0, 1.5, 2.0] {
            let w = (600.0 * scale) as u32;
            let s = State { hover: Some(Item::Copy), ..st() };
            let l = layout(w as f32, scale, &s, fonts());
            let pix = render(&l, w, scale, &s, fonts()).unwrap();
            assert_eq!(pix.height() as i32, height(scale));
            // The checked box is the accent.
            let (_, a) = l.items[0];
            let s = (BOX * scale).round();
            let px = pix.pixel((a.x + 6.0 * scale + s * 0.5) as u32, (a.cy() - s * 0.35) as u32).unwrap();
            assert!(px.red() > 200 && px.green() < 120, "{scale}: accent box");
        }
    }

    /// `BLINK_PIN_PREVIEW=<dir> cargo test pin_bar -- --nocapture` writes PNGs of the bar.
    #[test]
    fn preview() {
        let Some(dir) = std::env::var_os("BLINK_PIN_PREVIEW") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, width, scale, s) in [
            ("wide", 640.0, 1.0, State { hover: Some(Item::SaveAs), ..st() }),
            ("wide_150", 640.0, 1.5, State { on_top: false, opacity: Some(60), zoom: 75, ..st() }),
            ("narrow", 300.0, 1.0, State { copied: true, ..st() }),
        ] {
            let w = (width * scale) as u32;
            let l = layout(w as f32, scale, &s, fonts());
            let pix = render(&l, w, scale, &s, fonts()).unwrap();
            let out = dir.join(format!("pin_bar_{name}.png"));
            pix.save_png(&out).unwrap();
            println!("{}", out.display());
        }
    }
}
