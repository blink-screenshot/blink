//! The mode bar of the capture overlay, and the small labels around the selection.
//!
//! The bar sits at the top of the screen while you pick what to capture. Its top row shows
//! the four capture modes as segments, the active one in the accent (cyan for scrolling
//! capture), the way the editor shows its active tool; a click on a segment switches to that
//! mode. The second row says what the mouse and the keys do in the current mode. A grip on
//! the left side of the bar, as high as the bar, lets you drag the whole bar out of the way.
//!
//! Everything is drawn here with anti-aliasing (tiny-skia for the shapes, ab_glyph for the
//! text) into a premultiplied RGBA pixmap, so the Windows overlay only has to blend it onto
//! the screen. Plain GDI has no anti-aliasing, which made the old hint bar look jagged. The
//! layout does not depend on Windows, so it is tested (and can be previewed) on any platform.

use crate::i18n::t;
use crate::theme::{self, DARK};
use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use eframe::egui::Color32;
use std::sync::OnceLock;
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform,
};

/// What a click (or Enter) captures. Space cycles through these in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Window under the cursor, or a dragged region.
    Region,
    /// The monitor under the cursor.
    Screen,
    /// The whole virtual desktop.
    AllScreens,
    /// Scrolling capture of the area you pick.
    Scroll,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Region, Mode::Screen, Mode::AllScreens, Mode::Scroll];

    pub fn next(self) -> Mode {
        match self {
            Mode::Region => Mode::Screen,
            Mode::Screen => Mode::AllScreens,
            Mode::AllScreens => Mode::Scroll,
            Mode::Scroll => Mode::Region,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Region => t("Region / window"),
            Mode::Screen => t("This screen"),
            Mode::AllScreens => t("All screens"),
            Mode::Scroll => t("Scrolling"),
        }
    }

    /// The key for this mode: F and A capture the screen(s) right away, S switches to
    /// scrolling capture. Region / window is where the overlay starts, so it has none.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Mode::Region => None,
            Mode::Screen => Some("F"),
            Mode::AllScreens => Some("A"),
            Mode::Scroll => Some("S"),
        }
    }

    /// The color of the selection and the active segment in this mode.
    pub fn color(self) -> Color32 {
        match self {
            Mode::Scroll => rgb(theme::hex::SIGNAL),
            _ => theme::ACCENT,
        }
    }

    /// What the mouse and the keys do in this mode: (keys, meaning) pairs.
    fn help(self) -> Vec<(Vec<&'static str>, &'static str)> {
        let (drag, click, space, esc) = (t("Drag"), t("Click"), t("Space"), "Esc");
        match self {
            Mode::Region => vec![
                (vec![drag], t("region")),
                (vec![click], t("window")),
                (vec!["Ctrl", click], t("inner pane")),
                (vec![space], t("next mode")),
                (vec![esc], t("cancel")),
            ],
            Mode::Screen | Mode::AllScreens => vec![
                (vec![click], t("capture")),
                (vec![drag], t("region")),
                (vec![space], t("next mode")),
                (vec![esc], t("cancel")),
            ],
            Mode::Scroll => vec![
                (vec![click], t("scrollable area")),
                (vec![drag], t("region")),
                (vec![space], t("next mode")),
                (vec![esc], t("cancel")),
            ],
        }
    }
}

/// What is under the mouse on the bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Mode(Mode),
    Close,
    /// The grip on the left side of the bar that moves it.
    Grip,
}

const fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

// ---- fonts -----------------------------------------------------------------------------

/// The interface font (Segoe UI on Windows) in the two weights the overlay uses.
pub struct Fonts {
    pub regular: FontArc,
    pub semibold: FontArc,
}

impl Fonts {
    #[cfg(test)]
    pub fn new(regular: FontArc, semibold: FontArc) -> Self {
        Self { regular, semibold }
    }

