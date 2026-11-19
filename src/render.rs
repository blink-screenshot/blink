//! Software renderer: draws annotations onto the screenshot (used for both the editor
//! preview texture and the exported image, so what you see is exactly what you get).

use crate::annot::{arrow_geometry, callout_outline, shape_outline, Annotation, HeadShape, Kind, RedactMode, Style};
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use eframe::egui::{self, pos2, Color32, Pos2, Rect, Vec2};
use image::RgbaImage;
use std::sync::Arc;
use tiny_skia::{BlendMode, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

pub struct Fonts {
    pub regular: FontArc,
    pub bold: FontArc,
    pub italic: FontArc,
    pub bold_italic: FontArc,
    pub regular_bytes: Arc<Vec<u8>>,
    pub bold_bytes: Arc<Vec<u8>>,
    pub italic_bytes: Arc<Vec<u8>>,
    pub bold_italic_bytes: Arc<Vec<u8>>,
    /// Semibold UI font (labels); same as bold when the system has none.
    pub semibold_bytes: Arc<Vec<u8>>,
}

impl Fonts {
    pub fn load() -> Arc<Self> {
        let from_disk = |names: &[&str]| -> Option<Vec<u8>> {
            let dir = std::env::var_os("WINDIR")
                .map(|w| std::path::PathBuf::from(w).join("Fonts"))
                .unwrap_or_else(|| "C:\\Windows\\Fonts".into());
            names.iter().find_map(|n| std::fs::read(dir.join(n)).ok())
        };
        let fallback = || -> Vec<u8> {
            let defs = egui::FontDefinitions::default();
            let fd = defs
                .font_data
                .get("Ubuntu-Light")
                .or_else(|| defs.font_data.values().next())
                .expect("egui ships default fonts");
            fd.font.to_vec()
        };
        let regular = from_disk(&["segoeui.ttf"]).unwrap_or_else(fallback);
        let bold = from_disk(&["segoeuib.ttf", "seguisb.ttf"]).unwrap_or_else(|| regular.clone());
        let italic = from_disk(&["segoeuii.ttf"]).unwrap_or_else(|| regular.clone());
        let bold_italic = from_disk(&["segoeuiz.ttf"]).unwrap_or_else(|| bold.clone());
        let semibold = from_disk(&["seguisb.ttf"]).unwrap_or_else(|| bold.clone());
        let arc = |b: &Vec<u8>| FontArc::try_from_vec(b.clone()).expect("valid font");
        Arc::new(Self {
            regular: arc(&regular),
            bold: arc(&bold),
            italic: arc(&italic),
            bold_italic: arc(&bold_italic),
            regular_bytes: Arc::new(regular),
            bold_bytes: Arc::new(bold),
            italic_bytes: Arc::new(italic),
            bold_italic_bytes: Arc::new(bold_italic),
            semibold_bytes: Arc::new(semibold),
        })
    }

    /// Annotations are sized by em; egui (since 0.20) sizes fonts by em as well, so the
    /// in-place editor and live previews use the same number.
    pub fn egui_size(&self, em: f32) -> f32 {
        em
    }

    pub fn font(&self, bold: bool, italic: bool) -> &FontArc {
        match (bold, italic) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        }
    }

    /// Ascent (px) and the height above the baseline of the middle of the digits, for an em size.
    pub fn digit_metrics(&self, em: f32) -> (f32, f32) {
        let font = &self.bold;
        let scaled = font.as_scaled(px_scale(font, em));
        let id = font.glyph_id('8');
        let g = id.with_scale_and_position(scaled.scale(), ab_glyph::point(0.0, 0.0));
        let mid = font.outline_glyph(g).map(|o| -(o.px_bounds().min.y + o.px_bounds().max.y) / 2.0);
        (scaled.ascent(), mid.unwrap_or(scaled.ascent() * 0.35))
    }
}

/// `size` = em size in pixels.
fn px_scale(font: &FontArc, size: f32) -> PxScale {
    let upem = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(size * font.height_unscaled() / upem)
}

pub fn text_padding(size: f32) -> f32 {
    (size * 0.22).round()
}

struct LaidGlyph {
    glyph: ab_glyph::Glyph,
}

/// One laid-out line: baseline y and horizontal extent.
struct Line {
    baseline: f32,
    x0: f32,
    x1: f32,
}

/// Splits one paragraph into lines of at most `max_w` (`None` = no wrapping). `adv` holds each
/// char's advance. Breaks after spaces; a word wider than a line is broken between chars.
/// Returns char ranges; trailing spaces hang past the edge instead of forcing a break.
fn break_lines(chars: &[char], adv: &[f32], max_w: Option<f32>) -> Vec<(usize, usize)> {
    let n = chars.len();
    let mut lines = Vec::new();
    let (mut start, mut i, mut w) = (0, 0, 0.0f32);
    let mut brk: Option<usize> = None;
    while i < n {
        if let Some(m) = max_w {
            if chars[i] != ' ' && w + adv[i] > m && i > start {
                let cut = brk.filter(|b| *b > start).unwrap_or(i);
                lines.push((start, cut));
                (start, i, w, brk) = (cut, cut, 0.0, None);
                continue;
            }
        }
        w += adv[i];
        if chars[i] == ' ' {
            brk = Some(i + 1);
        }
        i += 1;
    }
    lines.push((start, n));
    lines
}

