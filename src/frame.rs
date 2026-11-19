//! Presentation frames: the screenshot on a background (gradients, mesh, waves) with rounded
//! corners and a shadow, and/or inside a device mockup. Used for both the editor preview and
//! the exported image, so the preview is exactly what you get.
//!
//! Device mockups are vector drawings whose proportions come from devices.css
//! (github.com/picturepan2/devices.css, MIT License, (c) Yan Zhu). Drawing them instead of
//! shipping photos keeps them sharp at any resolution and the binary small. The screen sizes
//! use each device's real resolution, so the automatic choice matches real screenshots.

use crate::render::{draw_text_px, text_extent, Fonts};
use eframe::egui::{pos2, vec2, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};
use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pattern, Pixmap,
    Point, RadialGradient, Shader, SpreadMode, Transform,
};

// ---- settings ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Aspect {
    Auto,
    Square,
    R4x3,
    R3x2,
    R16x9,
    R9x16,
    R4x5,
    // Fixed output sizes for social media and slides: the canvas gets the shape, and Copy /
    // Save scale the result to exactly this many pixels.
    LinkPreview,
    InstagramPost,
    InstagramPortrait,
    Story,
    Slide,
    FullHd,
}

impl Aspect {
    /// Plain ratios.
    pub const ALL: [Aspect; 7] =
        [Aspect::Auto, Aspect::Square, Aspect::R4x3, Aspect::R3x2, Aspect::R16x9, Aspect::R9x16, Aspect::R4x5];
    /// Fixed output sizes.
    pub const SIZES: [Aspect; 6] = [
        Aspect::LinkPreview,
        Aspect::InstagramPost,
        Aspect::InstagramPortrait,
        Aspect::Story,
        Aspect::Slide,
        Aspect::FullHd,
    ];
    /// English name (translate with `i18n::t`).
    pub fn name(self) -> &'static str {
        match self {
            Aspect::Auto => "Auto",
            Aspect::Square => "1:1",
            Aspect::R4x3 => "4:3",
            Aspect::R3x2 => "3:2",
            Aspect::R16x9 => "16:9",
            Aspect::R9x16 => "9:16",
            Aspect::R4x5 => "4:5",
            Aspect::LinkPreview => "Link preview (LinkedIn, X, Facebook)",
            Aspect::InstagramPost => "Instagram post",
            Aspect::InstagramPortrait => "Instagram portrait",
            Aspect::Story => "Story / Reel",
            Aspect::Slide => "Slide / presentation",
            Aspect::FullHd => "Full HD",
        }
    }
    /// The exact output size in pixels, for the fixed sizes.
    pub fn fixed_size(self) -> Option<(u32, u32)> {
        match self {
            Aspect::LinkPreview => Some((1200, 630)),
            Aspect::InstagramPost => Some((1080, 1080)),
            Aspect::InstagramPortrait => Some((1080, 1350)),
            Aspect::Story => Some((1080, 1920)),
            Aspect::Slide => Some((1600, 900)),
            Aspect::FullHd => Some((1920, 1080)),
            _ => None,
        }
    }
    fn ratio(self) -> Option<f32> {
        if let Some((w, h)) = self.fixed_size() {
            return Some(w as f32 / h as f32);
        }
        match self {
            Aspect::Square => Some(1.0),
            Aspect::R4x3 => Some(4.0 / 3.0),
            Aspect::R3x2 => Some(1.5),
            Aspect::R16x9 => Some(16.0 / 9.0),
            Aspect::R9x16 => Some(9.0 / 16.0),
            Aspect::R4x5 => Some(0.8),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    Auto,
    Portrait,
    Landscape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fit {
    /// Cover the whole screen, cropping what does not fit.
    Fill,
    /// Show the whole screenshot, black bars where it does not fill the screen.
    Fit,
}

/// Which part stays visible when Fill crops the screenshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Finish {
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Background {
    Preset(String),
    Custom { a: [u8; 3], b: [u8; 3], angle: f32 },
    Transparent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameSettings {
    /// Whether the features are on is per screenshot, not remembered.
    #[serde(skip)]
    pub background_on: bool,
    #[serde(skip)]
    pub mockup_on: bool,
    pub background: Background,
    /// Space around the content, as a fraction of its average side.
    pub padding: f32,
    /// Corner radius of the screenshot (background mode), in screenshot pixels.
    pub radius: f32,
    /// 0..=1
    pub shadow: f32,
    pub grain: bool,
    pub aspect: Aspect,
    /// Device id; `None` = pick automatically.
    pub device: Option<String>,
    pub orientation: Orientation,
    pub fit: Fit,
    pub align: Align,
    pub finish: Finish,
    /// Address bar text (browser) or title (app window).
    pub url: String,
}

impl Default for FrameSettings {
    fn default() -> Self {
        Self {
            background_on: false,
            mockup_on: false,
            background: Background::Preset("hot".into()),
            padding: 0.10,
            radius: 14.0,
            shadow: 0.6,
            grain: false,
            aspect: Aspect::Auto,
            device: None,
            orientation: Orientation::Auto,
            fit: Fit::Fill,
            align: Align::Center,
            finish: Finish::Dark,
            url: String::new(),
        }
    }
}

impl FrameSettings {
    pub fn active(&self) -> bool {
        self.background_on || self.mockup_on
    }

    /// A saved preset that no longer exists (the earlier "volt") becomes the default one, so the
    /// swatch grid always shows a selection.
    pub fn with_known_background(mut self) -> Self {
        if let Background::Preset(id) = &self.background {
            if preset(id).is_none() {
                self.background = FrameSettings::default().background;
            }
        }
        self
    }
}

// ---- backgrounds --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum Fill {
    Solid(u32),
    /// Colors evenly spread, CSS angle in degrees (0 = to top, 90 = to right).
    Linear(&'static [u32], f32),
    /// Two-color linear gradient (custom background).
    Linear2([u32; 2], f32),
    /// Base color plus soft color blobs: (color, x, y, radius) relative to the canvas.
    Mesh(u32, &'static [(u32, f32, f32, f32)]),
    /// Vertical gradient (top, bottom) with flowing wave bands on top.
    Waves(u32, u32, &'static [u32]),
    /// Center color to edge color.
    Radial(u32, u32),
}

pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub fill: Fill,
}

pub const PRESETS: &[Preset] = &[
    // Blink's own: ink to hot pink. First, so it is also the fallback for an unknown id, which
    // covers the "volt" (ink to lime) preset that earlier versions saved.
    Preset { id: "hot", name: "Hot pink", fill: Fill::Linear(&[0x101114, 0x5c1440, 0xff2e93], 135.0) },
    Preset { id: "indigo", name: "Indigo", fill: Fill::Linear(&[0x6366f1, 0x0ea5e9], 135.0) },
    Preset { id: "sunset", name: "Sunset", fill: Fill::Linear(&[0xf97316, 0xdb2777, 0x7c3aed], 135.0) },
    Preset { id: "peach", name: "Peach", fill: Fill::Linear(&[0xfbc2eb, 0xa6c1ee], 120.0) },
    Preset { id: "lime", name: "Lime", fill: Fill::Linear(&[0xd4fc79, 0x96e6a1], 135.0) },
    Preset { id: "ocean", name: "Ocean", fill: Fill::Linear(&[0x2193b0, 0x6dd5ed], 160.0) },
    Preset { id: "midnight", name: "Midnight", fill: Fill::Linear(&[0x0f172a, 0x1e293b, 0x334155], 160.0) },
    Preset {
        id: "aurora",
        name: "Aurora",
        fill: Fill::Mesh(
            0x0b1026,
            &[(0x22c55e, 0.15, 0.25, 0.55), (0x06b6d4, 0.85, 0.15, 0.5), (0x8b5cf6, 0.6, 0.95, 0.65)],
        ),
    },
    Preset {
        id: "candy",
        name: "Candy",
        fill: Fill::Mesh(0xfdf2f8, &[(0xf472b6, 0.1, 0.1, 0.6), (0xa78bfa, 0.9, 0.3, 0.55), (0xfdba74, 0.4, 1.0, 0.6)]),
    },
    Preset {
        id: "lagoon",
        name: "Lagoon",
        fill: Fill::Mesh(0xe0f2fe, &[(0x38bdf8, 0.0, 0.0, 0.6), (0x34d399, 1.0, 0.6, 0.6), (0x818cf8, 0.3, 1.0, 0.55)]),
    },
    Preset {
        id: "ember",
        name: "Ember",
        fill: Fill::Mesh(
            0x1c0a00,
            &[(0xea580c, 0.2, 0.8, 0.6), (0xdc2626, 0.85, 0.2, 0.55), (0xfacc15, 0.75, 0.95, 0.45)],
        ),
    },
    Preset {
        id: "waves-blue",
        name: "Blue waves",
        fill: Fill::Waves(0x1e3a8a, 0x0ea5e9, &[0x3b82f6, 0x60a5fa, 0x93c5fd]),
    },
    Preset {
        id: "waves-dusk",
        name: "Dusk waves",
        fill: Fill::Waves(0x312e81, 0xdb2777, &[0x7c3aed, 0xc026d3, 0xf472b6]),
    },
    Preset { id: "spotlight", name: "Spotlight", fill: Fill::Radial(0x3f3f46, 0x09090b) },
    Preset { id: "white", name: "White", fill: Fill::Solid(0xffffff) },
    Preset { id: "light", name: "Light gray", fill: Fill::Solid(0xe5e7eb) },
    Preset { id: "charcoal", name: "Charcoal", fill: Fill::Solid(0x18181b) },
];

fn hex(c: u32, a: u8) -> Color {
    Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, a)
}

fn rgb3(c: [u8; 3]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

fn shader_paint(shader: Shader<'_>) -> Paint<'_> {
    Paint { shader, anti_alias: true, ..Default::default() }
}

fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn linear_shader(colors: &[u32], angle: f32, w: f32, h: f32) -> Option<Shader<'static>> {
    let a = angle.to_radians();
    let d = vec2(a.sin(), -a.cos());
    let half = (w / 2.0 * a.sin()).abs() + (h / 2.0 * a.cos()).abs();
    let c = pos2(w / 2.0, h / 2.0);
    let (p0, p1) = (c - d * half, c + d * half);
    let n = colors.len().max(2) - 1;
    let stops = colors.iter().enumerate().map(|(i, &c)| GradientStop::new(i as f32 / n as f32, hex(c, 255))).collect();
    LinearGradient::new(
        Point::from_xy(p0.x, p0.y),
        Point::from_xy(p1.x, p1.y),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
}

/// For each of `dst` output pixels: the two source pixels to blend and the weight of the second
/// (0..=256), for a source scaled up by `scale` and shifted by `offset` output pixels.
fn bilinear_taps(dst: u32, src: u32, scale: f32, offset: f32) -> Vec<(usize, usize, u32)> {
    let last = src.saturating_sub(1) as f32;
    (0..dst)
        .map(|i| {
            let u = ((i as f32 + 0.5 - offset) / scale - 0.5).clamp(0.0, last);
            let i0 = u.floor();
            let i1 = (i0 + 1.0).min(last);
            (i0 as usize, i1 as usize, ((u - i0) * 256.0).round() as u32)
        })
        .collect()
}

/// Paints a background fill over the whole pixmap (replacing what was there).
pub fn paint_fill(pm: &mut Pixmap, fill: &Fill) {
    // Gradients are smooth, so a quarter-size render scaled up looks the same and is ~16x
    // less work (large radial gradients are slow in tiny-skia).
    let smooth = matches!(fill, Fill::Linear(..) | Fill::Linear2(..) | Fill::Mesh(..) | Fill::Radial(..));
    let (w, h) = (pm.width(), pm.height());
    if smooth && w.min(h) >= 256 {
        let f = 4.0;
        let (sw, sh) = ((w as f32 / f).ceil() as u32, (h as f32 / f).ceil() as u32);
        if let Some(mut small) = Pixmap::new(sw, sh) {
            paint_fill_direct(&mut small, fill);
            let (sx, sy) = (w as f32 / sw as f32, h as f32 / sh as f32);
            let (xt, yt) = (bilinear_taps(w, sw, sx, 0.0), bilinear_taps(h, sh, sy, 0.0));
            let (src, row) = (small.data(), w as usize * 4);
            for (y, d) in pm.data_mut().chunks_exact_mut(row).enumerate() {
                let (y0, y1, fy) = yt[y];
                let (r0, r1) = (&src[y0 * sw as usize * 4..], &src[y1 * sw as usize * 4..]);
                for (o, &(x0, x1, fx)) in d.chunks_exact_mut(4).zip(&xt) {
                    for c in 0..4 {
                        let top = r0[x0 * 4 + c] as u32 * (256 - fx) + r0[x1 * 4 + c] as u32 * fx;
                        let bot = r1[x0 * 4 + c] as u32 * (256 - fx) + r1[x1 * 4 + c] as u32 * fx;
                        o[c] = ((top * (256 - fy) + bot * fy + (1 << 15)) >> 16) as u8;
                    }
                }
            }
            return;
        }
    }
    paint_fill_direct(pm, fill);
}

fn paint_fill_direct(pm: &mut Pixmap, fill: &Fill) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let Some(full) = tiny_skia::Rect::from_xywh(0.0, 0.0, w, h) else { return };
    let t = Transform::identity();
    match *fill {
        Fill::Solid(c) => pm.fill(hex(c, 255)),
        Fill::Linear(colors, angle) => {
            if let Some(s) = linear_shader(colors, angle, w, h) {
                pm.fill_rect(full, &shader_paint(s), t, None);
            }
        }
        Fill::Linear2(colors, angle) => {
            if let Some(s) = linear_shader(&colors, angle, w, h) {
                pm.fill_rect(full, &shader_paint(s), t, None);
            }
        }
        Fill::Mesh(base, blobs) => {
            pm.fill(hex(base, 255));
            let m = w.max(h);
            for &(c, x, y, r) in blobs {
                let center = Point::from_xy(x * w, y * h);
                let stops = vec![
                    GradientStop::new(0.0, hex(c, 235)),
                    GradientStop::new(0.45, hex(c, 140)),
                    GradientStop::new(1.0, hex(c, 0)),
                ];
                if let Some(s) = RadialGradient::new(center, center, r * m, stops, SpreadMode::Pad, t) {
                    pm.fill_rect(full, &shader_paint(s), t, None);
                }
            }
        }
        Fill::Waves(top, bottom, waves) => {
            if let Some(s) = linear_shader(&[top, bottom], 180.0, w, h) {
                pm.fill_rect(full, &shader_paint(s), t, None);
            }
            for (i, &c) in waves.iter().enumerate() {
                let fi = i as f32;
                let y0 = h * (0.2 + 0.22 * fi);
                let amp = h * 0.07;
                let mut pb = PathBuilder::new();
                pb.move_to(0.0, h);
                let n = 96;
                for k in 0..=n {
                    let x = w * k as f32 / n as f32;
                    let u = k as f32 / n as f32 * std::f32::consts::TAU;
                    let y = y0 + amp * (u * (0.8 + 0.3 * fi) + fi * 1.9).sin() + amp * 0.4 * (u * 2.1 + fi).sin();
                    pb.line_to(x, y);
                }
                pb.line_to(w, h);
                pb.close();
                if let Some(p) = pb.finish() {
                    pm.fill_path(&p, &solid(hex(c, 120 + 30 * i as u8)), FillRule::Winding, t, None);
                }
            }
        }
        Fill::Radial(inner, outer) => {
            pm.fill(hex(outer, 255));
            let c = Point::from_xy(w * 0.5, h * 0.42);
            let stops = vec![GradientStop::new(0.0, hex(inner, 255)), GradientStop::new(1.0, hex(outer, 255))];
            if let Some(s) = RadialGradient::new(c, c, w.max(h) * 0.75, stops, SpreadMode::Pad, t) {
                pm.fill_rect(full, &shader_paint(s), t, None);
            }
        }
    }
}

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

fn background_fill(bg: &Background) -> Option<Fill> {
    match bg {
        Background::Preset(id) => Some(preset(id).unwrap_or(&PRESETS[0]).fill),
        Background::Custom { a, b, angle } => Some(Fill::Linear2([rgb3(*a), rgb3(*b)], *angle)),
        Background::Transparent => None,
    }
}

/// Light film grain (deterministic), for a less "flat digital" look.
fn add_grain(pm: &mut Pixmap) {
    let mut seed: u32 = 0x9E37_79B9;
    for p in pm.data_mut().chunks_exact_mut(4) {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let n = (seed % 15) as i32 - 7;
        let a = p[3] as i32;
        for c in &mut p[..3] {
            *c = (*c as i32 + n).clamp(0, a) as u8;
        }
    }
}

// ---- devices ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Phone,
    Tablet,
    Laptop,
    Desktop,
    Window,
}

impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Group::Phone => "Phones",
            Group::Tablet => "Tablets",
            Group::Laptop => "Laptops",
            Group::Desktop => "Desktops",
            Group::Window => "Windows",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Model {
    IPhone,
    Pixel,
    IPad,
    SurfacePro,
    MacBook { notch: bool },
    WinLaptop,
    IMac,
    Monitor,
    Browser,
    AppWindow,
}

pub struct Device {
    pub id: &'static str,
    pub name: &'static str,
    pub group: Group,
    /// Native screen resolution in the device's natural orientation; (0, 0) = adapts to the screenshot.
    pub res: (u32, u32),
    pub rotatable: bool,
    model: Model,
}

impl Device {
    pub fn flexible(&self) -> bool {
        self.res.0 == 0
    }
    pub fn label(&self) -> String {
        if self.flexible() {
            self.display_name().to_string()
        } else {
            format!("{}  ({} x {})", self.display_name(), self.res.0, self.res.1)
        }
    }
    /// The name in the interface language (product names stay as they are).
    pub fn display_name(&self) -> &'static str {
        crate::i18n::t(self.name)
    }
}

const fn dev(id: &'static str, name: &'static str, group: Group, res: (u32, u32), rot: bool, model: Model) -> Device {
    Device { id, name, group, res, rotatable: rot, model }
}

pub const DEVICES: &[Device] = &[
    dev("iphone-15-pro", "iPhone 15 Pro", Group::Phone, (1179, 2556), true, Model::IPhone),
    dev("iphone-15-pro-max", "iPhone 15 Pro Max", Group::Phone, (1290, 2796), true, Model::IPhone),
    dev("pixel-8", "Pixel 8", Group::Phone, (1080, 2400), true, Model::Pixel),
    dev("galaxy-s24", "Galaxy S24", Group::Phone, (1080, 2340), true, Model::Pixel),
    dev("ipad-pro-11", "iPad Pro 11\u{2033}", Group::Tablet, (1668, 2388), true, Model::IPad),
    dev("ipad-pro-13", "iPad Pro 12.9\u{2033}", Group::Tablet, (2048, 2732), true, Model::IPad),
    dev("android-tablet", "Android tablet 16:10", Group::Tablet, (1600, 2560), true, Model::IPad),
    dev("surface-pro", "Surface Pro 3:2", Group::Tablet, (2880, 1920), true, Model::SurfacePro),
    dev("macbook-pro-14", "MacBook Pro 14\u{2033}", Group::Laptop, (3024, 1964), false, Model::MacBook { notch: true }),
    dev("macbook-air", "MacBook Air 16:10", Group::Laptop, (2560, 1600), false, Model::MacBook { notch: false }),
    dev("laptop-16-9", "Windows laptop 16:9", Group::Laptop, (1920, 1080), false, Model::WinLaptop),
    dev("laptop-3-2", "Surface Laptop 3:2", Group::Laptop, (2256, 1504), false, Model::WinLaptop),
    dev("imac-24", "iMac 24\u{2033}", Group::Desktop, (4480, 2520), false, Model::IMac),
    dev("monitor-16-9", "Monitor 16:9", Group::Desktop, (2560, 1440), false, Model::Monitor),
    dev("monitor-16-10", "Monitor 16:10", Group::Desktop, (1920, 1200), false, Model::Monitor),
    dev("ultrawide", "Ultrawide 21:9", Group::Desktop, (3440, 1440), false, Model::Monitor),
    dev("browser", "Browser window", Group::Window, (0, 0), false, Model::Browser),
    dev("window", "App window", Group::Window, (0, 0), false, Model::AppWindow),
];

pub fn device(id: &str) -> Option<&'static Device> {
    DEVICES.iter().find(|d| d.id == id)
}

/// How far apart two aspect ratios are (0 = identical).
fn aspect_distance(a: f32, b: f32) -> f32 {
    (a / b).ln().abs()
}

/// Largest aspect mismatch (about 15%) the automatic choice accepts before falling back to
/// a window frame, which adapts to any size.
const AUTO_TOLERANCE: f32 = 0.14;

/// Best matching device and whether it is used rotated (away from its natural orientation).
pub fn auto_device(iw: f32, ih: f32) -> (&'static Device, bool) {
    let ar = iw / ih;
    let mut best: Option<(&'static Device, bool, f32, f32)> = None;
    for d in DEVICES.iter().filter(|d| !d.flexible()) {
        for rotated in [false, true] {
            if rotated && !d.rotatable {
                continue;
            }
            let (w, h) = if rotated { (d.res.1, d.res.0) } else { d.res };
            let score = aspect_distance(ar, w as f32 / h as f32);
            let size = (iw / w as f32).ln().abs();
            let better = match best {
                None => true,
                Some((_, _, bs, bsize)) => score < bs - 0.004 || (score < bs + 0.004 && size < bsize),
            };
            if better {
                best = Some((d, rotated, score, size));
            }
        }
    }
    match best {
        Some((d, rot, score, _)) if score <= AUTO_TOLERANCE => (d, rot),
        _ => (device("browser").unwrap(), false),
    }
}

/// The device a setting resolves to for a screenshot, with its rotation.
pub fn resolve_device(s: &FrameSettings, iw: f32, ih: f32) -> (&'static Device, bool) {
    let d = match s.device.as_deref().and_then(device) {
        Some(d) => d,
        None => {
            let (d, rot) = auto_device(iw, ih);
            if s.orientation == Orientation::Auto || !d.rotatable {
                return (d, rot);
            }
            d
        }
    };
    if !d.rotatable || d.flexible() {
        return (d, false);
    }
    let natural_portrait = d.res.1 > d.res.0;
    let want_portrait = match s.orientation {
        Orientation::Portrait => true,
        Orientation::Landscape => false,
        Orientation::Auto => {
            let ar = iw / ih;
            let (w, h) = (d.res.0 as f32, d.res.1 as f32);
            let natural = aspect_distance(ar, w / h) <= aspect_distance(ar, h / w);
            return (d, !natural);
        }
    };
    (d, want_portrait != natural_portrait)
}

/// Device geometry in local coordinates (natural orientation), for a given screen size.
#[derive(Clone, Copy, Debug)]
pub struct Geom {
    /// Bounding box of everything that is drawn (buttons and stands included).
    pub extent: Rect,
    pub body: Rect,
    pub screen: Rect,
    /// tl, tr, br, bl
    pub screen_radii: [f32; 4],
    /// devices.css pixel in local units.
    u: f32,
}

fn geometry(model: Model, sw: f32, sh: f32) -> Geom {
    let r4 = |r: f32| [r; 4];
    match model {
        Model::IPhone => {
            let u = sw / 390.0;
            let b = 19.0 * u;
            let body = Rect::from_min_size(Pos2::ZERO, vec2(sw + 2.0 * b, sh + 2.0 * b));
            let screen = Rect::from_min_size(pos2(b, b), vec2(sw, sh));
            Geom { extent: body.expand2(vec2(2.5 * u, 0.0)), body, screen, screen_radii: r4(49.0 * u), u }
        }
        Model::Pixel => {
            let u = sw / 376.0;
            let body = Rect::from_min_size(Pos2::ZERO, vec2(sw + 24.0 * u, sh + 46.0 * u));
            let screen = Rect::from_min_size(pos2(12.0 * u, 20.0 * u), vec2(sw, sh));
            Geom { extent: body.expand2(vec2(3.0 * u, 0.0)), body, screen, screen_radii: r4(34.0 * u), u }
        }
        Model::IPad => {
            let u = sw / 506.0;
            let b = 27.0 * u;
            let body = Rect::from_min_size(Pos2::ZERO, vec2(sw + 2.0 * b, sh + 2.0 * b));
            let screen = Rect::from_min_size(pos2(b, b), vec2(sw, sh));
            Geom { extent: body.expand(2.5 * u), body, screen, screen_radii: r4(16.0 * u), u }
        }
        Model::SurfacePro => {
            let u = sw / 513.0;
            let body = Rect::from_min_size(Pos2::ZERO, vec2(sw + 48.0 * u, sh + 52.0 * u));
            let screen = Rect::from_min_size(pos2(24.0 * u, 26.0 * u), vec2(sw, sh));
            Geom { extent: body.expand(2.5 * u), body, screen, screen_radii: r4(2.0 * u), u }
        }
        Model::MacBook { .. } => {
            let u = sw / 600.0;
            let lid = Rect::from_min_size(pos2(61.0 * u, 0.0), vec2(sw + 18.0 * u, sh + 32.0 * u));
            let screen = Rect::from_min_size(lid.min + vec2(9.0 * u, 9.0 * u), vec2(sw, sh));
            let extent = Rect::from_min_max(Pos2::ZERO, pos2(740.0 * u, lid.max.y + 16.0 * u));
            let tr = 10.0 * u;
            Geom { extent, body: lid, screen, screen_radii: [tr, tr, 0.0, 0.0], u }
        }
        Model::WinLaptop => {
            let u = sw / 540.0;
            let lid = Rect::from_min_size(pos2(72.0 * u, 0.0), vec2(sw + 44.0 * u, sh + 48.0 * u));
            let screen = Rect::from_min_size(lid.min + vec2(22.0 * u, 24.0 * u), vec2(sw, sh));
            let extent = Rect::from_min_max(Pos2::ZERO, pos2(728.0 * u, lid.max.y + 18.0 * u));
            Geom { extent, body: lid, screen, screen_radii: r4(2.0 * u), u }
        }
        Model::IMac => {
            let u = sw / 608.0;
            let body = Rect::from_min_size(Pos2::ZERO, vec2(sw + 32.0 * u, sh + 96.0 * u));
            let screen = Rect::from_min_size(pos2(16.0 * u, 16.0 * u), vec2(sw, sh));
            let extent = Rect::from_min_max(Pos2::ZERO, pos2(body.max.x, body.max.y + 98.0 * u));
            Geom { extent, body, screen, screen_radii: r4(2.0 * u), u }
        }
        Model::Monitor => {
            let u = sw / 624.0;
            let body = Rect::from_min_size(Pos2::ZERO, vec2(sw + 16.0 * u, sh + 16.0 * u));
            let screen = Rect::from_min_size(pos2(8.0 * u, 8.0 * u), vec2(sw, sh));
            let extent = Rect::from_min_max(Pos2::ZERO, pos2(body.max.x, body.max.y + 108.0 * u));
            Geom { extent, body, screen, screen_radii: r4(2.0 * u), u }
        }
        Model::Browser | Model::AppWindow => {
            let bar = window_bar_height(model, sw);
            let r = bar * 0.28;
            let body = Rect::from_min_size(Pos2::ZERO, vec2(sw, sh + bar));
            let screen = Rect::from_min_size(pos2(0.0, bar), vec2(sw, sh));
            Geom { extent: body, body, screen, screen_radii: [0.0, 0.0, r, r], u: 1.0 }
        }
    }
}

fn window_bar_height(model: Model, sw: f32) -> f32 {
    match model {
        Model::Browser => (sw * 0.036).clamp(30.0, 150.0),
        _ => (sw * 0.026).clamp(24.0, 110.0),
    }
}

/// Rounded rectangle with per-corner radii (tl, tr, br, bl).
pub fn rrect(r: Rect, radii: [f32; 4]) -> Option<Path> {
    let max = (r.width().min(r.height()) / 2.0).max(0.0);
    let [tl, tr, br, bl] = radii.map(|x| x.clamp(0.0, max));
    const K: f32 = 0.552_284_8;
    let (l, t, rr, b) = (r.min.x, r.min.y, r.max.x, r.max.y);
    let mut pb = PathBuilder::new();
    pb.move_to(l + tl, t);
    pb.line_to(rr - tr, t);
    if tr > 0.0 {
        pb.cubic_to(rr - tr + tr * K, t, rr, t + tr - tr * K, rr, t + tr);
    }
    pb.line_to(rr, b - br);
    if br > 0.0 {
        pb.cubic_to(rr, b - br + br * K, rr - br + br * K, b, rr - br, b);
    }
    pb.line_to(l + bl, b);
    if bl > 0.0 {
        pb.cubic_to(l + bl - bl * K, b, l, b - bl + bl * K, l, b - bl);
    }
    pb.line_to(l, t + tl);
    if tl > 0.0 {
        pb.cubic_to(l, t + tl - tl * K, l + tl - tl * K, t, l + tl, t);
    }
    pb.close();
    pb.finish()
}

fn rr(r: Rect, radius: f32) -> Option<Path> {
    rrect(r, [radius; 4])
}

fn circle(c: Pos2, r: f32) -> Option<Path> {
    PathBuilder::from_circle(c.x, c.y, r.max(0.1))
}

fn vgradient(r: Rect, stops: &[(f32, u32)]) -> Option<Shader<'static>> {
    LinearGradient::new(
        Point::from_xy(r.min.x, r.min.y),
        Point::from_xy(r.min.x, r.max.y),
        stops.iter().map(|&(p, c)| GradientStop::new(p, hex(c, 255))).collect(),
        SpreadMode::Pad,
        Transform::identity(),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    /// Body, bezels, buttons, stand: drawn below the screenshot.
    Under,
    /// Cut-outs (dynamic island, notch, camera hole): drawn on top of it.
    Over,
    /// Everything that casts a shadow, in black.
    Silhouette,
}

struct Pen<'a> {
    pm: &'a mut Pixmap,
    t: Transform,
    silhouette: bool,
}

impl Pen<'_> {
    fn fill(&mut self, path: Option<Path>, c: u32) {
        self.fill_alpha(path, c, 255);
    }
    fn fill_alpha(&mut self, path: Option<Path>, c: u32, a: u8) {
        let Some(path) = path else { return };
        let paint = if self.silhouette { solid(hex(0, 255)) } else { solid(hex(c, a)) };
        self.pm.fill_path(&path, &paint, FillRule::Winding, self.t, None);
    }
    fn fill_shader(&mut self, path: Option<Path>, shader: Option<Shader<'static>>, fallback: u32) {
        match (self.silhouette, shader) {
            (false, Some(s)) => {
                if let Some(path) = path {
                    self.pm.fill_path(&path, &shader_paint(s), FillRule::Winding, self.t, None);
                }
            }
            _ => self.fill(path, fallback),
        }
    }
    fn stroke(&mut self, path: Option<Path>, c: u32, a: u8, width: f32) {
        if self.silhouette {
            return;
        }
        let Some(path) = path else { return };
        let s = tiny_skia::Stroke { width, ..Default::default() };
        self.pm.stroke_path(&path, &solid(hex(c, a)), &s, self.t, None);
    }
}