    fn load() -> Self {
        let dir = std::env::var_os("WINDIR")
            .map(|w| std::path::PathBuf::from(w).join("Fonts"))
            .unwrap_or_else(|| "C:\\Windows\\Fonts".into());
        let read = |names: &[&str]| names.iter().find_map(|n| std::fs::read(dir.join(n)).ok());
        let font = |b: Vec<u8>| FontArc::try_from_vec(b).ok();
        let fallback = || {
            let defs = eframe::egui::FontDefinitions::default();
            let fd = defs.font_data.values().next().expect("egui ships default fonts");
            FontArc::try_from_vec(fd.font.to_vec()).expect("egui's default font is valid")
        };
        let regular = read(&["segoeui.ttf"]).and_then(font).unwrap_or_else(fallback);
        let semibold = read(&["seguisb.ttf", "segoeuib.ttf"]).and_then(font).unwrap_or_else(|| regular.clone());
        Self { regular, semibold }
    }
}

/// The fonts, read from disk once per process.
pub fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(Fonts::load)
}

/// ab_glyph scales by line height; the interface sizes text by em, like egui and GDI.
fn px_scale(font: &FontArc, em: f32) -> PxScale {
    let upem = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(em * font.height_unscaled() / upem)
}

fn text_width(font: &FontArc, em: f32, s: &str) -> f32 {
    let sf = font.as_scaled(px_scale(font, em));
    let mut w = 0.0;
    let mut prev = None;
    for c in s.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Height of a capital letter, to center text on its caps (it then looks centered).
fn cap_height(font: &FontArc, em: f32) -> f32 {
    let g = font.glyph_id('H').with_scale_and_position(px_scale(font, em), point(0.0, 0.0));
    font.outline_glyph(g).map_or(em * 0.7, |o| -o.px_bounds().min.y)
}

/// Draws `s` with its left edge at `x`, vertically centered on `cy`.
fn draw_text(pix: &mut Pixmap, font: &FontArc, em: f32, x: f32, cy: f32, color: Color32, s: &str) {
    let scale = px_scale(font, em);
    let sf = font.as_scaled(scale);
    let baseline = (cy + cap_height(font, em) / 2.0).round();
    let (w, h) = (pix.width() as i32, pix.height() as i32);
    let pixels = pix.pixels_mut();
    let mut pen = x.round();
    let mut prev = None;
    for c in s.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = prev {
            pen += sf.kern(p, id);
        }
        let g = id.with_scale_and_position(scale, point(pen, baseline));
        pen += sf.h_advance(id);
        prev = Some(id);
        let Some(o) = font.outline_glyph(g) else { continue };
        let b = o.px_bounds();
        o.draw(|gx, gy, cov| {
            let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
            if px < 0 || py < 0 || px >= w || py >= h {
                return;
            }
            let cov = cov.clamp(0.0, 1.0);
            let d = &mut pixels[(py * w + px) as usize];
            let blend = |src: u8, dst: u8| (src as f32 * cov + dst as f32 * (1.0 - cov)).round() as u8;
            let a = blend(255, d.alpha());
            let (r, g, bl) = (blend(color.r(), d.red()), blend(color.g(), d.green()), blend(color.b(), d.blue()));
            if let Some(p) = PremultipliedColorU8::from_rgba(r.min(a), g.min(a), bl.min(a), a) {
                *d = p;
            }
        });
    }
}

// ---- shapes ----------------------------------------------------------------------------

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Area {
    fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x: x.round(), y: y.round(), w: w.round(), h: h.round() }
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    fn inset(&self, l: f32, t: f32, r: f32, b: f32) -> Self {
        Self { x: self.x + l, y: self.y + t, w: self.w - l - r, h: self.h - t - b }
    }
}

/// Rounded rectangle with its own radius per corner (top-left, top-right, bottom-right,
/// bottom-left).
fn rrect(a: Area, r: [f32; 4]) -> Option<Path> {
    let max = a.w.min(a.h) / 2.0;
    let [tl, tr, br, bl] = r.map(|r| r.clamp(0.0, max));
    let k = 1.0 - 0.552_284_8;
    let (x0, y0, x1, y1) = (a.x, a.y, a.x + a.w, a.y + a.h);
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + tl, y0);
    pb.line_to(x1 - tr, y0);
    pb.cubic_to(x1 - tr * k, y0, x1, y0 + tr * k, x1, y0 + tr);
    pb.line_to(x1, y1 - br);
    pb.cubic_to(x1, y1 - br * k, x1 - br * k, y1, x1 - br, y1);
    pb.line_to(x0 + bl, y1);
    pb.cubic_to(x0 + bl * k, y1, x0, y1 - bl * k, x0, y1 - bl);
    pb.line_to(x0, y0 + tl);
    pb.cubic_to(x0, y0 + tl * k, x0 + tl * k, y0, x0 + tl, y0);
    pb.close();
    pb.finish()
}