/// Lays out multi-line text with its top-left at `origin`, wrapping lines at `wrap` pixels
/// when given. Returns glyphs, lines and the text size.
fn layout(font: &FontArc, text: &str, size: f32, origin: Pos2, wrap: Option<f32>) -> (Vec<LaidGlyph>, Vec<Line>, Vec2) {
    let scaled = font.as_scaled(px_scale(font, size));
    let line_h = scaled.height() + scaled.line_gap();
    let mut out = Vec::new();
    let mut lines = Vec::new();
    let mut max_w: f32 = 0.0;
    for para in text.split('\n') {
        let chars: Vec<char> = para.chars().collect();
        let ids: Vec<_> = chars.iter().map(|c| scaled.glyph_id(*c)).collect();
        let adv: Vec<f32> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| scaled.h_advance(*id) + if i > 0 { scaled.kern(ids[i - 1], *id) } else { 0.0 })
            .collect();
        let ranges = break_lines(&chars, &adv, wrap);
        let last = ranges.len() - 1;
        for (ri, (s, e)) in ranges.into_iter().enumerate() {
            let baseline = origin.y + scaled.ascent() + lines.len() as f32 * line_h;
            let mut x = origin.x;
            for i in s..e {
                if i > s {
                    x += scaled.kern(ids[i - 1], ids[i]);
                }
                out.push(LaidGlyph {
                    glyph: ids[i].with_scale_and_position(scaled.scale(), ab_glyph::point(x, baseline)),
                });
                x += scaled.h_advance(ids[i]);
            }
            // A wrapped line does not count the spaces it ends with.
            let mut x1 = x;
            if ri < last {
                for i in (s..e).rev().take_while(|i| chars[*i] == ' ') {
                    x1 -= scaled.h_advance(ids[i]);
                }
            }
            max_w = max_w.max(x1 - origin.x);
            lines.push(Line { baseline, x0: origin.x, x1 });
        }
    }
    let h = scaled.height() + (lines.len().max(1) - 1) as f32 * line_h;
    (out, lines, Vec2::new(max_w, h))
}

/// Width of a text box the user dragged to size (`b.x` right of `a.x`); `None` = sized by its
/// text. Includes the padding.
pub fn text_fixed_width(ann: &Annotation) -> Option<f32> {
    let w = ann.b.x - ann.a.x;
    (w > 1.0).then(|| w.max(text_min_width(ann.style.font_size)))
}

/// Narrowest a text box can be made.
pub fn text_min_width(size: f32) -> f32 {
    size * 1.5 + text_padding(size) * 2.0
}

/// Width available to the text itself (without padding): the box width if it was sized by
/// hand, else the room left up to the right edge of the image (`bound_w`).
pub fn text_wrap_width(ann: &Annotation, bound_w: f32) -> f32 {
    let size = ann.style.font_size;
    if let Kind::Callout { .. } = ann.kind {
        return (ann.rect().width() - callout_padding(size) * 2.0).max(size);
    }
    let box_w = text_fixed_width(ann).unwrap_or(bound_w - ann.a.x);
    (box_w - text_padding(size) * 2.0).max(size * 2.0)
}

/// Size of a text annotation's box (text plus padding), in image pixels. `bound_w` is the
/// image width.
pub fn text_box_size(fonts: &Fonts, ann: &Annotation, bound_w: f32) -> Vec2 {
    match &ann.kind {
        Kind::Text(t) => {
            let font = fonts.font(ann.style.bold, ann.style.italic);
            let wrap = Some(text_wrap_width(ann, bound_w));
            let (_, _, sz) = layout(font, if t.is_empty() { " " } else { t }, ann.style.font_size, Pos2::ZERO, wrap);
            let pad = text_padding(ann.style.font_size);
            let w = match text_fixed_width(ann) {
                Some(w) => w,
                None => sz.x.max(ann.style.font_size * 0.3) + pad * 2.0,
            };
            Vec2::new(w, sz.y + pad * 2.0)
        }
        Kind::Callout { text, .. } => {
            let font = fonts.font(ann.style.bold, ann.style.italic);
            let wrap = Some(text_wrap_width(ann, bound_w));
            let size = ann.style.font_size;
            let (_, _, sz) = layout(font, if text.is_empty() { " " } else { text }, size, Pos2::ZERO, wrap);
            let r = ann.rect();
            Vec2::new(r.width(), r.height().max(sz.y + callout_padding(size) * 2.0))
        }
        _ => Vec2::ZERO,
    }
}

/// Space between a callout's border and its text.
pub fn callout_padding(size: f32) -> f32 {
    (size * 0.5).round().max(4.0)
}