/// Colors per finish: (frame edge, frame ring, highlight).
fn phone_colors(f: Finish) -> (u32, u32, u32) {
    match f {
        Finish::Dark => (0x1b1a1f, 0x3a3940, 0x8e8a96),
        Finish::Light => (0xa9a6a0, 0xd9d6cf, 0xffffff),
    }
}

fn lens(pen: &mut Pen, c: Pos2, r: f32) {
    pen.fill(circle(c, r), 0x111827);
    pen.fill(circle(c, r * 0.6), 0x1e2a4a);
    pen.fill_alpha(circle(c + vec2(-r * 0.3, -r * 0.3), r * 0.25), 0x6074bf, 200);
}

#[allow(clippy::too_many_arguments)]
fn draw_device(
    pm: &mut Pixmap,
    d: &Device,
    g: &Geom,
    t: Transform,
    finish: Finish,
    layer: Layer,
    fonts: &Fonts,
    title: &str,
) {
    let silhouette = layer == Layer::Silhouette;
    let under = layer != Layer::Over;
    let mut pen = Pen { pm, t, silhouette };
    let u = g.u;
    let b = g.body;
    let sc = g.screen;
    let cx = b.center().x;
    match d.model {
        Model::IPhone => {
            let (edge, ring, hi) = phone_colors(finish);
            if under {
                let btn =
                    |x: f32, y: f32, h: f32| rr(Rect::from_min_size(pos2(x, y * u), vec2(3.0 * u, h * u)), 1.5 * u);
                for (y, h) in [(115.0, 32.0), (175.0, 62.0), (255.0, 62.0)] {
                    pen.fill(btn(b.min.x - 2.0 * u, y, h), edge);
                }
                pen.fill(btn(b.max.x - 1.0 * u, 200.0, 100.0), edge);
                pen.fill(rr(b, 68.0 * u), edge);
                pen.fill(rr(b.shrink(1.0 * u), 67.0 * u), ring);
                pen.stroke(rr(b.shrink(4.0 * u), 64.0 * u), hi, 110, 1.5 * u);
                pen.fill(rr(b.shrink(7.0 * u), 61.0 * u), 0x010101);
            } else {
                let island = Rect::from_center_size(pos2(cx, sc.min.y + 27.5 * u), vec2(120.0 * u, 35.0 * u));
                pen.fill(rr(island, 17.5 * u), 0x010101);
                lens(&mut pen, pos2(cx + 40.5 * u, island.center().y), 4.5 * u);
            }
        }
        Model::Pixel => {
            let (edge, _, hi) = phone_colors(finish);
            if under {
                pen.fill(
                    rr(Rect::from_min_size(pos2(b.max.x - 1.0 * u, 194.0 * u), vec2(3.5 * u, 58.0 * u)), 1.5 * u),
                    edge,
                );
                pen.fill(
                    rr(Rect::from_min_size(pos2(b.max.x - 1.0 * u, 306.0 * u), vec2(3.5 * u, 102.0 * u)), 1.5 * u),
                    edge,
                );
                pen.fill(rr(b, 46.0 * u), edge);
                pen.stroke(rr(b.shrink(1.5 * u), 44.5 * u), hi, 90, 1.2 * u);
                pen.fill(rr(b.shrink(3.0 * u), 43.0 * u), 0x0b0b0c);
            } else {
                lens(&mut pen, pos2(cx, sc.min.y + 19.0 * u), 11.0 * u);
            }
        }
        Model::IPad | Model::SurfacePro => {
            let surface = matches!(d.model, Model::SurfacePro);
            let edge = match finish {
                Finish::Dark => 0x3a3b3d,
                Finish::Light => 0xc4c6c8,
            };
            if under {
                if surface {
                    pen.fill(Some(PathBuilder::from_rect(rect_xywh(48.0 * u, -2.0 * u, 26.0 * u, 2.5 * u))), edge);
                    pen.fill(Some(PathBuilder::from_rect(rect_xywh(94.0 * u, -2.0 * u, 48.0 * u, 2.5 * u))), edge);
                } else {
                    pen.fill(
                        rr(Rect::from_min_size(pos2(b.max.x - 76.0 * u, -2.0 * u), vec2(36.0 * u, 3.0 * u)), 1.0 * u),
                        edge,
                    );
                    for y in [63.0, 100.0] {
                        pen.fill(
                            rr(Rect::from_min_size(pos2(b.max.x - 1.0 * u, y * u), vec2(3.0 * u, 32.0 * u)), 1.0 * u),
                            edge,
                        );
                    }
                }
                let radius = if surface { 10.0 } else { 36.0 } * u;
                pen.fill(rr(b, radius), edge);
                pen.fill(rr(b.shrink(2.0 * u), radius - 2.0 * u), 0x0d0d0d);
                // Front camera in the top bezel.
                lens(&mut pen, pos2(cx, (sc.min.y - b.min.y) * 0.5), 3.0 * u);
            }
        }
        Model::MacBook { notch } => {
            let (edge, base0, base1, base2, dent) = match finish {
                Finish::Light => (0xc8cacb, 0xe2e3e4, 0xc8cacb, 0x8f9296, 0xd0d2d4),
                Finish::Dark => (0x55565a, 0x4a4b4f, 0x35363a, 0x1d1e20, 0x3c3d41),
            };
            if under {
                pen.fill(rr(b, 20.0 * u), edge);
                pen.fill(rr(b.shrink(2.0 * u), 18.0 * u), 0x0d0d0d);
                let chin =
                    Rect::from_min_max(pos2(b.min.x + 2.0 * u, b.max.y - 26.0 * u), b.max - vec2(2.0 * u, 2.0 * u));
                pen.fill_shader(
                    rrect(chin, [0.0, 0.0, 18.0 * u, 18.0 * u]),
                    vgradient(chin, &[(0.0, 0x272727), (1.0, 0x0d0d0d)]),
                    0x1a1a1a,
                );
                let base = Rect::from_min_size(pos2(0.0, b.max.y - 10.0 * u), vec2(740.0 * u, 24.0 * u));
                // Feet.
                for dx in [-300.0, 300.0] {
                    pen.fill(
                        rr(
                            Rect::from_center_size(
                                pos2(base.center().x + dx * u, base.max.y + 1.0 * u),
                                vec2(40.0 * u, 3.0 * u),
                            ),
                            1.5 * u,
                        ),
                        0x272727,
                    );
                }
                pen.fill_shader(
                    rrect(base, [2.0 * u, 2.0 * u, 12.0 * u, 12.0 * u]),
                    vgradient(base, &[(0.0, base0), (0.55, base1), (1.0, base2)]),
                    base1,
                );
                let indent =
                    Rect::from_min_size(pos2(base.center().x - 60.0 * u, base.min.y), vec2(120.0 * u, 10.0 * u));
                pen.fill(rrect(indent, [0.0, 0.0, 10.0 * u, 10.0 * u]), dent);
            } else if notch {
                let n = Rect::from_min_size(pos2(cx - 32.0 * u, sc.min.y - 1.0), vec2(64.0 * u, 12.0 * u + 1.0));
                pen.fill(rrect(n, [0.0, 0.0, 4.0 * u, 4.0 * u]), 0x0d0d0d);
                lens(&mut pen, pos2(cx, sc.min.y + 5.0 * u), 2.2 * u);
            }
        }
        Model::WinLaptop => {
            let (edge, base0, base1, dent) = match finish {
                Finish::Light => (0xc8c8c8, 0xeeeeee, 0xc4c4c4, 0xdedede),
                Finish::Dark => (0x46474b, 0x45464a, 0x2b2c2f, 0x37383c),
            };
            if under {
                pen.fill(rr(b, 12.0 * u), edge);
                pen.fill(rr(b.shrink(2.0 * u), 10.0 * u), 0x0d0d0d);
                lens(&mut pen, pos2(cx, 12.0 * u), 2.5 * u);
                let base = Rect::from_min_size(pos2(0.0, b.max.y + 2.0 * u), vec2(728.0 * u, 16.0 * u));
                pen.fill_shader(
                    rrect(base, [2.0 * u, 2.0 * u, 8.0 * u, 8.0 * u]),
                    vgradient(base, &[(0.0, base0), (1.0, base1)]),
                    base1,
                );
                let indent =
                    Rect::from_min_size(pos2(base.center().x - 125.0 * u, base.min.y), vec2(250.0 * u, 7.0 * u));
                pen.fill(rrect(indent, [0.0, 0.0, 6.0 * u, 6.0 * u]), dent);
            }
        }
        Model::IMac => {
            let (bezel, chin, s0, s1, s2) = match finish {
                Finish::Light => (0xedeef0, 0xd4d5d7, 0x9fa1a6, 0xc7c8cb, 0x6b6e74),
                Finish::Dark => (0x1f1f22, 0x3a3a3e, 0x2c2c30, 0x4a4a50, 0x18181b),
            };
            if under {
                let neck = Rect::from_min_size(pos2(cx - 76.0 * u, b.max.y), vec2(152.0 * u, 92.0 * u));
                pen.fill_shader(
                    Some(PathBuilder::from_rect(to_sk(neck))),
                    vgradient(neck, &[(0.0, s0), (0.4, s1), (0.85, s1), (0.9, 0xffffff), (1.0, s2)]),
                    s1,
                );
                let foot = Rect::from_min_size(pos2(cx - 80.0 * u, neck.max.y), vec2(160.0 * u, 6.0 * u));
                pen.fill(rr(foot, 2.0 * u), chin);
                pen.fill(rr(b, 18.0 * u), bezel);
                let c = Rect::from_min_max(pos2(b.min.x, b.max.y - 64.0 * u), b.max);
                pen.fill(rrect(c, [0.0, 0.0, 18.0 * u, 18.0 * u]), chin);
                pen.fill(Some(PathBuilder::from_rect(to_sk(sc.expand(2.0 * u)))), 0x121212);
                pen.fill(circle(pos2(cx, 9.0 * u), 3.0 * u), 0x050505);
            }
        }
        Model::Monitor => {
            let (s0, s1, s2) = match finish {
                Finish::Light => (0x9fa1a6, 0xc7c8cb, 0x6b6e74),
                Finish::Dark => (0x2c2c30, 0x45454b, 0x141416),
            };
            if under {
                let neck = Rect::from_min_size(pos2(cx - 76.0 * u, b.max.y), vec2(152.0 * u, 102.0 * u));
                pen.fill_shader(
                    Some(PathBuilder::from_rect(to_sk(neck))),
                    vgradient(neck, &[(0.0, s0), (0.4, s1), (0.85, s1), (0.9, 0xffffff), (1.0, s2)]),
                    s1,
                );
                let foot = Rect::from_min_size(pos2(cx - 80.0 * u, neck.max.y), vec2(160.0 * u, 6.0 * u));
                pen.fill(rr(foot, 2.0 * u), s1);
                pen.fill(rr(b, 4.0 * u), 0x151515);
                pen.stroke(rr(b.shrink(0.5 * u), 3.5 * u), 0xd4d5d7, 60, 1.0 * u);
            }
        }
        Model::Browser | Model::AppWindow => {
            let dark = finish == Finish::Dark;
            let bar_h = sc.min.y - b.min.y;
            let r = bar_h * 0.28;
            let (bar, border, pill, text) =
                if dark { (0x2b2b30, 0x3f3f46, 0x3f3f46, 0xa1a1aa) } else { (0xececef, 0xd4d4d8, 0xffffff, 0x71717a) };
            if under {
                pen.fill(rr(b, r), bar);
                pen.stroke(rr(b, r), border, 255, 1.0_f32.max(bar_h / 60.0));
                if silhouette {
                    return;
                }
                let dot = bar_h * 0.12;
                for (i, c) in [0xff5f57, 0xfebc2e, 0x28c840].into_iter().enumerate() {
                    pen.fill(circle(pos2(bar_h * 0.45 + i as f32 * bar_h * 0.38, bar_h / 2.0), dot), c);
                }
                let browser = matches!(d.model, Model::Browser);
                let text_size = bar_h * if browser { 0.3 } else { 0.36 };
                if browser {
                    let pw = (b.width() * 0.5).min(b.width() - bar_h * 4.0).max(bar_h);
                    let p = Rect::from_center_size(pos2(cx, bar_h / 2.0), vec2(pw, bar_h * 0.56));
                    pen.fill(rr(p, p.height() / 2.0), pill);
                }
                if !title.is_empty() {
                    let font = if browser { &fonts.regular } else { &fonts.bold };
                    let mut pts = [Point::from_xy(cx, bar_h / 2.0)];
                    t.map_points(&mut pts);
                    let k = (t.sx * t.sx + t.ky * t.ky).sqrt();
                    let size = text_size * k;
                    let ext = text_extent(font, title, size);
                    let origin = pos2(pts[0].x - ext.x / 2.0, pts[0].y - ext.y / 2.0);
                    draw_text_px(pen.pm, font, title, size, origin, egui_color(text));
                }
            } else {
                // Thin separator between the title bar and the content.
                let line = Rect::from_min_size(pos2(0.0, sc.min.y - 0.5), vec2(b.width(), 1.0_f32.max(bar_h / 60.0)));
                pen.fill(Some(PathBuilder::from_rect(to_sk(line))), border);
            }
        }
    }
}