fn paint(c: Color32, alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.r(), c.g(), c.b(), (alpha * 255.0).round() as u8);
    p.anti_alias = true;
    p
}

fn fill(pix: &mut Pixmap, path: Option<Path>, c: Color32, alpha: f32) {
    if let Some(path) = path {
        pix.fill_path(&path, &paint(c, alpha), FillRule::Winding, Transform::identity(), None);
    }
}

fn stroke(pix: &mut Pixmap, path: Option<Path>, c: Color32, width: f32) {
    if let Some(path) = path {
        let s = Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        pix.stroke_path(&path, &paint(c, 1.0), &s, Transform::identity(), None);
    }
}

/// A soft shadow under a rounded shape, from a few widening, faint copies of it.
fn shadow(pix: &mut Pixmap, a: Area, radius: f32, scale: f32) {
    for i in 1..=6 {
        let g = i as f32 * 1.7 * scale;
        let s = Area { x: a.x - g, y: a.y - g + 4.0 * scale, w: a.w + 2.0 * g, h: a.h + 2.0 * g };
        fill(pix, rrect(s, [radius + g; 4]), rgb(0x000000), 0.055);
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Region,
    Screen,
    AllScreens,
    Scroll,
    Close,
}

/// Line icon in an 18 x 18 grid, drawn at `a` (square). `bg` fills the front monitor of
/// the "all screens" icon so the back one passes behind it.
fn icon(pix: &mut Pixmap, which: Icon, a: Area, c: Color32, bg: Color32) {
    icon_weighted(pix, which, a, c, Some(bg), 1.7);
}

/// The icon of a capture mode, as on the mode bar, at `(x, y)` and `size` pixels square on a
/// transparent pixmap, with lines `weight` grid units (of 18) wide. The tray menu uses these,
/// so a mode looks the same everywhere.
pub fn mode_icon(pix: &mut Pixmap, mode: Mode, x: f32, y: f32, size: f32, c: Color32, weight: f32) {
    let which = match mode {
        Mode::Region => Icon::Region,
        Mode::Screen => Icon::Screen,
        Mode::AllScreens => Icon::AllScreens,
        Mode::Scroll => Icon::Scroll,
    };
    icon_weighted(pix, which, Area { x, y, w: size, h: size }, c, None, weight);
}

/// `icon` with its line width in grid units. Without `bg` the back monitor of "all screens"
/// is erased behind the front one, for a transparent background.
fn icon_weighted(pix: &mut Pixmap, which: Icon, a: Area, c: Color32, bg: Option<Color32>, weight: f32) {
    let u = a.w / 18.0;
    let p = |x: f32, y: f32| (a.x + x * u, a.y + y * u);
    let area = |x: f32, y: f32, w: f32, h: f32| Area { x: a.x + x * u, y: a.y + y * u, w: w * u, h: h * u };
    let width = weight * u;
    let lines = |segs: &[&[(f32, f32)]]| {
        let mut pb = PathBuilder::new();
        for s in segs {
            let (x, y) = p(s[0].0, s[0].1);
            pb.move_to(x, y);
            for &(px, py) in &s[1..] {
                let (x, y) = p(px, py);
                pb.line_to(x, y);
            }
        }
        pb.finish()
    };
    match which {
        Icon::Region => {
            // The logo's four rounded corners.
            let mut pb = PathBuilder::new();
            for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
                let at = |dx: f32, dy: f32| p(9.0 + sx * dx, 9.0 + sy * dy);
                let (x, y) = at(7.0, 3.0);
                pb.move_to(x, y);
                let (x, y) = at(7.0, 4.5);
                pb.line_to(x, y);
                let (cx, cy) = at(7.0, 7.0);
                let (x, y) = at(4.5, 7.0);
                pb.quad_to(cx, cy, x, y);
                let (x, y) = at(3.0, 7.0);
                pb.line_to(x, y);
            }
            stroke(pix, pb.finish(), c, width);
        }
        Icon::Screen => {
            stroke(pix, rrect(area(2.0, 3.0, 14.0, 9.5), [2.0 * u; 4]), c, width);
            stroke(pix, lines(&[&[(6.5, 15.5), (11.5, 15.5)], &[(9.0, 12.5), (9.0, 15.5)]]), c, width);
        }
        Icon::AllScreens => {
            stroke(pix, rrect(area(1.5, 4.0, 9.0, 6.5), [1.6 * u; 4]), c, width);
            let front = rrect(area(7.5, 7.0, 9.0, 6.5), [1.6 * u; 4]);
            match bg {
                Some(bg) => fill(pix, front, bg, 1.0),
                None => {
                    if let Some(path) = front {
                        let mut clear = paint(c, 1.0);
                        clear.blend_mode = tiny_skia::BlendMode::Clear;
                        pix.fill_path(&path, &clear, FillRule::Winding, Transform::identity(), None);
                    }
                }
            }
            stroke(pix, rrect(area(7.5, 7.0, 9.0, 6.5), [1.6 * u; 4]), c, width);
            stroke(pix, lines(&[&[(10.0, 16.0), (14.0, 16.0)]]), c, width);
        }
        Icon::Scroll => {
            stroke(pix, rrect(area(3.0, 2.0, 12.0, 14.0), [2.4 * u; 4]), c, width);
            stroke(pix, lines(&[&[(9.0, 6.0), (9.0, 12.0)], &[(6.8, 9.8), (9.0, 12.0), (11.2, 9.8)]]), c, width);
        }
        Icon::Close => {
            stroke(pix, lines(&[&[(5.0, 5.0), (13.0, 13.0)], &[(13.0, 5.0), (5.0, 13.0)]]), c, width * 1.06);
        }
    }
}