/// The box of a callout, grown to fit its text.
pub fn callout_box(fonts: &Fonts, ann: &Annotation, bound_w: f32) -> Rect {
    Rect::from_min_size(ann.rect().min, text_box_size(fonts, ann, bound_w))
}

/// Text color of a callout: readable on its fill, or the border color without a fill.
pub fn callout_text_color(st: &Style) -> Color32 {
    if st.fill.a() >= 128 {
        contrast_color(st.fill)
    } else {
        st.stroke
    }
}

/// Em size of a step marker's number: as large as fits in the circle.
pub fn step_text_size(radius: f32, n: u32) -> f32 {
    match n {
        0..=9 => radius * 1.5,
        10..=99 => radius * 1.18,
        _ => radius * 0.9,
    }
}

/// Underline / strikethrough offset from the baseline (positive = down) and thickness.
pub fn underline_metrics(size: f32) -> (f32, f32) {
    (size * 0.11, (size * 0.065).max(1.0))
}
pub fn strike_metrics(size: f32) -> (f32, f32) {
    (-size * 0.27, (size * 0.065).max(1.0))
}

pub struct Renderer {
    base: Pixmap,
    pub fonts: Arc<Fonts>,
}

fn premultiply(img: &RgbaImage) -> Vec<u8> {
    let mut data = img.as_raw().clone();
    for p in data.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    data
}

impl Renderer {
    pub fn new(img: &RgbaImage, fonts: Arc<Fonts>) -> Self {
        let size = tiny_skia::IntSize::from_wh(img.width().max(1), img.height().max(1)).unwrap();
        let base = Pixmap::from_vec(premultiply(img), size).expect("pixmap");
        Self { base, fonts }
    }

    pub fn width(&self) -> u32 {
        self.base.width()
    }
    /// The screenshot without annotations (premultiplied).
    pub fn base(&self) -> &Pixmap {
        &self.base
    }
    pub fn height(&self) -> u32 {
        self.base.height()
    }

    /// Renders all annotations (except `skip`) onto a copy of the base image.
    /// Result is premultiplied RGBA.
    pub fn render(&self, anns: &[Annotation], skip: Option<u64>) -> Pixmap {
        let mut pm = self.base.clone();
        for a in anns {
            if Some(a.id) != skip {
                self.draw(&mut pm, a);
            }
        }
        pm
    }

    /// Final image: rendered, cropped and un-premultiplied.
    pub fn export(&self, anns: &[Annotation], crop: Option<Rect>) -> RgbaImage {
        unpremultiply(&crop_pixmap(&self.render(anns, None), crop))
    }

