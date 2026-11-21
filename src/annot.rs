//! Annotation model shared by the editor (preview / interaction) and the renderer (export).
//! All coordinates are in image pixels.

use eframe::egui::{Color32, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RedactMode {
    /// Pixelate + heavy blur + noise. Unreadable, looks like frosted glass.
    Blur,
    /// Large blocks with per-block noise.
    Pixelate,
    /// Opaque box. The only mode that is 100% irreversible by construction.
    Solid,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Rect,
    Ellipse,
    Arrow,
    Line,
    Freehand(Vec<Pos2>),
    Text(String),
    Highlight,
    Redact(RedactMode),
    /// Numbered step marker.
    Step(u32),
    /// Star, hexagon, flowchart shape, ... in the rect `a`-`b`.
    Shape(ShapeKind),
    /// Text box with a pointer: the box is `a`-`b` (it grows to fit the text), the pointer
    /// ends at `tail`.
    Callout {
        text: String,
        tail: Pos2,
    },
    /// Emoji stamp (Twemoji file id, e.g. "1f44d"), drawn in the square centered in `a`-`b`.
    Emoji(String),
    /// The mouse pointer captured with the screenshot, stretched over `a`-`b`.
    Pointer(std::sync::Arc<image::RgbaImage>),
}

impl Kind {
    /// The text of a text or callout annotation.
    pub fn text(&self) -> Option<&String> {
        match self {
            Kind::Text(t) | Kind::Callout { text: t, .. } => Some(t),
            _ => None,
        }
    }
    pub fn text_mut(&mut self) -> Option<&mut String> {
        match self {
            Kind::Text(t) | Kind::Callout { text: t, .. } => Some(t),
            _ => None,
        }
    }
}

/// The shapes of the Shapes tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Star,
    Hexagon,
    Pentagon,
    Triangle,
    /// Flowchart: decision.
    Diamond,
    /// Flowchart: start / end.
    Terminator,
    /// Flowchart: input / output.
    Parallelogram,
    /// Flowchart: database.
    Cylinder,
    /// Flowchart: document.
    Document,
}

impl ShapeKind {
    pub const ALL: [ShapeKind; 9] = [
        ShapeKind::Star,
        ShapeKind::Hexagon,
        ShapeKind::Pentagon,
        ShapeKind::Triangle,
        ShapeKind::Diamond,
        ShapeKind::Terminator,
        ShapeKind::Parallelogram,
        ShapeKind::Cylinder,
        ShapeKind::Document,
    ];
    /// English name (translate with `i18n::t`).
    pub fn name(self) -> &'static str {
        match self {
            ShapeKind::Star => "Star",
            ShapeKind::Hexagon => "Hexagon",
            ShapeKind::Pentagon => "Pentagon",
            ShapeKind::Triangle => "Triangle",
            ShapeKind::Diamond => "Decision (diamond)",
            ShapeKind::Terminator => "Start / end",
            ShapeKind::Parallelogram => "Input / output",
            ShapeKind::Cylinder => "Database",
            ShapeKind::Document => "Document",
        }
    }
    /// Whether the outline is convex (egui can only fill convex polygons in the live preview).
    pub fn convex(self) -> bool {
        !matches!(self, ShapeKind::Star | ShapeKind::Document)
    }
}