// ---- the bar ---------------------------------------------------------------------------

struct Segment {
    mode: Mode,
    area: Area,
    icon: Area,
    label_x: f32,
    key: Option<Area>,
}

struct HelpItem {
    keys: Vec<(Area, &'static str)>,
    /// Where the "+" between two keys goes.
    plus: Vec<f32>,
    label_x: f32,
    label: &'static str,
}

/// Where everything on the bar goes, in pixels, relative to the top-left of the panel.
pub struct Layout {
    pub mode: Mode,
    pub scale: f32,
    /// Size of the panel.
    pub w: i32,
    pub h: i32,
    /// Room around the panel for its shadow; the pixmap is `w + 2 * margin` wide.
    pub margin: i32,
    radius: f32,
    row_h: f32,
    segments: Vec<Segment>,
    separator: Area,
    close: Area,
    /// The grip on the left side of the panel, as high as the panel.
    grip: Area,
    help: Vec<HelpItem>,
    help_cy: f32,
}

// Sizes in points (multiplied by the display scale).
const PAD: f32 = 5.0;
const SEG_H: f32 = 36.0;
const SEG_GAP: f32 = 4.0;
const SEG_PAD: f32 = 10.0;
const ICON: f32 = 18.0;
const LABEL_EM: f32 = 13.0;
const KEY_EM: f32 = 11.5;
const KEY_H: f32 = 19.0;
const HELP_EM: f32 = 12.5;
const HELP_KEY_EM: f32 = 11.0;
const HELP_KEY_H: f32 = 18.0;
const HELP_H: f32 = 34.0;
const HELP_PAD: f32 = 16.0;
const HELP_GAP: f32 = 18.0;
const GRIP_W: f32 = 22.0;

impl Layout {
    pub fn new(mode: Mode, scale: f32, fonts: &Fonts) -> Self {
        let sc = scale.max(0.5);
        let semi = &fonts.semibold;
        let key_w = |s: &str, em: f32, pad: f32| (text_width(semi, em * sc, s) + 2.0 * pad * sc).max(20.0 * sc).round();

        // Top row, right of the grip: the four modes, a separator and the close button.
        let grip_w = (GRIP_W * sc).round();
        let mut x = grip_w + PAD * sc;
        let top = PAD * sc;
        let mut segments = Vec::new();
        for m in Mode::ALL {
            let label_w = text_width(semi, LABEL_EM * sc, m.label());
            let kw = m.key().map(|k| key_w(k, KEY_EM, 6.0));
            let w = SEG_PAD * sc + ICON * sc + 8.0 * sc + label_w + kw.map_or(0.0, |k| 9.0 * sc + k) + SEG_PAD * sc;
            let area = Area::new(x, top, w, SEG_H * sc);
            let icon = Area::new(x + SEG_PAD * sc, area.cy() - ICON * sc / 2.0, ICON * sc, ICON * sc);
            let label_x = icon.x + icon.w + 8.0 * sc;
            let key =
                kw.map(|k| Area::new(area.x + area.w - SEG_PAD * sc - k, area.cy() - KEY_H * sc / 2.0, k, KEY_H * sc));
            segments.push(Segment { mode: m, area, icon, label_x, key });
            x = area.x + area.w + SEG_GAP * sc;
        }
        x += 4.0 * sc;
        let separator = Area::new(x, top + 7.0 * sc, sc.round().max(1.0), SEG_H * sc - 14.0 * sc);
        x = separator.x + separator.w + 8.0 * sc;
        let close = Area::new(x, top, SEG_H * sc, SEG_H * sc);
        let row_w = close.x + close.w + PAD * sc;
        let row_h = (SEG_H + 2.0 * PAD) * sc;

        // Second row: what the mouse and the keys do.
        let mut help = Vec::new();
        let mut hx = 0.0;
        let help_cy = row_h + HELP_H * sc / 2.0;
        for (keys, label) in mode.help() {
            let mut item = HelpItem { keys: Vec::new(), plus: Vec::new(), label_x: 0.0, label };
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    item.plus.push(hx + 4.0 * sc);
                    hx += 4.0 * sc + text_width(&fonts.regular, HELP_EM * sc, "+") + 4.0 * sc;
                }
                let w = key_w(k, HELP_KEY_EM, 5.0);
                item.keys.push((Area::new(hx, help_cy - HELP_KEY_H * sc / 2.0, w, HELP_KEY_H * sc), *k));
                hx += w;
            }
            item.label_x = hx + 7.0 * sc;
            hx = item.label_x + text_width(&fonts.regular, HELP_EM * sc, label) + HELP_GAP * sc;
            help.push(item);
        }
        let help_w = grip_w + hx - HELP_GAP * sc + 2.0 * HELP_PAD * sc;