fn egui_color(c: u32) -> eframe::egui::Color32 {
    eframe::egui::Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn rect_xywh(x: f32, y: f32, w: f32, h: f32) -> tiny_skia::Rect {
    tiny_skia::Rect::from_xywh(x, y, w.max(0.01), h.max(0.01)).unwrap()
}

fn to_sk(r: Rect) -> tiny_skia::Rect {
    rect_xywh(r.min.x, r.min.y, r.width(), r.height())
}

// ---- layout -------------------------------------------------------------------------------

/// A device placed in the output.
#[derive(Clone, Copy)]
pub struct Placed {
    pub device: &'static Device,
    geom: Geom,
    /// Device-local coordinates -> output pixels.
    tf: Transform,
}

/// Where everything goes in the output image (output pixels).
#[derive(Clone, Copy)]
pub struct Layout {
    pub size: Vec2,
    /// Where the whole screenshot lands (may extend beyond `clip` when filling a screen).
    pub shot: Rect,
    /// Visible screenshot area.
    pub clip: Rect,
    pub clip_radii: [f32; 4],
    /// Black behind the screenshot (Fit inside a device screen).
    pub letterbox: bool,
    pub device: Option<Placed>,
    /// Shadow size (output pixels).
    shadow_blur: f32,
}

impl Layout {
    /// Output pixels per screenshot pixel.
    pub fn scale(&self, shot_w: f32) -> f32 {
        self.shot.width() / shot_w.max(1.0)
    }
}

/// Largest output side; bigger frames are scaled down (keeps memory and file size sane).
const MAX_SIDE: f32 = 12000.0;

fn map_rect(t: Transform, r: Rect) -> Rect {
    let mut pts = [
        Point::from_xy(r.min.x, r.min.y),
        Point::from_xy(r.max.x, r.min.y),
        Point::from_xy(r.max.x, r.max.y),
        Point::from_xy(r.min.x, r.max.y),
    ];
    t.map_points(&mut pts);
    let mut out = Rect::NOTHING;
    for p in pts {
        out.extend_with(pos2(p.x, p.y));
    }
    out
}

/// Computes the frame layout for a screenshot of `iw` x `ih` pixels, or `None` when no frame is on.
pub fn layout(s: &FrameSettings, iw: f32, ih: f32) -> Option<Layout> {
    if !s.active() || iw < 1.0 || ih < 1.0 {
        return None;
    }
    // Content (device or rounded screenshot) in content coordinates.
    let (content, shot, clip, radii, letterbox, device) = if s.mockup_on {
        let (d, rotated) = resolve_device(s, iw, ih);
        // Screen size in output pixels, chosen so the screenshot keeps its native resolution.
        let (sw, sh, k) = if d.flexible() {
            (iw, ih, 1.0)
        } else {
            let (rw, rh) = if rotated { (d.res.1, d.res.0) } else { d.res };
            let ar = rw as f32 / rh as f32;
            let sh = match s.fit {
                Fit::Fill => (iw / ar).min(ih),
                Fit::Fit => (iw / ar).max(ih),
            };
            let sw = sh * ar;
            let k = match s.fit {
                Fit::Fill => (sw / iw).max(sh / ih),
                Fit::Fit => (sw / iw).min(sh / ih),
            };
            (sw, sh, k)
        };
        let (lw, lh) = if rotated { (sh, sw) } else { (sw, sh) };
        let g = geometry(d.model, lw, lh);
        let e = g.extent;
        let tf = if rotated {
            // Natural top goes to the left (like a phone turned counter-clockwise).
            Transform::from_row(0.0, -1.0, 1.0, 0.0, -e.min.y, e.max.x)
        } else {
            Transform::from_translate(-e.min.x, -e.min.y)
        };
        let content = if rotated { vec2(e.height(), e.width()) } else { e.size() };
        let screen = map_rect(tf, g.screen);
        let p = g.screen_radii;
        let radii = if rotated { [p[1], p[2], p[3], p[0]] } else { p };
        let size = vec2(iw, ih) * k;
        let a = match s.align {
            Align::Start => 0.0,
            Align::Center => 0.5,
            Align::End => 1.0,
        };
        let shot = Rect::from_min_size(screen.min + (screen.size() - size) * a, size);
        let placed = Placed { device: d, geom: g, tf };
        (content, shot, screen, radii, s.fit == Fit::Fit && !d.flexible(), Some(placed))
    } else {
        let r = Rect::from_min_size(Pos2::ZERO, vec2(iw, ih));
        (r.size(), r, r, [s.radius.max(0.0); 4], false, None)
    };

    let avg = (content.x + content.y) / 2.0;
    let pad = if s.background_on {
        s.padding.clamp(0.0, 1.0) * avg
    } else if s.shadow > 0.0 {
        0.06 * avg
    } else {
        0.0
    };
    let mut size = content + Vec2::splat(pad * 2.0);
    if let (true, Some(r)) = (s.background_on, s.aspect.ratio()) {
        if size.x / size.y < r {
            size.x = size.y * r;
        } else {
            size.y = size.x / r;
        }
    }
    let off = (size - content) / 2.0;
    let g = (MAX_SIDE / size.x.max(size.y)).min(1.0);
    let tr = |r: Rect| Rect::from_min_size(((r.min.to_vec2() + off) * g).to_pos2(), r.size() * g);
    let device = device.map(|p| Placed { tf: p.tf.post_translate(off.x, off.y).post_scale(g, g), ..p });
    let mut shot = tr(shot);
    if (shot.width() - iw).abs() < 1e-3 && (shot.height() - ih).abs() < 1e-3 {
        // Native size: put it on whole pixels, so it is copied 1:1 (sharp, and much faster).
        shot = Rect::from_min_size(shot.min.round(), shot.size());
    }
    Some(Layout {
        size: (size * g).round(),
        shot,
        clip: tr(clip),
        clip_radii: radii.map(|r| r * g),
        letterbox,
        device,
        shadow_blur: 0.035 * avg * g,
    })
}

// ---- composing ----------------------------------------------------------------------------

/// Box-blurs the alpha channel of a black (premultiplied) pixmap.
fn blur_alpha(pm: &mut Pixmap, radius: usize) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let mut a: Vec<u32> = pm.data().chunks_exact(4).map(|p| p[3] as u32).collect();
    let mut tmp = vec![0u32; w.max(h)];
    let win = (2 * radius + 1) as u32;
    for _ in 0..3 {
        for y in 0..h {
            let row = &mut a[y * w..(y + 1) * w];
            let at = |x: isize, row: &[u32]| row[x.clamp(0, w as isize - 1) as usize];
            let mut acc: u32 = (-(radius as isize)..=radius as isize).map(|k| at(k, row)).sum();
            for x in 0..w {
                tmp[x] = acc / win;
                acc += at(x as isize + radius as isize + 1, row);
                acc -= at(x as isize - radius as isize, row);
            }
            row.copy_from_slice(&tmp[..w]);
        }
        for x in 0..w {
            let at = |y: isize, a: &[u32]| a[y.clamp(0, h as isize - 1) as usize * w + x];
            let mut acc: u32 = (-(radius as isize)..=radius as isize).map(|k| at(k, &a)).sum();
            for y in 0..h {
                tmp[y] = acc / win;
                acc += at(y as isize + radius as isize + 1, &a);
                acc -= at(y as isize - radius as isize, &a);
            }
            for y in 0..h {
                a[y * w + x] = tmp[y];
            }
        }
    }
    for (p, v) in pm.data_mut().chunks_exact_mut(4).zip(a) {
        p.copy_from_slice(&[0, 0, 0, v.min(255) as u8]);
    }
}