/// Outline of a shape in `r`, as a closed polygon (curves sampled), plus open lines drawn on
/// top (the front edge of the cylinder's lid).
pub fn shape_outline(kind: ShapeKind, r: Rect) -> (Vec<Pos2>, Vec<Vec<Pos2>>) {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    let c = r.center();
    let (hw, hh) = (r.width() / 2.0, r.height() / 2.0);
    let at = |x: f32, y: f32| Pos2::new(r.min.x + x * r.width(), r.min.y + y * r.height());
    let regular = |n: usize, inner: Option<f32>| -> Vec<Pos2> {
        let k = if inner.is_some() { n * 2 } else { n };
        // Points on the unit circle, then stretched so the polygon fills the rect exactly.
        let raw: Vec<Vec2> = (0..k)
            .map(|i| {
                let a = -FRAC_PI_2 + TAU * i as f32 / k as f32;
                let rad = match inner {
                    Some(f) if i % 2 == 1 => f,
                    _ => 1.0,
                };
                Vec2::new(a.cos() * rad, a.sin() * rad)
            })
            .collect();
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for v in &raw {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        let span = (hi - lo).max(Vec2::splat(1e-3));
        raw.iter().map(|v| at((v.x - lo.x) / span.x, (v.y - lo.y) / span.y)).collect()
    };
    let arc = |center: Pos2, rx: f32, ry: f32, from: f32, to: f32, n: usize| -> Vec<Pos2> {
        (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32;
                Pos2::new(center.x + rx * a.cos(), center.y + ry * a.sin())
            })
            .collect()
    };
    match kind {
        ShapeKind::Star => (regular(5, Some(0.42)), Vec::new()),
        ShapeKind::Hexagon => {
            (vec![at(0.25, 0.0), at(0.75, 0.0), at(1.0, 0.5), at(0.75, 1.0), at(0.25, 1.0), at(0.0, 0.5)], Vec::new())
        }
        ShapeKind::Pentagon => (regular(5, None), Vec::new()),
        ShapeKind::Triangle => (vec![at(0.5, 0.0), at(1.0, 1.0), at(0.0, 1.0)], Vec::new()),
        ShapeKind::Diamond => (vec![at(0.5, 0.0), at(1.0, 0.5), at(0.5, 1.0), at(0.0, 0.5)], Vec::new()),
        ShapeKind::Terminator => {
            let rad = hh.min(hw);
            let mut pts = arc(Pos2::new(r.max.x - rad, c.y), rad, rad, -FRAC_PI_2, FRAC_PI_2, 16);
            pts.extend(arc(Pos2::new(r.min.x + rad, c.y), rad, rad, FRAC_PI_2, PI + FRAC_PI_2, 16));
            (pts, Vec::new())
        }
        ShapeKind::Parallelogram => {
            let k = (r.width() * 0.2).min(r.height() * 0.6) / r.width().max(1e-3);
            (vec![at(k, 0.0), at(1.0, 0.0), at(1.0 - k, 1.0), at(0.0, 1.0)], Vec::new())
        }
        ShapeKind::Cylinder => {
            let ry = (r.height() * 0.12).min(r.width() * 0.25);
            let top = r.min.y + ry;
            let bottom = r.max.y - ry;
            // Right side down, the bottom curve, left side up, the back of the lid.
            let mut pts = arc(Pos2::new(c.x, bottom), hw, ry, 0.0, PI, 24);
            pts.extend(arc(Pos2::new(c.x, top), hw, ry, PI, TAU, 24));
            let front = arc(Pos2::new(c.x, top), hw, ry, 0.0, PI, 24);
            (pts, vec![front])
        }
        ShapeKind::Document => {
            let wave = r.height() * 0.1;
            let base = r.max.y - wave;
            let mut pts = vec![r.left_top(), r.right_top(), Pos2::new(r.max.x, base)];
            let n = 32;
            for i in 1..n {
                let t = i as f32 / n as f32;
                let x = r.max.x - t * r.width();
                pts.push(Pos2::new(x, base - (t * TAU).sin() * wave));
            }
            pts.push(Pos2::new(r.min.x, base));
            (pts, Vec::new())
        }
    }
}

/// Outline of a callout: the rounded box `r` with a pointer to `tail`, as one closed polygon
/// (clockwise on screen). No pointer when `tail` is inside the box.
pub fn callout_outline(r: Rect, tail: Pos2, radius: f32) -> Vec<Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let rad = radius.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
    let corner = |c: Pos2, from: f32, out: &mut Vec<Pos2>| {
        for i in 0..=6 {
            let a = from + FRAC_PI_2 * i as f32 / 6.0;
            out.push(Pos2::new(c.x + rad * a.cos(), c.y + rad * a.sin()));
        }
    };
    // Which side the pointer leaves from (0 top, 1 right, 2 bottom, 3 left), or none.
    let side = if r.contains(tail) {
        None
    } else {
        let d = tail - r.center();
        let (nx, ny) = (d.x / r.width().max(1.0), d.y / r.height().max(1.0));
        Some(if nx.abs() > ny.abs() {
            if nx > 0.0 {
                1
            } else {
                3
            }
        } else if ny > 0.0 {
            2
        } else {
            0
        })
    };
    // Base of the pointer on that side: under the tip where possible, as wide as fits.
    let base = |len: f32, lo: f32, at: f32| -> (f32, f32) {
        let half = (len * 0.18).clamp(4.0, 28.0).min((len - 2.0 * rad) / 2.0 - 1.0).max(1.0);
        let mid = at.clamp(lo + rad + half, lo + len - rad - half);
        (mid - half, mid + half)
    };
    let mut pts = Vec::new();
    // Top edge, left to right.
    corner(Pos2::new(r.min.x + rad, r.min.y + rad), PI, &mut pts);
    if side == Some(0) {
        let (x0, x1) = base(r.width(), r.min.x, tail.x);
        pts.extend([Pos2::new(x0, r.min.y), tail, Pos2::new(x1, r.min.y)]);
    }
    corner(Pos2::new(r.max.x - rad, r.min.y + rad), -FRAC_PI_2, &mut pts);
    if side == Some(1) {
        let (y0, y1) = base(r.height(), r.min.y, tail.y);
        pts.extend([Pos2::new(r.max.x, y0), tail, Pos2::new(r.max.x, y1)]);
    }
    corner(Pos2::new(r.max.x - rad, r.max.y - rad), 0.0, &mut pts);
    if side == Some(2) {
        let (x0, x1) = base(r.width(), r.min.x, tail.x);
        pts.extend([Pos2::new(x1, r.max.y), tail, Pos2::new(x0, r.max.y)]);
    }
    corner(Pos2::new(r.min.x + rad, r.max.y - rad), FRAC_PI_2, &mut pts);
    if side == Some(3) {
        let (y0, y1) = base(r.height(), r.min.y, tail.y);
        pts.extend([Pos2::new(r.min.x, y1), tail, Pos2::new(r.min.x, y0)]);
    }
    pts
}