        // Both rows centered, right of the grip, in the wider of the two.
        let w = row_w.max(help_w).round();
        let dx = ((w - row_w) / 2.0).round();
        for s in &mut segments {
            s.area.x += dx;
            s.icon.x += dx;
            s.label_x += dx;
            if let Some(k) = &mut s.key {
                k.x += dx;
            }
        }
        let (mut separator, mut close) = (separator, close);
        separator.x += dx;
        close.x += dx;
        let hdx = ((w - help_w) / 2.0 + grip_w + HELP_PAD * sc).round();
        for item in &mut help {
            for (a, _) in &mut item.keys {
                a.x += hdx;
            }
            for p in &mut item.plus {
                *p += hdx;
            }
            item.label_x += hdx;
        }

        let h = (row_h + HELP_H * sc).round();
        let grip = Area::new(0.0, 0.0, grip_w, h);

        Layout {
            mode,
            scale: sc,
            w: w as i32,
            h: h as i32,
            // Room for the shadow.
            margin: (14.0 * sc).ceil() as i32,
            // Concentric with the segments inside: their radius plus the padding.
            radius: theme::radius(SEG_H) as f32 * sc + PAD * sc,
            row_h: row_h.round(),
            segments,
            separator,
            close,
            grip,
            help,
            help_cy,
        }
    }

    /// What is at `(x, y)`, relative to the top-left of the panel.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if self.grip.contains(x, y) {
            return Some(Hit::Grip);
        }
        if let Some(s) = self.segments.iter().find(|s| s.area.contains(x, y)) {
            return Some(Hit::Mode(s.mode));
        }
        self.close.contains(x, y).then_some(Hit::Close)
    }

    /// Whether `(x, y)` (relative to the panel) is on the bar at all.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= 0.0 && y >= 0.0 && x < self.w as f32 && y < self.h as f32
    }

    /// Draws the bar, with `hover` under the mouse. The panel's top-left is at
    /// `(margin, margin)` in the returned pixmap (premultiplied RGBA).
    pub fn render(&self, hover: Option<Hit>, fonts: &Fonts) -> Option<Pixmap> {
        let m = self.margin as f32;
        let sc = self.scale;
        let mut pix = Pixmap::new((self.w + 2 * self.margin) as u32, (self.h + 2 * self.margin) as u32)?;
        let panel = Area { x: m, y: m, w: self.w as f32, h: self.h as f32 };
        let at = |a: Area| Area { x: a.x + m, y: a.y + m, ..a };
        let r = self.radius;
        let line = sc.round().max(1.0);

        shadow(&mut pix, panel, r, sc);
        fill(&mut pix, rrect(panel, [r; 4]), DARK.panel, 1.0);
        let gw = self.grip.w;
        let help = Area { x: panel.x + gw, y: panel.y + self.row_h, w: panel.w - gw, h: panel.h - self.row_h };
        fill(&mut pix, rrect(help, [0.0, 0.0, r, r]), DARK.panel_raised, 1.0);
        fill(&mut pix, rrect(Area { h: line, ..help }, [0.0; 4]), DARK.border, 1.0);
        stroke(
            &mut pix,
            rrect(panel.inset(line / 2.0, line / 2.0, line / 2.0, line / 2.0), [r - line / 2.0; 4]),
            DARK.border,
            line,
        );

        // The grip: a strip on the left, as high as the bar, behind a thin line, with two
        // columns of dots that light up under the mouse.
        let grip = at(self.grip);
        let hot = hover == Some(Hit::Grip);
        if hot {
            fill(
                &mut pix,
                rrect(grip.inset(line, line, 0.0, line), [r - line, 0.0, 0.0, r - line]),
                DARK.control_hover,
                1.0,
            );
        }
        fill(&mut pix, rrect(Area { x: grip.x + grip.w - line, w: line, ..grip }, [0.0; 4]), DARK.border, 1.0);
        let dots = if hot { DARK.text } else { DARK.muted };
        let (gcx, gcy) = (grip.x + (grip.w - line) / 2.0, grip.y + grip.h / 2.0);
        for col in [-0.5f32, 0.5] {
            for row in -1..=1 {
                let mut pb = PathBuilder::new();
                pb.push_circle(gcx + col * 6.0 * sc, gcy + row as f32 * 6.0 * sc, 1.4 * sc);
                fill(&mut pix, pb.finish(), dots, 1.0);
            }
        }

        let semi = &fonts.semibold;
        let seg_r = theme::radius(SEG_H) as f32 * sc;
        for s in &self.segments {
            let active = s.mode == self.mode;
            let a = at(s.area);
            let bg = if active {
                s.mode.color()
            } else if hover == Some(Hit::Mode(s.mode)) {
                DARK.control_hover
            } else {
                DARK.panel
            };
            if bg != DARK.panel {
                fill(&mut pix, rrect(a, [seg_r; 4]), bg, 1.0);
            }
            let fg = if active { theme::INK } else { DARK.text };
            let which = match s.mode {
                Mode::Region => Icon::Region,
                Mode::Screen => Icon::Screen,
                Mode::AllScreens => Icon::AllScreens,
                Mode::Scroll => Icon::Scroll,
            };
            icon(&mut pix, which, at(s.icon), fg, bg);
            draw_text(&mut pix, semi, LABEL_EM * sc, s.label_x + m, a.cy(), fg, s.mode.label());
            if let (Some(k), Some(text)) = (s.key, s.mode.key()) {
                let (face, rim) = if active {
                    (mix(bg, theme::INK, 0.16), mix(bg, theme::INK, 0.3))
                } else {
                    (DARK.control, DARK.control_strong)
                };
                key_cap(&mut pix, semi, KEY_EM * sc, at(k), text, face, rim, fg, sc);
            }
        }
        fill(&mut pix, rrect(at(self.separator), [0.0; 4]), DARK.border, 1.0);
        let close = at(self.close);
        let close_c = if hover == Some(Hit::Close) {
            fill(&mut pix, rrect(close, [seg_r; 4]), DARK.control_hover, 1.0);
            DARK.text
        } else {
            DARK.muted
        };
        let s = 16.0 * sc;
        let ia = Area { x: close.x + (close.w - s) / 2.0, y: close.y + (close.h - s) / 2.0, w: s, h: s };
        icon(&mut pix, Icon::Close, ia, close_c, DARK.panel);

        for item in &self.help {
            for (k, text) in &item.keys {
                key_cap(
                    &mut pix,
                    semi,
                    HELP_KEY_EM * sc,
                    at(*k),
                    text,
                    DARK.control,
                    DARK.control_strong,
                    DARK.text,
                    sc,
                );
            }
            for p in &item.plus {
                draw_text(&mut pix, &fonts.regular, HELP_EM * sc, p + m, self.help_cy + m, DARK.muted, "+");
            }
            draw_text(
                &mut pix,
                &fonts.regular,
                HELP_EM * sc,
                item.label_x + m,
                self.help_cy + m,
                DARK.muted,
                item.label,
            );
        }
        Some(pix)
    }
}