/// Source pixels (index, weight) that make up each output pixel when shrinking `src` to `dst`
/// pixels: exact area coverage, weights sum to 1.
fn box_weights(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let step = src as f32 / dst as f32;
    (0..dst)
        .map(|i| {
            let (a, b) = (i as f32 * step, ((i + 1) as f32 * step).min(src as f32));
            let first = a.floor() as usize;
            let last = (b.ceil() as usize).min(src).max(first + 1);
            (first..last)
                .map(|k| (k, ((b.min(k as f32 + 1.0) - a.max(k as f32)) / step).max(0.0)))
                .filter(|&(_, w)| w > 0.0)
                .collect()
        })
        .collect()
}

/// Area-averaging downscale of a premultiplied pixmap (bicubic alone aliases when shrinking a
/// lot). Works row by row, so it needs little memory even for very tall scrolling captures.
fn downscale(src: &Pixmap, factor: f32) -> Option<Pixmap> {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let w = ((sw as f32 * factor).round() as usize).clamp(1, sw);
    let h = ((sh as f32 * factor).round() as usize).clamp(1, sh);
    let (xw, yw) = (box_weights(sw, w), box_weights(sh, h));
    let mut out = Pixmap::new(w as u32, h as u32)?;
    let data = src.data();
    let mut row = vec![0f32; w * 4];
    let mut acc = vec![0f32; w * 4];
    for (y, taps) in yw.iter().enumerate() {
        acc.fill(0.0);
        for &(sy, wy) in taps {
            let line = &data[sy * sw * 4..(sy + 1) * sw * 4];
            for (o, xt) in row.chunks_exact_mut(4).zip(&xw) {
                let mut px = [0f32; 4];
                for &(sx, wx) in xt {
                    let p = &line[sx * 4..sx * 4 + 4];
                    for c in 0..4 {
                        px[c] += p[c] as f32 * wx;
                    }
                }
                o.copy_from_slice(&px);
            }
            for (a, r) in acc.iter_mut().zip(&row) {
                *a += r * wy;
            }
        }
        let dst = &mut out.data_mut()[y * w * 4..(y + 1) * w * 4];
        for (d, px) in dst.chunks_exact_mut(4).zip(acc.chunks_exact(4)) {
            let a = px[3].round().clamp(0.0, 255.0);
            for c in 0..3 {
                d[c] = px[c].round().clamp(0.0, a) as u8; // stays valid premultiplied
            }
            d[3] = a as u8;
        }
    }
    Some(out)
}