/// Whether `p` is inside the closed polygon `poly` (even-odd).
pub fn point_in_polygon(p: Pos2, poly: &[Pos2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

fn near_polyline(p: Pos2, poly: &[Pos2], closed: bool, w: f32) -> bool {
    let n = poly.len();
    let segs = if closed { n } else { n.saturating_sub(1) };
    (0..segs).any(|i| dist_to_segment(p, poly[i], poly[(i + 1) % n]) <= w)
}

/// Shape at the end(s) of an arrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArrowHead {
    Triangle,
    Open,
    Circle,
    Diamond,
    Bar,
}

impl ArrowHead {
    pub const ALL: [ArrowHead; 5] =
        [ArrowHead::Triangle, ArrowHead::Open, ArrowHead::Circle, ArrowHead::Diamond, ArrowHead::Bar];
    pub fn name(self) -> &'static str {
        match self {
            ArrowHead::Triangle => "Arrow",
            ArrowHead::Open => "Open arrow",
            ArrowHead::Circle => "Dot",
            ArrowHead::Diamond => "Diamond",
            ArrowHead::Bar => "Bar",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    #[serde(with = "color_serde")]
    pub stroke: Color32,
    /// Transparent = no fill.
    #[serde(with = "color_serde")]
    pub fill: Color32,
    pub width: f32,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// 1..=10
    pub strength: u8,
    /// Arrow: head shape, and whether both ends get one.
    pub head: ArrowHead,
    pub both_ends: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            stroke: Color32::from_rgb(239, 68, 68),
            fill: Color32::TRANSPARENT,
            width: 4.0,
            font_size: 28.0,
            bold: true,
            italic: false,
            underline: false,
            strike: false,
            strength: 6,
            head: ArrowHead::Triangle,
            both_ends: false,
        }
    }
}