/// A key on the keyboard: a rounded face with a slightly thicker edge at the bottom.
#[allow(clippy::too_many_arguments)]
fn key_cap(
    pix: &mut Pixmap,
    font: &FontArc,
    em: f32,
    a: Area,
    text: &str,
    face: Color32,
    rim: Color32,
    fg: Color32,
    sc: f32,
) {
    let r = theme::radius(a.h / sc) as f32 * sc + sc;
    let line = sc.round().max(1.0);
    fill(pix, rrect(a, [r; 4]), rim, 1.0);
    fill(pix, rrect(a.inset(line, line, line, 2.0 * line), [r - line; 4]), face, 1.0);
    let tw = text_width(font, em, text);
    draw_text(pix, font, em, a.x + (a.w - tw) / 2.0, a.cy() - line / 2.0, fg, text);
}

/// A rounded label such as the size of the selection: `text` on `fill`, with an optional
/// one-point `border`. Premultiplied RGBA, exactly the size of the label.
pub fn label(
    text: &str,
    fill_c: Color32,
    border: Option<Color32>,
    fg: Color32,
    scale: f32,
    fonts: &Fonts,
) -> Option<Pixmap> {
    let sc = scale.max(0.5);
    let em = 12.0 * sc;
    let font = &fonts.semibold;
    let tw = text_width(font, em, text);
    let a = Area::new(0.0, 0.0, tw + 20.0 * sc, 23.0 * sc);
    let mut pix = Pixmap::new(a.w as u32, a.h as u32)?;
    fill(&mut pix, rrect(a, [a.h / 2.0; 4]), fill_c, 1.0);
    if let Some(b) = border {
        let line = sc.round().max(1.0);
        stroke(&mut pix, rrect(a.inset(line / 2.0, line / 2.0, line / 2.0, line / 2.0), [a.h / 2.0; 4]), b, line);
    }
    draw_text(&mut pix, font, em, (a.w - tw) / 2.0, a.cy(), fg, text);
    Some(pix)
}