    fn draw(&self, pm: &mut Pixmap, ann: &Annotation) {
        let st = &ann.style;
        let stroke_paint = paint(st.stroke);
        let stroke = Stroke {
            width: st.width.max(0.5),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        let t = Transform::identity();
        match &ann.kind {
            Kind::Rect | Kind::Ellipse => {
                let r = ann.rect();
                let Some(sr) = tiny_skia::Rect::from_ltrb(
                    r.min.x,
                    r.min.y,
                    r.max.x.max(r.min.x + 1.0),
                    r.max.y.max(r.min.y + 1.0),
                ) else {
                    return;
                };
                let path = if matches!(ann.kind, Kind::Rect) {
                    Some(PathBuilder::from_rect(sr))
                } else {
                    PathBuilder::from_oval(sr)
                };
                if let Some(path) = path {
                    if st.fill.a() > 0 {
                        pm.fill_path(&path, &paint(st.fill), FillRule::Winding, t, None);
                    }
                    if st.width > 0.0 && st.stroke.a() > 0 {
                        let s = Stroke {
                            line_join: if matches!(ann.kind, Kind::Rect) { LineJoin::Miter } else { LineJoin::Round },
                            ..stroke
                        };
                        pm.stroke_path(&path, &stroke_paint, &s, t, None);
                    }
                }
            }
            Kind::Line => {
                let mut pb = PathBuilder::new();
                pb.move_to(ann.a.x, ann.a.y);
                pb.line_to(ann.b.x, ann.b.y);
                if let Some(p) = pb.finish() {
                    pm.stroke_path(&p, &stroke_paint, &stroke, t, None);
                }
            }
            Kind::Arrow => {
                let ((s0, s1), heads) = arrow_geometry(ann.a, ann.b, st.width, st.head, st.both_ends);
                let mut pb = PathBuilder::new();
                pb.move_to(s0.x, s0.y);
                pb.line_to(s1.x, s1.y);
                if let Some(p) = pb.finish() {
                    pm.stroke_path(&p, &stroke_paint, &stroke, t, None);
                }
                for h in heads {
                    match h {
                        HeadShape::Fill(pts) => {
                            if let Some(p) = polygon(&pts, true) {
                                pm.fill_path(&p, &stroke_paint, FillRule::Winding, t, None);
                                let s = Stroke {
                                    width: (st.width * 0.5).max(1.0),
                                    line_join: LineJoin::Round,
                                    ..Default::default()
                                };
                                pm.stroke_path(&p, &stroke_paint, &s, t, None);
                            }
                        }
                        HeadShape::Stroke(pts) => {
                            if let Some(p) = polygon(&pts, false) {
                                pm.stroke_path(&p, &stroke_paint, &stroke, t, None);
                            }
                        }
                        HeadShape::Dot(c, r) => {
                            if let Some(p) = PathBuilder::from_circle(c.x, c.y, r) {
                                pm.fill_path(&p, &stroke_paint, FillRule::Winding, t, None);
                            }
                        }
                    }
                }
            }
            Kind::Freehand(pts) => {
                if pts.is_empty() {
                    return;
                }
                let mut pb = PathBuilder::new();
                pb.move_to(pts[0].x, pts[0].y);
                if pts.len() == 1 {
                    pb.line_to(pts[0].x + 0.01, pts[0].y);
                }
                // Quadratic smoothing through midpoints.
                for w in pts.windows(2) {
                    let mid = w[0] + (w[1] - w[0]) * 0.5;
                    pb.quad_to(w[0].x, w[0].y, mid.x, mid.y);
                }
                let last = pts[pts.len() - 1];
                pb.line_to(last.x, last.y);
                if let Some(p) = pb.finish() {
                    pm.stroke_path(&p, &stroke_paint, &stroke, t, None);
                }
            }
            Kind::Highlight => {
                let r = ann.rect();
                if let Some(sr) = tiny_skia::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y) {
                    let mut p = paint(st.stroke);
                    p.blend_mode = BlendMode::Multiply;
                    pm.fill_rect(sr, &p, t, None);
                }
            }
            Kind::Redact(mode) => {
                let (w, h) = (pm.width(), pm.height());
                let (x, y, rw, rh) = crop_bounds(Some(ann.rect()), w, h);
                if rw > 0 && rh > 0 && ann.rect().width() >= 1.0 && ann.rect().height() >= 1.0 {
                    redact(pm.data_mut(), w as usize, (x, y, rw, rh), *mode, st.strength, ann.id);
                }
            }
            Kind::Step(n) => {
                let r = ann.step_radius();
                if let Some(path) = PathBuilder::from_circle(ann.a.x, ann.a.y, r) {
                    pm.fill_path(&path, &stroke_paint, FillRule::Winding, t, None);
                    let ring = Stroke { width: (r * 0.12).max(1.5), ..Default::default() };
                    pm.stroke_path(&path, &paint(Color32::WHITE), &ring, t, None);
                }
                let text = n.to_string();
                let size = step_text_size(r, *n);
                let font = &self.fonts.bold;
                let (glyphs, _, sz) = layout(font, &text, size, Pos2::ZERO, None);
                // Center the digits' ink, not the line box (which includes room for descenders).
                let mut ink = Rect::NOTHING;
                for g in &glyphs {
                    if let Some(o) = font.outline_glyph(g.glyph.clone()) {
                        let b = o.px_bounds();
                        ink = ink.union(Rect::from_min_max(pos2(b.min.x, b.min.y), pos2(b.max.x, b.max.y)));
                    }
                }
                let center = if ink.is_positive() { ink.center() } else { (sz * 0.5).to_pos2() };
                let origin = ann.a - center.to_vec2();
                let fg = contrast_color(st.stroke);
                self.draw_text(pm, font, &text, size, origin, fg);
            }
            Kind::Shape(k) => {
                let r = ann.rect();
                if r.width() < 1.0 || r.height() < 1.0 {
                    return;
                }
                let (outline, extra) = shape_outline(*k, r);
                if let Some(path) = polygon(&outline, true) {
                    if st.fill.a() > 0 {
                        pm.fill_path(&path, &paint(st.fill), FillRule::Winding, t, None);
                    }
                    if st.width > 0.0 && st.stroke.a() > 0 {
                        pm.stroke_path(&path, &stroke_paint, &stroke, t, None);
                        for line in extra {
                            if let Some(p) = polygon(&line, false) {
                                pm.stroke_path(&p, &stroke_paint, &stroke, t, None);
                            }
                        }
                    }
                }
            }
            Kind::Callout { text, tail } => {
                let size = st.font_size;
                let r = callout_box(&self.fonts, ann, self.base.width() as f32);
                if let Some(path) = polygon(&callout_outline(r, *tail, ann.callout_radius()), true) {
                    if st.fill.a() > 0 {
                        pm.fill_path(&path, &paint(st.fill), FillRule::Winding, t, None);
                    }
                    if st.width > 0.0 && st.stroke.a() > 0 {
                        pm.stroke_path(&path, &stroke_paint, &stroke, t, None);
                    }
                }
                let pad = callout_padding(size);
                let font = self.fonts.font(st.bold, st.italic);
                let wrap = Some(text_wrap_width(ann, self.base.width() as f32));
                draw_text_wrapped(pm, font, text, size, r.min + Vec2::splat(pad), callout_text_color(st), wrap);
            }
            Kind::Emoji(id) => {
                let r = ann.rect();
                let side = r.width().min(r.height());
                if side >= 1.0 {
                    let c = r.center();
                    crate::emoji::draw(pm, id, c.x - side / 2.0, c.y - side / 2.0, side);
                }
            }
            Kind::Text(text) => {
                let size = st.font_size;
                let pad = text_padding(size);
                if st.fill.a() > 0 {
                    let bs = text_box_size(&self.fonts, ann, self.base.width() as f32);
                    let r = Rect::from_min_size(ann.a, bs);
                    if let Some(path) = rounded_rect(r, pad * 0.8) {
                        pm.fill_path(&path, &paint(st.fill), FillRule::Winding, t, None);
                    }
                }
                let font = self.fonts.font(st.bold, st.italic);
                let origin = ann.a + Vec2::splat(pad);
                let wrap = Some(text_wrap_width(ann, self.base.width() as f32));
                draw_text_wrapped(pm, font, text, size, origin, st.stroke, wrap);
                if st.underline || st.strike {
                    let (_, lines, _) = layout(font, text, size, origin, wrap);
                    let p = paint(st.stroke);
                    for l in lines.iter().filter(|l| l.x1 > l.x0) {
                        let mut deco = Vec::new();
                        if st.underline {
                            deco.push(underline_metrics(size));
                        }
                        if st.strike {
                            deco.push(strike_metrics(size));
                        }
                        for (off, th) in deco {
                            let y = l.baseline + off - th / 2.0;
                            if let Some(r) = tiny_skia::Rect::from_xywh(l.x0, y, l.x1 - l.x0, th) {
                                pm.fill_rect(r, &p, t, None);
                            }
                        }
                    }
                }
            }
        }
    }