/// Colors as `[r, g, b, a]` (unmultiplied) in JSON.
pub mod color_serde {
    use eframe::egui::Color32;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(c: &Color32, s: S) -> Result<S::Ok, S::Error> {
        c.to_srgba_unmultiplied().serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color32, D::Error> {
        let [r, g, b, a] = <[u8; 4]>::deserialize(d)?;
        Ok(Color32::from_rgba_unmultiplied(r, g, b, a))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub id: u64,
    pub kind: Kind,
    /// Rect-like: one corner. Line/Arrow: start. Text/Step: anchor (top-left / center).
    pub a: Pos2,
    /// Rect-like: opposite corner. Line/Arrow: end. Unused otherwise.
    pub b: Pos2,
    pub style: Style,
}

impl Annotation {
    pub fn rect(&self) -> Rect {
        Rect::from_two_pos(self.a, self.b)
    }

    pub fn is_rect_like(&self) -> bool {
        matches!(
            self.kind,
            Kind::Rect
                | Kind::Ellipse
                | Kind::Highlight
                | Kind::Redact(_)
                | Kind::Shape(_)
                | Kind::Callout { .. }
                | Kind::Emoji(_)
                | Kind::Pointer(_)
        )
    }

    /// Corner radius of a callout box.
    pub fn callout_radius(&self) -> f32 {
        (self.style.font_size * 0.45).max(4.0)
    }

    pub fn is_segment(&self) -> bool {
        matches!(self.kind, Kind::Arrow | Kind::Line)
    }

    pub fn translate(&mut self, d: Vec2) {
        self.a += d;
        self.b += d;
        match &mut self.kind {
            Kind::Freehand(pts) => {
                for p in pts {
                    *p += d;
                }
            }
            Kind::Callout { tail, .. } => *tail += d,
            _ => {}
        }
    }

    pub fn step_radius(&self) -> f32 {
        (self.style.font_size * 0.75).max(10.0)
    }

    /// Bounding box in image pixels (text needs measured size from the renderer).
    pub fn bounds(&self, text_size: impl Fn(&Annotation) -> Vec2) -> Rect {
        let pad = self.style.width / 2.0 + 1.0;
        match &self.kind {
            Kind::Freehand(pts) => {
                let mut r = Rect::NOTHING;
                for p in pts {
                    r.extend_with(*p);
                }
                r.expand(pad)
            }
            Kind::Text(_) => Rect::from_min_size(self.a, text_size(self)),
            Kind::Callout { tail, .. } => {
                Rect::from_min_size(self.rect().min, text_size(self)).union(Rect::from_pos(*tail)).expand(pad)
            }
            Kind::Step(_) => Rect::from_center_size(self.a, Vec2::splat(self.step_radius() * 2.0)),
            Kind::Arrow | Kind::Line => self.rect().expand(pad + self.style.width * 2.0),
            _ => self.rect().expand(pad),
        }
    }

    /// Hit test; `tol` is the click tolerance in image pixels.
    pub fn hit(&self, p: Pos2, tol: f32, text_size: impl Fn(&Annotation) -> Vec2) -> bool {
        let w = self.style.width / 2.0 + tol;
        match &self.kind {
            Kind::Rect => {
                let r = self.rect();
                if self.style.fill.a() > 0 {
                    r.expand(w).contains(p)
                } else {
                    r.expand(w).contains(p) && !r.shrink(w).contains(p)
                }
            }
            Kind::Ellipse => {
                let r = self.rect();
                let c = r.center();
                let rx = (r.width() / 2.0).max(1.0);
                let ry = (r.height() / 2.0).max(1.0);
                let d = ((p.x - c.x) / rx).powi(2) + ((p.y - c.y) / ry).powi(2);
                let k = w / rx.min(ry);
                if self.style.fill.a() > 0 {
                    d <= (1.0 + k).powi(2)
                } else {
                    d <= (1.0 + k).powi(2) && d >= (1.0 - k).max(0.0).powi(2)
                }
            }
            Kind::Arrow | Kind::Line => dist_to_segment(p, self.a, self.b) <= w,
            Kind::Freehand(pts) => pts.windows(2).any(|s| dist_to_segment(p, s[0], s[1]) <= w),
            Kind::Highlight | Kind::Redact(_) => self.rect().expand(tol).contains(p),
            Kind::Text(_) | Kind::Step(_) => self.bounds(text_size).expand(tol).contains(p),
            Kind::Emoji(_) | Kind::Pointer(_) => self.rect().expand(tol).contains(p),
            Kind::Shape(k) => {
                let (outline, _) = shape_outline(*k, self.rect());
                point_in_polygon(p, &outline) || near_polyline(p, &outline, true, w)
            }
            Kind::Callout { tail, .. } => {
                let r = Rect::from_min_size(self.rect().min, text_size(self));
                let outline = callout_outline(r, *tail, self.callout_radius());
                point_in_polygon(p, &outline) || near_polyline(p, &outline, true, w)
            }
        }
    }
}

pub fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// One drawable part of an arrow head.
#[derive(Clone, Debug, PartialEq)]
pub enum HeadShape {
    /// Filled polygon (with a thin outline to round its corners).
    Fill(Vec<Pos2>),
    /// Stroked open polyline, drawn with the arrow's line width.
    Stroke(Vec<Pos2>),
    /// Filled circle: center, radius.
    Dot(Pos2, f32),
}

/// Geometry of an arrow: the shaft (start, end) and the head shapes.
pub fn arrow_geometry(a: Pos2, b: Pos2, width: f32, head: ArrowHead, both: bool) -> ((Pos2, Pos2), Vec<HeadShape>) {
    let len = (b - a).length();
    if len < 1e-3 {
        return ((a, b), Vec::new());
    }
    let share = if both { 0.4 } else { 0.6 };
    let head_len = (width * 3.5 + 8.0).min(len * share).max(1.0);
    let mut shapes = Vec::new();
    let end = |tip: Pos2, dir: Vec2, shapes: &mut Vec<HeadShape>| -> Pos2 {
        let n = Vec2::new(-dir.y, dir.x);
        let hw = head_len * 0.75;
        let base = tip - dir * head_len;
        match head {
            ArrowHead::Triangle => {
                shapes.push(HeadShape::Fill(vec![tip, base + n * hw, base - n * hw]));
                tip - dir * (head_len * 0.6)
            }
            ArrowHead::Open => {
                let inset = dir * (width * 0.5);
                shapes.push(HeadShape::Stroke(vec![
                    base + n * hw * 0.9 - inset,
                    tip - inset,
                    base - n * hw * 0.9 - inset,
                ]));
                tip - dir * (width * 0.6)
            }
            ArrowHead::Circle => {
                let r = (width * 1.6 + 3.0).min(head_len * 0.6);
                let c = tip - dir * r;
                shapes.push(HeadShape::Dot(c, r));
                c
            }
            ArrowHead::Diamond => {
                let mid = tip - dir * (head_len * 0.5);
                shapes.push(HeadShape::Fill(vec![tip, mid + n * hw * 0.6, base, mid - n * hw * 0.6]));
                mid
            }
            ArrowHead::Bar => {
                let inset = dir * (width * 0.5);
                shapes.push(HeadShape::Stroke(vec![tip + n * hw * 0.8 - inset, tip - n * hw * 0.8 - inset]));
                tip - inset
            }
        }
    };
    let dir = (b - a) / len;
    let shaft_end = end(b, dir, &mut shapes);
    let shaft_start = if both { end(a, -dir, &mut shapes) } else { a };
    ((shaft_start, shaft_end), shapes)
}

/// Freehand point simplification while drawing: skip points closer than `min_dist`.
pub fn push_point(pts: &mut Vec<Pos2>, p: Pos2, min_dist: f32) {
    if pts.last().map_or(true, |l| (*l - p).length() >= min_dist) {
        pts.push(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    fn ann(kind: Kind, fill: Color32) -> Annotation {
        Annotation {
            id: 1,
            kind,
            a: pos2(10.0, 10.0),
            b: pos2(110.0, 60.0),
            style: Style { fill, width: 4.0, ..Default::default() },
        }
    }

    #[test]
    fn rect_hit_outline_vs_filled() {
        let none = |_: &Annotation| Vec2::ZERO;
        let outline = ann(Kind::Rect, Color32::TRANSPARENT);
        assert!(outline.hit(pos2(10.0, 30.0), 3.0, none));
        assert!(!outline.hit(pos2(60.0, 35.0), 3.0, none));
        let filled = ann(Kind::Rect, Color32::RED);
        assert!(filled.hit(pos2(60.0, 35.0), 3.0, none));
    }

    #[test]
    fn shapes_fill_their_rect() {
        let r = Rect::from_min_max(pos2(10.0, 20.0), pos2(110.0, 80.0));
        for k in ShapeKind::ALL {
            let (pts, _) = shape_outline(k, r);
            let mut b = Rect::NOTHING;
            for p in &pts {
                b.extend_with(*p);
            }
            assert!((b.min - r.min).length() < 0.6 && (b.max - r.max).length() < 0.6, "{k:?}: {b:?}");
        }
        let star = ann(Kind::Shape(ShapeKind::Star), Color32::RED);
        assert!(star.hit(pos2(60.0, 40.0), 1.0, |_| Vec2::ZERO));
        assert!(!star.hit(pos2(12.0, 12.0), 1.0, |_| Vec2::ZERO));
    }

    #[test]
    fn callout_pointer() {
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 50.0));
        let tip = pos2(30.0, 120.0);
        let pts = callout_outline(r, tip, 8.0);
        assert!(pts.contains(&tip));
        assert!(point_in_polygon(pos2(30.0, 60.0), &pts), "inside the pointer");
        assert!(!callout_outline(r, pos2(50.0, 25.0), 8.0).contains(&pos2(50.0, 25.0)));
    }

    #[test]
    fn segment_distance() {
        assert!((dist_to_segment(pos2(5.0, 5.0), pos2(0.0, 0.0), pos2(10.0, 0.0)) - 5.0).abs() < 1e-4);
        let a = ann(Kind::Arrow, Color32::TRANSPARENT);
        assert!(a.hit(pos2(60.0, 35.0), 3.0, |_| Vec2::ZERO));
        assert!(!a.hit(pos2(100.0, 15.0), 3.0, |_| Vec2::ZERO));
    }
}