/// Shrinks a premultiplied pixmap so it is `w` pixels wide (for the editor preview).
pub fn shrink_to_width(src: &Pixmap, w: f32) -> Option<Pixmap> {
    downscale(src, w / src.width() as f32)
}

/// Renders the framed image. `src` is the (annotated, cropped) screenshot, premultiplied.
/// `scale` < 1 renders a smaller preview of the same composition.
pub fn compose(src: &Pixmap, l: &Layout, s: &FrameSettings, fonts: &Fonts, scale: f32) -> Option<Pixmap> {
    let w = ((l.size.x * scale).round() as u32).max(1);
    let h = ((l.size.y * scale).round() as u32).max(1);
    let mut out = Pixmap::new(w, h)?;
    let t = Transform::from_scale(scale, scale);

    if s.background_on {
        if let Some(fill) = background_fill(&s.background) {
            paint_fill(&mut out, &fill);
            if s.grain {
                add_grain(&mut out);
            }
        }
    }

    let title = s.url.trim();
    // Shadow, rendered small, blurred and scaled up (fast at any size).
    if s.shadow > 0.0 && l.shadow_blur * scale >= 1.0 {
        let f = 4.0;
        let (mw, mh) = (((w as f32) / f).ceil() as u32 + 1, ((h as f32) / f).ceil() as u32 + 1);
        if let Some(mut m) = Pixmap::new(mw, mh) {
            let mt = Transform::from_scale(1.0 / f, 1.0 / f).pre_concat(t);
            match &l.device {
                Some(p) => {
                    draw_device(&mut m, p.device, &p.geom, mt.pre_concat(p.tf), s.finish, Layer::Silhouette, fonts, "")
                }
                None => {
                    if let Some(path) = rrect(l.clip, l.clip_radii) {
                        m.fill_path(&path, &solid(hex(0, 255)), FillRule::Winding, mt, None);
                    }
                }
            }
            let blur = l.shadow_blur * scale / f;
            blur_alpha(&mut m, (blur / 1.6).round().max(1.0) as usize);
            let opacity = ((0.62 * s.shadow).clamp(0.0, 1.0) * 256.0).round() as u32;
            let dy = l.shadow_blur * scale * 0.45;
            // Scale the blurred mask up while darkening the output (black, source-over).
            let (xt, yt) = (bilinear_taps(w, mw, f, 0.0), bilinear_taps(h, mh, f, dy));
            let (mask, row) = (m.data(), w as usize * 4);
            for (y, d) in out.data_mut().chunks_exact_mut(row).enumerate() {
                let (y0, y1, fy) = yt[y];
                let (r0, r1) = (&mask[y0 * mw as usize * 4..], &mask[y1 * mw as usize * 4..]);
                for (o, &(x0, x1, fx)) in d.chunks_exact_mut(4).zip(&xt) {
                    let top = r0[x0 * 4 + 3] as u32 * (256 - fx) + r0[x1 * 4 + 3] as u32 * fx;
                    let bot = r1[x0 * 4 + 3] as u32 * (256 - fx) + r1[x1 * 4 + 3] as u32 * fx;
                    let a = ((((top * (256 - fy) + bot * fy) >> 16) * opacity) >> 8).min(255); // 0..=255
                    if a > 0 {
                        let keep = 255 - a;
                        for c in &mut o[..3] {
                            *c = ((*c as u32 * keep + 127) / 255) as u8;
                        }
                        o[3] = (a + (o[3] as u32 * keep + 127) / 255) as u8;
                    }
                }
            }
        }
    }

    if let Some(p) = &l.device {
        draw_device(&mut out, p.device, &p.geom, t.pre_concat(p.tf), s.finish, Layer::Under, fonts, title);
    }

    // The screenshot, clipped to the (rounded) screen.
    let clip = map_rect(t, l.clip);
    let radii = l.clip_radii.map(|r| r * scale);
    if let Some(clip_path) = rrect(clip, radii) {
        let mut mask = Mask::new(w, h)?;
        mask.fill_path(&clip_path, FillRule::Winding, true, Transform::identity());
        if l.letterbox {
            out.fill_path(&clip_path, &solid(hex(0, 255)), FillRule::Winding, Transform::identity(), None);
        }
        let shot = map_rect(t, l.shot);
        let k = shot.width() / src.width() as f32;
        let small;
        let (img, mut k) = if k < 0.7 {
            small = downscale(src, k);
            match &small {
                Some(sm) => (sm, shot.width() / sm.width() as f32),
                None => (src, k),
            }
        } else {
            (src, k)
        };
        let mut exact = (k - 1.0).abs() < 1e-4 && shot.min.x.fract() == 0.0 && shot.min.y.fract() == 0.0;
        let mut origin = shot.min;
        if scale < 1.0 && (k - 1.0).abs() * img.width() as f32 <= 1.0 {
            // Preview with a pre-shrunk screenshot (within a pixel of its size): copy it 1:1 on
            // whole pixels instead of resampling it, which would make it soft.
            (exact, k, origin) = (true, 1.0, shot.min.round());
        }
        let quality = if exact {
            FilterQuality::Nearest
        } else if scale < 1.0 {
            FilterQuality::Bilinear // editor preview: several times faster, looks the same on screen
        } else {
            FilterQuality::Bicubic
        };
        let pt = Transform::from_scale(k, k).post_translate(origin.x, origin.y);
        let paint = shader_paint(Pattern::new(img.as_ref(), SpreadMode::Pad, quality, 1.0, pt));
        let area = shot.intersect(clip);
        if area.is_positive() {
            out.fill_rect(to_sk(area), &paint, Transform::identity(), Some(&mask));
        }
    }

    if let Some(p) = &l.device {
        draw_device(&mut out, p.device, &p.geom, t.pre_concat(p.tf), s.finish, Layer::Over, fonts, title);
    }
    Some(out)
}