    fn draw_text(&self, pm: &mut Pixmap, font: &FontArc, text: &str, size: f32, origin: Pos2, color: Color32) {
        draw_text_px(pm, font, text, size, origin, color);
    }
}

/// Size of laid-out text (no padding), in pixels.
pub fn text_extent(font: &FontArc, text: &str, size: f32) -> Vec2 {
    layout(font, text, size, Pos2::ZERO, None).2
}

/// Draws text with its top-left at `origin`, in pixmap pixels.
pub fn draw_text_px(pm: &mut Pixmap, font: &FontArc, text: &str, size: f32, origin: Pos2, color: Color32) {
    draw_text_wrapped(pm, font, text, size, origin, color, None);
}

/// Like `draw_text_px`, wrapping lines at `wrap` pixels.
fn draw_text_wrapped(
    pm: &mut Pixmap,
    font: &FontArc,
    text: &str,
    size: f32,
    origin: Pos2,
    color: Color32,
    wrap: Option<f32>,
) {
    let (glyphs, _, _) = layout(font, text, size, origin, wrap);
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let [cr, cg, cb, ca] = color.to_srgba_unmultiplied();
    for g in glyphs {
        let Some(outline) = font.outline_glyph(g.glyph) else { continue };
        let b = outline.px_bounds();
        outline.draw(|gx, gy, cov| {
            let x = b.min.x as i32 + gx as i32;
            let y = b.min.y as i32 + gy as i32;
            if x < 0 || y < 0 || x >= w || y >= h {
                return;
            }
            let sa = cov.clamp(0.0, 1.0) * ca as f32 / 255.0;
            if sa <= 0.0 {
                return;
            }
            let i = ((y * w + x) * 4) as usize;
            let px = &mut data[i..i + 4];
            let inv = 1.0 - sa;
            px[0] = (cr as f32 * sa + px[0] as f32 * inv).round() as u8;
            px[1] = (cg as f32 * sa + px[1] as f32 * inv).round() as u8;
            px[2] = (cb as f32 * sa + px[2] as f32 * inv).round() as u8;
            px[3] = (255.0 * sa + px[3] as f32 * inv).round() as u8;
        });
    }
}

pub fn contrast_color(bg: Color32) -> Color32 {
    let l = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if l > 165.0 {
        Color32::from_rgb(20, 20, 20)
    } else {
        Color32::WHITE
    }
}

fn paint(c: Color32) -> Paint<'static> {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let mut p = Paint::default();
    p.set_color_rgba8(r, g, b, a);
    p.anti_alias = true;
    p
}

fn polygon(pts: &[Pos2], close: bool) -> Option<tiny_skia::Path> {
    let (first, rest) = pts.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.x, first.y);
    for p in rest {
        pb.line_to(p.x, p.y);
    }
    if close {
        pb.close();
    }
    pb.finish()
}

pub fn rounded_rect(r: Rect, radius: f32) -> Option<tiny_skia::Path> {
    let rad = radius.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
    let (l, t, rr, b) = (r.min.x, r.min.y, r.max.x, r.max.y);
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    pb.line_to(rr - rad, t);
    pb.quad_to(rr, t, rr, t + rad);
    pb.line_to(rr, b - rad);
    pb.quad_to(rr, b, rr - rad, b);
    pb.line_to(l + rad, b);
    pb.quad_to(l, b, l, b - rad);
    pb.line_to(l, t + rad);
    pb.quad_to(l, t, l + rad, t);
    pb.close();
    pb.finish()
}