/// A rounded rim of `width` pixels on the inside edge of a `w` x `h` box (premultiplied
/// RGBA, transparent inside). Covers the jagged edge of a picture clipped by GDI.
pub fn rim(w: i32, h: i32, radius: f32, color: Color32, width: f32) -> Option<Pixmap> {
    let mut pix = Pixmap::new(w as u32, h as u32)?;
    let a = Area { x: 0.0, y: 0.0, w: w as f32, h: h as f32 }.inset(width / 2.0, width / 2.0, width / 2.0, width / 2.0);
    stroke(&mut pix, rrect(a, [radius; 4]), color, width);
    Some(pix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_fonts() -> Fonts {
        // Inter where it is installed (close to Segoe UI), otherwise egui's own font.
        let read = |p: &str| std::fs::read(p).ok().and_then(|b| FontArc::try_from_vec(b).ok());
        match (
            read("/usr/share/fonts/opentype/inter/Inter-Regular.otf"),
            read("/usr/share/fonts/opentype/inter/Inter-SemiBold.otf"),
        ) {
            (Some(r), Some(s)) => Fonts::new(r, s),
            _ => Fonts::load(),
        }
    }

    #[test]
    fn segments_follow_each_other_and_are_hit() {
        let fonts = test_fonts();
        for scale in [1.0, 1.5, 2.0] {
            for mode in Mode::ALL {
                let l = Layout::new(mode, scale, &fonts);
                assert!(l.w > 0 && l.h > 0);
                for pair in l.segments.windows(2) {
                    assert!(pair[0].area.x + pair[0].area.w <= pair[1].area.x, "segments overlap");
                }
                for s in &l.segments {
                    assert!(s.area.x >= 0.0 && s.area.x + s.area.w <= l.w as f32);
                    let (cx, cy) = (s.area.x + s.area.w / 2.0, s.area.cy());
                    assert_eq!(l.hit(cx, cy), Some(Hit::Mode(s.mode)));
                }
                assert_eq!(l.hit(l.close.x + 2.0, l.close.cy()), Some(Hit::Close));
                // The help row and the gaps between segments are not buttons.
                assert_eq!(l.hit(l.w as f32 / 2.0, l.help_cy), None);
                assert!(l.contains(l.w as f32 / 2.0, l.help_cy));
                assert!(!l.contains(-1.0, 0.0));
                // The grip is the left side of the panel, top to bottom, left of everything.
                assert_eq!((l.grip.x, l.grip.y, l.grip.h), (0.0, 0.0, l.h as f32));
                for y in [1.0, l.help_cy, l.h as f32 - 1.0] {
                    assert_eq!(l.hit(l.grip.w / 2.0, y), Some(Hit::Grip));
                }
                assert!(l.segments.iter().all(|s| s.area.x >= l.grip.w));
                assert!(l.help.iter().all(|i| i.keys[0].0.x >= l.grip.w));
            }
        }
    }

    #[test]
    fn help_row_fits_inside_the_panel() {
        let fonts = test_fonts();
        for mode in Mode::ALL {
            let l = Layout::new(mode, 1.25, &fonts);
            let first = l.help.first().unwrap().keys[0].0.x;
            assert!(first >= (GRIP_W + HELP_PAD) * 1.25 - 1.0, "{mode:?}: {first}");
            assert!(l.help.iter().all(|i| i.label_x < l.w as f32));
        }
    }

    #[test]
    fn renders_the_active_mode_in_its_color() {
        let fonts = test_fonts();
        for mode in [Mode::Region, Mode::Scroll] {
            let l = Layout::new(mode, 1.0, &fonts);
            let pix = l.render(None, &fonts).expect("pixmap");
            let seg = l.segments.iter().find(|s| s.mode == mode).unwrap();
            // A pixel just inside the active segment's left edge, halfway down.
            let (x, y) = ((seg.area.x + 3.0) as u32 + l.margin as u32, seg.area.cy() as u32 + l.margin as u32);
            let p = pix.pixel(x, y).unwrap();
            let c = mode.color();
            assert_eq!((p.red(), p.green(), p.blue(), p.alpha()), (c.r(), c.g(), c.b(), 255));
        }
    }

    /// `BLINK_BAR_PREVIEW=<dir> cargo test capture_bar -- --nocapture` writes a PNG of the bar in
    /// every mode, to look at the design without Windows.
    #[test]
    fn preview() {
        let Some(dir) = std::env::var_os("BLINK_BAR_PREVIEW") else { return };
        let fonts = test_fonts();
        for (lang, name) in [(crate::i18n::Language::English, "en"), (crate::i18n::Language::Dutch, "nl")] {
            crate::i18n::set_language(lang);
            for (i, mode) in Mode::ALL.into_iter().enumerate() {
                let hover = (mode == Mode::Region).then_some(Hit::Mode(Mode::Screen));
                for scale in [1.0, 2.0] {
                    let l = Layout::new(mode, scale, &fonts);
                    let mut pix =
                        Pixmap::new(l.w as u32 + 2 * l.margin as u32, l.h as u32 + 2 * l.margin as u32).unwrap();
                    pix.fill(tiny_skia::Color::from_rgba8(24, 28, 40, 255));
                    let bar = l.render(hover, &fonts).unwrap();
                    pix.draw_pixmap(
                        0,
                        0,
                        bar.as_ref(),
                        &tiny_skia::PixmapPaint::default(),
                        Transform::identity(),
                        None,
                    );
                    let path = std::path::Path::new(&dir).join(format!("bar-{name}-{i}-{scale}x.png"));
                    pix.save_png(path).unwrap();
                }
            }
        }
        crate::i18n::set_language(crate::i18n::Language::English);
        let size = label("1000 × 620", theme::ACCENT, None, theme::INK, 2.0, &fonts).unwrap();
        size.save_png(std::path::Path::new(&dir).join("size-label-2x.png")).unwrap();
    }
}