/// Small preview swatch of a background (for the settings panel), premultiplied RGBA.
pub fn thumbnail(bg: &Background, w: u32, h: u32) -> Option<Pixmap> {
    let mut pm = Pixmap::new(w, h)?;
    match background_fill(bg) {
        Some(f) => paint_fill(&mut pm, &f),
        None => {
            // Checkerboard for "transparent".
            let cell = (h / 4).max(2);
            for y in 0..h {
                for x in 0..w {
                    let v = if (x / cell + y / cell) % 2 == 0 { 0xd4 } else { 0xf4 };
                    let i = ((y * w + x) * 4) as usize;
                    pm.data_mut()[i..i + 4].copy_from_slice(&[v, v, v, 255]);
                }
            }
        }
    }
    Some(pm)
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_unknown_saved_preset_falls_back_to_the_default() {
        let old = FrameSettings { background: Background::Preset("volt".into()), ..Default::default() };
        assert_eq!(old.with_known_background().background, FrameSettings::default().background);
        let kept = FrameSettings { background: Background::Preset("ocean".into()), ..Default::default() };
        assert_eq!(kept.clone().with_known_background().background, kept.background);
    }

    use super::*;

    fn src(w: u32, h: u32) -> Pixmap {
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(Color::from_rgba8(200, 30, 30, 255));
        pm
    }

    #[test]
    fn auto_device_matches_aspect() {
        assert_eq!(auto_device(1179.0, 2556.0).0.id, "iphone-15-pro");
        let (d, rot) = auto_device(2556.0, 1179.0);
        assert_eq!((d.id, rot), ("iphone-15-pro", true));
        assert_eq!(auto_device(1920.0, 1080.0).0.id, "laptop-16-9");
        assert_eq!(auto_device(3440.0, 1440.0).0.id, "ultrawide");
        // Nothing close: falls back to a frame that adapts.
        assert_eq!(auto_device(1000.0, 1000.0).0.id, "browser");
    }

    #[test]
    fn background_layout_keeps_native_size() {
        let s = FrameSettings { background_on: true, padding: 0.1, ..Default::default() };
        let l = layout(&s, 800.0, 400.0).unwrap();
        assert_eq!(l.shot.size(), vec2(800.0, 400.0));
        assert_eq!(l.size, vec2(920.0, 520.0));
        let sq = FrameSettings { aspect: Aspect::Square, ..s };
        let l = layout(&sq, 800.0, 400.0).unwrap();
        assert_eq!(l.size.x, l.size.y);
        assert!(layout(&FrameSettings::default(), 800.0, 400.0).is_none());
    }

    #[test]
    fn fill_covers_and_fit_contains() {
        let mut s = FrameSettings { mockup_on: true, device: Some("iphone-15-pro".into()), ..Default::default() };
        s.orientation = Orientation::Portrait;
        let l = layout(&s, 1000.0, 2000.0).unwrap();
        assert!(l.shot.width() >= l.clip.width() - 0.01 && l.shot.height() >= l.clip.height() - 0.01);
        assert!((l.scale(1000.0) - 1.0).abs() < 1e-3, "keeps native resolution");
        s.fit = Fit::Fit;
        let l = layout(&s, 1000.0, 2000.0).unwrap();
        assert!(l.shot.width() <= l.clip.width() + 0.01 && l.shot.height() <= l.clip.height() + 0.01);
        assert!(l.letterbox);
    }

    #[test]
    fn compose_draws_screenshot_and_frame() {
        let fonts = Fonts::load();
        let pm = src(300, 200);
        for id in ["browser", "laptop-16-9", "iphone-15-pro", "imac-24", "macbook-pro-14", "ipad-pro-11"] {
            let s =
                FrameSettings { mockup_on: true, background_on: true, device: Some(id.into()), ..Default::default() };
            let l = layout(&s, 300.0, 200.0).unwrap();
            let out = compose(&pm, &l, &s, &fonts, 1.0).unwrap();
            assert_eq!((out.width() as f32, out.height() as f32), (l.size.x, l.size.y));
            let c = l.clip.center();
            let px = out.pixel(c.x as u32, c.y as u32).unwrap();
            assert_eq!((px.red(), px.green(), px.blue()), (200, 30, 30), "{id}: screenshot visible in screen");
        }
        // Transparent corners without a background; rounded corners cut the screenshot.
        let s = FrameSettings {
            background_on: true,
            background: Background::Transparent,
            radius: 20.0,
            shadow: 0.0,
            padding: 0.0,
            ..Default::default()
        };
        let l = layout(&s, 300.0, 200.0).unwrap();
        let out = compose(&pm, &l, &s, &fonts, 1.0).unwrap();
        assert_eq!(out.pixel(0, 0).unwrap().alpha(), 0);
        assert_eq!(out.pixel(150, 100).unwrap().alpha(), 255);
    }

    /// Timing of the editor preview: `cargo test --release compose_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn compose_timing() {
        let fonts = Fonts::load();
        let pm = src(2560, 1440);
        let cases = [
            (
                "mesh bg",
                FrameSettings {
                    background_on: true,
                    background: Background::Preset("aurora".into()),
                    grain: true,
                    ..Default::default()
                },
            ),
            (
                "laptop",
                FrameSettings {
                    mockup_on: true,
                    background_on: true,
                    device: Some("macbook-pro-14".into()),
                    ..Default::default()
                },
            ),
        ];
        let t = std::time::Instant::now();
        shrink_to_width(&pm, 1280.0).unwrap();
        println!("shrink to half: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        for (name, s) in cases {
            let l = layout(&s, 2560.0, 1440.0).unwrap();
            for scale in [1.0, 0.5, 0.35] {
                let t = std::time::Instant::now();
                let n = 5;
                for _ in 0..n {
                    compose(&pm, &l, &s, &fonts, scale).unwrap();
                }
                println!("{name} {:?} at {scale}: {:.1} ms", l.size, t.elapsed().as_secs_f64() * 1000.0 / n as f64);
            }
        }
    }

    /// Renders every device and background to PNGs for visual review:
    /// `BLINK_GALLERY=<dir> cargo test gallery -- --ignored`
    #[test]
    #[ignore]
    fn gallery() {
        let Some(dir) = std::env::var_os("BLINK_GALLERY") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fonts = Fonts::load();
        let fake = |w: u32, h: u32| {
            let img = image::RgbaImage::from_fn(w, h, |x, y| {
                if y < h / 12 {
                    image::Rgba([30, 41, 59, 255])
                } else if (x / 40 + y / 40) % 2 == 0 {
                    image::Rgba([241, 245, 249, 255])
                } else {
                    image::Rgba([186, 230, 253, 255])
                }
            });
            Pixmap::from_vec(img.into_raw(), tiny_skia::IntSize::from_wh(w, h).unwrap()).unwrap()
        };
        let save = |pm: &Pixmap, name: &str| {
            crate::render::unpremultiply(pm).save(dir.join(format!("{name}.png"))).unwrap();
        };
        for d in DEVICES {
            for finish in [Finish::Dark, Finish::Light] {
                let (w, h) = if d.flexible() { (1280, 800) } else { (d.res.0 / 3, d.res.1 / 3) };
                let s = FrameSettings {
                    mockup_on: true,
                    background_on: true,
                    device: Some(d.id.into()),
                    finish,
                    url: "blink.app/screenshot".into(),
                    ..Default::default()
                };
                let l = layout(&s, w as f32, h as f32).unwrap();
                let out = compose(&fake(w, h), &l, &s, &fonts, (900.0 / l.size.x.max(l.size.y)).min(1.0)).unwrap();
                save(&out, &format!("{}-{:?}", d.id, finish));
            }
        }
        let (d, _) = auto_device(2556.0, 1179.0);
        let s = FrameSettings {
            mockup_on: true,
            device: Some(d.id.into()),
            orientation: Orientation::Landscape,
            ..Default::default()
        };
        let l = layout(&s, 852.0, 393.0).unwrap();
        save(&compose(&fake(852, 393), &l, &s, &fonts, 1.0).unwrap(), "phone-landscape-transparent");
        for p in PRESETS {
            let s = FrameSettings {
                background_on: true,
                background: Background::Preset(p.id.into()),
                ..Default::default()
            };
            let l = layout(&s, 640.0, 400.0).unwrap();
            save(&compose(&fake(640, 400), &l, &s, &fonts, 1.0).unwrap(), &format!("bg-{}", p.id));
        }
    }
}