/// Copies the `crop` part of a pixmap (whole pixmap for `None`).
pub fn crop_pixmap(pm: &Pixmap, crop: Option<Rect>) -> Pixmap {
    let (w, h) = (pm.width(), pm.height());
    let r = crop_bounds(crop, w, h);
    if r == (0, 0, w, h) {
        return pm.clone();
    }
    tiny_skia::IntRect::from_xywh(r.0 as i32, r.1 as i32, r.2.max(1), r.3.max(1))
        .and_then(|ir| pm.clone_rect(ir))
        .unwrap_or_else(|| pm.clone())
}

/// Premultiplied pixmap -> straight-alpha image.
pub fn unpremultiply(pm: &Pixmap) -> RgbaImage {
    let mut out = RgbaImage::from_raw(pm.width(), pm.height(), pm.data().to_vec()).expect("pixmap size");
    for p in out.as_mut().chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 255 && a != 0 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    out
}

/// Integer (x, y, w, h) of `r` clamped to the image; whole image for `None`.
pub fn crop_bounds(r: Option<Rect>, w: u32, h: u32) -> (u32, u32, u32, u32) {
    let Some(r) = r else { return (0, 0, w, h) };
    let x0 = r.min.x.round().clamp(0.0, w as f32) as u32;
    let y0 = r.min.y.round().clamp(0.0, h as f32) as u32;
    let x1 = r.max.x.round().clamp(0.0, w as f32) as u32;
    let y1 = r.max.y.round().clamp(0.0, h as f32) as u32;
    (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in [-amp, amp].
    fn noise(&mut self, amp: i32) -> i32 {
        if amp == 0 {
            return 0;
        }
        (self.next() % (2 * amp as u64 + 1)) as i32 - amp
    }
}

/// Destroys the information inside `rect` of a premultiplied RGBA buffer.
///
/// Blur is not a plain Gaussian (which can partially be reversed and is what makes
/// weak blurs readable): the region is first reduced to blocks, then box-blurred three
/// times with a radius that scales with the region, then noise is added. What remains
/// holds far too little information to reconstruct text.
pub fn redact(
    data: &mut [u8],
    stride_px: usize,
    rect: (u32, u32, u32, u32),
    mode: RedactMode,
    strength: u8,
    seed: u64,
) {
    let (x0, y0, w, h) = (rect.0 as usize, rect.1 as usize, rect.2 as usize, rect.3 as usize);
    let strength = strength.clamp(1, 10) as usize;
    let min_dim = w.min(h);
    let mut rng = Rng::new(seed ^ ((x0 as u64) << 32) ^ y0 as u64);
    let idx = |x: usize, y: usize| ((y0 + y) * stride_px + x0 + x) * 4;

    match mode {
        RedactMode::Solid => {
            for y in 0..h {
                for x in 0..w {
                    let i = idx(x, y);
                    data[i..i + 4].copy_from_slice(&[17, 17, 17, 255]);
                }
            }
        }
        RedactMode::Pixelate => {
            let block = (4 + strength * 2).max(min_dim * strength / 20).max(4);
            let amp = (6 + strength * 2) as i32;
            pixelate(data, w, h, block, &idx, Some((&mut rng, amp)));
        }
        RedactMode::Blur => {
            let radius = (2 + strength * 2).max(min_dim * strength / 25).max(4);
            pixelate(data, w, h, (radius / 2).max(2), &idx, None);
            let mut buf: Vec<[i32; 4]> = Vec::with_capacity(w * h);
            for y in 0..h {
                for x in 0..w {
                    let i = idx(x, y);
                    buf.push([data[i] as i32, data[i + 1] as i32, data[i + 2] as i32, data[i + 3] as i32]);
                }
            }
            for _ in 0..3 {
                box_blur_h(&mut buf, w, h, radius);
                box_blur_v(&mut buf, w, h, radius);
            }
            let amp = (3 + strength) as i32;
            for y in 0..h {
                for x in 0..w {
                    let p = buf[y * w + x];
                    let n = rng.noise(amp);
                    let i = idx(x, y);
                    let a = p[3].clamp(0, 255);
                    for c in 0..3 {
                        data[i + c] = (p[c] + n).clamp(0, a) as u8;
                    }
                    data[i + 3] = a as u8;
                }
            }
        }
    }
}

fn pixelate(
    data: &mut [u8],
    w: usize,
    h: usize,
    block: usize,
    idx: &dyn Fn(usize, usize) -> usize,
    mut noise: Option<(&mut Rng, i32)>,
) {
    let mut by = 0;
    while by < h {
        let bh = block.min(h - by);
        let mut bx = 0;
        while bx < w {
            let bw = block.min(w - bx);
            let mut sum = [0u64; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = idx(x, y);
                    for c in 0..4 {
                        sum[c] += data[i + c] as u64;
                    }
                }
            }
            let n = (bw * bh) as u64;
            let mut avg = [0i32; 4];
            for c in 0..4 {
                avg[c] = (sum[c] / n) as i32;
            }
            if let Some((rng, amp)) = noise.as_mut() {
                let d = rng.noise(*amp);
                for c in 0..3 {
                    avg[c] = (avg[c] + d + rng.noise(*amp / 3)).clamp(0, avg[3]);
                }
            }
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let i = idx(x, y);
                    for c in 0..4 {
                        data[i + c] = avg[c] as u8;
                    }
                }
            }
            bx += block;
        }
        by += block;
    }
}

fn box_blur_h(buf: &mut [[i32; 4]], w: usize, h: usize, r: usize) {
    let mut line = vec![[0i32; 4]; w];
    let win = (2 * r + 1) as i32;
    for y in 0..h {
        let row = &buf[y * w..(y + 1) * w];
        let at = |x: isize| row[x.clamp(0, w as isize - 1) as usize];
        let mut acc = [0i32; 4];
        for k in -(r as isize)..=(r as isize) {
            let p = at(k);
            for c in 0..4 {
                acc[c] += p[c];
            }
        }
        for x in 0..w {
            for c in 0..4 {
                line[x][c] = acc[c] / win;
            }
            let add = at(x as isize + r as isize + 1);
            let sub = at(x as isize - r as isize);
            for c in 0..4 {
                acc[c] += add[c] - sub[c];
            }
        }
        buf[y * w..(y + 1) * w].copy_from_slice(&line);
    }
}

fn box_blur_v(buf: &mut [[i32; 4]], w: usize, h: usize, r: usize) {
    let mut col = vec![[0i32; 4]; h];
    let win = (2 * r + 1) as i32;
    for x in 0..w {
        let at = |y: isize| buf[y.clamp(0, h as isize - 1) as usize * w + x];
        let mut acc = [0i32; 4];
        for k in -(r as isize)..=(r as isize) {
            let p = at(k);
            for c in 0..4 {
                acc[c] += p[c];
            }
        }
        for y in 0..h {
            for c in 0..4 {
                col[y][c] = acc[c] / win;
            }
            let add = at(y as isize + r as isize + 1);
            let sub = at(y as isize - r as isize);
            for c in 0..4 {
                acc[c] += add[c] - sub[c];
            }
        }
        for y in 0..h {
            buf[y * w + x] = col[y];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annot::Style;
    use eframe::egui::pos2;

    fn checker(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            if (x / 2 + y / 3) % 2 == 0 {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        })
    }

    /// Mean absolute difference between neighbouring pixels: a proxy for remaining detail.
    fn detail(img: &RgbaImage, r: (u32, u32, u32, u32)) -> f64 {
        let mut total = 0f64;
        let mut n = 0f64;
        for y in r.1..r.1 + r.3 {
            for x in r.0..r.0 + r.2 - 1 {
                let a = img.get_pixel(x, y).0[0] as f64;
                let b = img.get_pixel(x + 1, y).0[0] as f64;
                total += (a - b).abs();
                n += 1.0;
            }
        }
        total / n
    }

    fn redacted(mode: RedactMode) -> RgbaImage {
        let img = checker(200, 60);
        let r = Renderer::new(&img, Fonts::load());
        let ann = Annotation {
            id: 7,
            kind: Kind::Redact(mode),
            a: pos2(10.0, 20.0),
            b: pos2(190.0, 40.0),
            style: Style::default(),
        };
        r.export(&[ann], None)
    }

    #[test]
    fn redaction_removes_detail_and_stays_inside() {
        let orig = checker(200, 60);
        let region = (10, 20, 180, 20);
        assert!(detail(&orig, region) > 100.0);
        for mode in [RedactMode::Blur, RedactMode::Pixelate, RedactMode::Solid] {
            let out = redacted(mode);
            let d = detail(&out, region);
            assert!(d < 12.0, "{mode:?} leaves too much detail: {d}");
            // Outside the region nothing changes.
            assert_eq!(out.get_pixel(5, 5), orig.get_pixel(5, 5));
            assert_eq!(out.get_pixel(100, 50), orig.get_pixel(100, 50));
            // Deterministic (no flicker on re-render).
            assert!(out == redacted(mode));
        }
    }

    #[test]
    fn renders_shapes_and_text() {
        let img = RgbaImage::from_pixel(300, 200, image::Rgba([255, 255, 255, 255]));
        let r = Renderer::new(&img, Fonts::load());
        let style = Style::default();
        let mk = |kind, a, b| Annotation { id: 1, kind, a, b, style };
        let anns = vec![
            mk(Kind::Rect, pos2(10.0, 10.0), pos2(100.0, 60.0)),
            mk(Kind::Arrow, pos2(120.0, 20.0), pos2(250.0, 90.0)),
            mk(Kind::Text("Hello".into()), pos2(20.0, 120.0), pos2(0.0, 0.0)),
            mk(Kind::Step(3), pos2(250.0, 160.0), pos2(0.0, 0.0)),
        ];
        let out = r.export(&anns, Some(Rect::from_min_max(pos2(5.0, 5.0), pos2(295.0, 195.0))));
        assert_eq!(out.dimensions(), (290, 190));
        let reds = out.pixels().filter(|p| p.0[0] > 200 && p.0[1] < 100).count();
        assert!(reds > 500, "expected red strokes, got {reds}");
    }

    #[test]
    fn text_wraps_at_image_edge_and_in_sized_box() {
        let fonts = Fonts::load();
        let style = Style { font_size: 20.0, ..Default::default() };
        let words = "wrap this rather long line of text please";
        let mk = |a: Pos2, b: Pos2| Annotation { id: 1, kind: Kind::Text(words.into()), a, b, style };
        let one_line = text_box_size(&fonts, &mk(pos2(0.0, 0.0), pos2(0.0, 0.0)), 5000.0);
        // Automatic width: stays inside the image and gets taller.
        let auto = mk(pos2(100.0, 10.0), pos2(100.0, 10.0));
        let bs = text_box_size(&fonts, &auto, 300.0);
        assert!(100.0 + bs.x <= 300.0 + 0.5, "box {} runs past the edge", bs.x);
        assert!(bs.y > one_line.y * 1.5, "expected several lines, height {}", bs.y);
        // Sized by hand: exactly that wide, wrapped inside it.
        let fixed = mk(pos2(10.0, 10.0), pos2(170.0, 10.0));
        let bs = text_box_size(&fonts, &fixed, 5000.0);
        assert_eq!(bs.x, 160.0);
        assert!(bs.y > one_line.y * 1.5);
        // Rendered ink stays inside the sized box.
        let img = RgbaImage::from_pixel(400, 200, image::Rgba([255, 255, 255, 255]));
        let out = Renderer::new(&img, fonts).export(&[fixed], None);
        let ink_right = out.enumerate_pixels().filter(|(_, _, p)| p.0[1] < 200).map(|(x, ..)| x).max().unwrap();
        assert!(ink_right <= 170, "ink reaches x={ink_right}");
    }

    #[test]
    fn long_word_breaks_between_chars() {
        let chars: Vec<char> = "abcdefghij".chars().collect();
        let adv = vec![10.0; chars.len()];
        assert_eq!(break_lines(&chars, &adv, Some(35.0)), vec![(0, 3), (3, 6), (6, 9), (9, 10)]);
        let chars: Vec<char> = "aa bb cc".chars().collect();
        let adv = vec![10.0; chars.len()];
        assert_eq!(break_lines(&chars, &adv, Some(50.0)), vec![(0, 6), (6, 8)]);
        assert_eq!(break_lines(&chars, &adv, None), vec![(0, 8)]);
    }

    #[test]
    fn step_number_is_large_and_centered() {
        let img = RgbaImage::from_pixel(200, 200, image::Rgba([255, 255, 255, 255]));
        let r = Renderer::new(&img, Fonts::load());
        let style = Style { stroke: Color32::from_rgb(20, 20, 200), font_size: 40.0, ..Default::default() };
        let ann = Annotation { id: 1, kind: Kind::Step(4), a: pos2(100.0, 100.0), b: pos2(100.0, 100.0), style };
        let out = r.export(&[ann.clone()], None);
        // The digit is white on the blue disc: find its ink inside the circle.
        let rad = ann.step_radius() * 0.8;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, 0.0f32, 0.0f32);
        for (x, y, p) in out.enumerate_pixels() {
            let (fx, fy) = (x as f32, y as f32);
            if ((fx - 100.0).powi(2) + (fy - 100.0).powi(2)).sqrt() < rad && p.0[0] > 180 {
                (x0, y0, x1, y1) = (x0.min(fx), y0.min(fy), x1.max(fx), y1.max(fy));
            }
        }
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        assert!((cx - 100.0).abs() < 2.0 && (cy - 100.0).abs() < 2.0, "ink center {cx},{cy}");
        let height = y1 - y0;
        assert!(height > ann.step_radius() * 0.9, "digit height {height} vs radius {}", ann.step_radius());
    }

    #[test]
    fn arrow_heads_render_at_both_ends() {
        let img = RgbaImage::from_pixel(300, 100, image::Rgba([255, 255, 255, 255]));
        let r = Renderer::new(&img, Fonts::load());
        for head in crate::annot::ArrowHead::ALL {
            let style = Style { head, both_ends: true, ..Default::default() };
            let ann = Annotation { id: 1, kind: Kind::Arrow, a: pos2(30.0, 50.0), b: pos2(270.0, 50.0), style };
            let out = r.export(&[ann], None);
            // Heads are wider than the shaft: there is red well above the line at both ends.
            let red =
                |xs: std::ops::Range<u32>| xs.into_iter().any(|x| (30..44).any(|y| out.get_pixel(x, y).0[1] < 120));
            assert!(red(30..60) && red(240..270), "{head:?}");
            // ...and not in the middle of the shaft.
            assert!(!red(120..180), "{head:?}");
        }
    }
}
