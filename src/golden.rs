//! Golden outputs and timings for the image pipelines (render, redact, export, frame, stitch,
//! OCR input). The digests were recorded before the memory work of 1.3.3: a change that
//! alters a single output pixel fails here. Text is drawn with the egui fallback font off
//! Windows, so the render and frame digests are only checked there.
//!
//! Timings, before and after a change:
//! `cargo test --release golden::timing -- --ignored --nocapture`

use crate::annot::{Annotation, Kind, RedactMode, Style};
use crate::frame::{self, Background, FrameSettings};
use crate::render::{self, Fonts, Renderer};
use eframe::egui::{pos2, Color32, Rect};
use image::RgbaImage;
use sha2::{Digest, Sha256};
use tiny_skia::Pixmap;

fn digest(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().take(10).map(|b| format!("{b:02x}")).collect()
}

/// A screenshot-like test image: gradients, fine text-like stripes, hard edges, noise and a
/// half-transparent corner (so premultiplying and unpremultiplying are covered too).
fn shot(w: u32, h: u32) -> RgbaImage {
    let mut s: u32 = 0x1234_5678;
    RgbaImage::from_fn(w, h, |x, y| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let n = (s & 15) as u8;
        let stripe = if (y / 3) % 4 == 0 && (x / 2) % 3 != 0 { 90 } else { 0 };
        let r = ((x * 255 / w.max(1)) as u8).saturating_sub(stripe).wrapping_add(n);
        let g = ((y * 255 / h.max(1)) as u8).saturating_sub(stripe);
        let b = if (x / 40 + y / 40) % 2 == 0 { 230 } else { 40 };
        let a = if x < w / 6 && y < h / 6 { 140 } else { 255 };
        image::Rgba([r, g, b, a])
    })
}

fn ann(id: u64, kind: Kind, a: (f32, f32), b: (f32, f32), style: Style) -> Annotation {
    Annotation { id, kind, a: pos2(a.0, a.1), b: pos2(b.0, b.1), style }
}

fn annotations(w: f32, h: f32) -> Vec<Annotation> {
    let st = Style::default();
    let strong = Style { strength: 10, ..st };
    let weak = Style { strength: 1, ..st };
    vec![
        ann(1, Kind::Redact(RedactMode::Blur), (0.0, 0.0), (w * 0.55, h * 0.6), st),
        ann(2, Kind::Redact(RedactMode::Blur), (w * 0.6, h * 0.1), (w * 0.95, h * 0.2), strong),
        ann(3, Kind::Redact(RedactMode::Blur), (w * 0.62, h * 0.25), (w * 0.7, h * 0.27), weak),
        ann(4, Kind::Redact(RedactMode::Pixelate), (w * 0.1, h * 0.7), (w * 0.4, h * 0.9), st),
        ann(5, Kind::Redact(RedactMode::Solid), (w * 0.5, h * 0.7), (w * 0.6, h * 0.8), st),
        // Runs past the image edge: clipped.
        ann(6, Kind::Redact(RedactMode::Blur), (w * 0.8, h * 0.8), (w * 1.2, h * 1.3), st),
        ann(7, Kind::Rect, (w * 0.05, h * 0.05), (w * 0.3, h * 0.3), st),
        ann(8, Kind::Arrow, (w * 0.3, h * 0.5), (w * 0.9, h * 0.4), st),
        ann(9, Kind::Text("Golden 123".into()), (w * 0.4, h * 0.45), (w * 0.4, h * 0.45), st),
        ann(10, Kind::Step(2), (w * 0.7, h * 0.6), (w * 0.7, h * 0.6), st),
        ann(11, Kind::Highlight, (w * 0.2, h * 0.35), (w * 0.5, h * 0.4), Style { stroke: Color32::YELLOW, ..st }),
    ]
}

fn pixmap_from(img: &RgbaImage) -> Pixmap {
    Pixmap::from_vec(img.as_raw().clone(), tiny_skia::IntSize::from_wh(img.width(), img.height()).unwrap()).unwrap()
}

#[test]
#[cfg_attr(windows, ignore = "text is drawn with Segoe UI on Windows: other digests")]
fn render_and_export_are_unchanged() {
    let fonts = Fonts::load();
    let mut got = Vec::new();
    for (w, h) in [(640u32, 360u32), (333, 517)] {
        let img = shot(w, h);
        let r = Renderer::new(&img, fonts.clone());
        got.push(digest(r.base().data()));
        let anns = annotations(w as f32, h as f32);
        got.push(digest(r.render(&anns, None).data()));
        got.push(digest(r.render(&anns, Some(2)).data()));
        got.push(digest(r.export(&anns, None).as_raw()));
        let crop = Rect::from_min_max(pos2(13.0, 7.0), pos2(w as f32 - 21.0, h as f32 - 5.0));
        got.push(digest(r.export(&anns, Some(crop)).as_raw()));
        got.push(digest(render::crop_pixmap(&r.render(&anns, None), Some(crop)).data()));
        got.push(digest(render::crop_pixmap(&r.render(&anns, None), None).data()));
        // Owned variants (no copies) must give the same pixels as the borrowed ones.
        assert_eq!(Renderer::from_image(img.clone(), fonts.clone()).base().data(), r.base().data());
        let pm = r.render(&anns, None);
        assert_eq!(render::unpremultiply_owned(pm.clone()), render::unpremultiply(&pm));
        let cropped = render::crop_pixmap_owned(pm.clone(), Some(crop));
        assert_eq!(cropped.data(), render::crop_pixmap(&pm, Some(crop)).data());
        assert_eq!(render::crop_pixmap_owned(pm.clone(), None).data(), pm.data());
        // The editor's texture holds the same pixels as the pixmap, also cut into tiles.
        let whole = crate::editor::texture_image(&pm, 0, 0, w as usize, h as usize);
        assert_eq!(whole, eframe::egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pm.data()));
        let tile = crate::editor::texture_image(&pm, 100, 50, 64, 40);
        let mut data = Vec::new();
        for y in 50..90 {
            data.extend_from_slice(&pm.data()[(y * w as usize + 100) * 4..][..64 * 4]);
        }
        assert_eq!(tile, eframe::egui::ColorImage::from_rgba_premultiplied([64, 40], &data));
    }
    let want = GOLDEN_RENDER;
    assert_eq!(got, want, "render output changed:\n{got:#?}");
}

#[test]
fn redact_is_unchanged() {
    // Straight on a buffer, with a stride wider than the region, every mode and strength.
    let img = shot(300, 200);
    let mut got = Vec::new();
    for mode in [RedactMode::Blur, RedactMode::Pixelate, RedactMode::Solid] {
        for strength in [1u8, 4, 10] {
            for rect in [(0, 0, 300, 200), (17, 9, 101, 63), (250, 150, 50, 50), (5, 5, 3, 120)] {
                let mut data = img.as_raw().clone();
                render::redact(&mut data, 300, rect, mode, strength, 42);
                got.push(digest(&data));
            }
        }
    }
    let joined = digest(got.join("").as_bytes());
    assert_eq!(joined, GOLDEN_REDACT, "redact output changed");
}

#[test]
#[cfg_attr(windows, ignore = "text is drawn with Segoe UI on Windows: other digests")]
fn frame_compose_is_unchanged() {
    let fonts = Fonts::load();
    let img = shot(400, 250);
    let r = Renderer::new(&img, fonts.clone());
    let src = render::crop_pixmap(&r.render(&annotations(400.0, 250.0), None), None);
    let cases = [
        FrameSettings { background_on: true, ..Default::default() },
        FrameSettings {
            background_on: true,
            background: Background::Preset("aurora".into()),
            grain: true,
            ..Default::default()
        },
        FrameSettings { mockup_on: true, background_on: true, device: Some("browser".into()), ..Default::default() },
        FrameSettings {
            mockup_on: true,
            background_on: true,
            device: Some("macbook-pro-14".into()),
            ..Default::default()
        },
        FrameSettings {
            background_on: true,
            background: Background::Transparent,
            radius: 20.0,
            padding: 0.0,
            ..Default::default()
        },
    ];
    let mut got = Vec::new();
    for s in &cases {
        let l = frame::layout(s, 400.0, 250.0).unwrap();
        for scale in [1.0, 0.5] {
            got.push(digest(frame::compose(&src, &l, s, &fonts, scale).unwrap().data()));
        }
    }
    got.push(digest(frame::shrink_to_width(&src, 150.0).unwrap().data()));
    assert_eq!(got, GOLDEN_FRAME, "frame output changed:\n{got:#?}");
}

/// A long page and what a scrolling capture would see of it: a sticky header and footer and
/// a view that moves down by `steps`.
fn page(w: u32, h: u32) -> RgbaImage {
    let base = shot(w, h);
    RgbaImage::from_fn(w, h, |x, y| {
        let p = base.get_pixel(x, y).0;
        // Opaque, and a different line pattern every 37 rows so overlaps are unambiguous.
        let line = if (y % 37) < 5 && x % 7 != (y / 37) % 7 { 0 } else { p[1] };
        image::Rgba([p[0], line, p[2], 255])
    })
}

fn view(content: &RgbaImage, off: u32, vh: u32) -> RgbaImage {
    let w = content.width();
    RgbaImage::from_fn(w, vh, |x, y| {
        if y < 20 {
            image::Rgba([30, 40, 60, 255])
        } else if y >= vh - 15 {
            image::Rgba([200, 200, 210, 255])
        } else {
            *content.get_pixel(x, off + y)
        }
    })
}

#[test]
fn stitch_is_unchanged() {
    let content = page(260, 1400);
    let mut got = Vec::new();
    for steps in [&[0u32, 90, 180, 300, 420, 560, 700, 840, 990, 1100][..], &[0, 200, 200, 400, 401, 640]] {
        let mut st = crate::stitch::Stitcher::new(100_000);
        let mut prev: Option<RgbaImage> = None;
        let mut log = String::new();
        for &off in steps {
            let v = view(&content, off, 300);
            if let Some(p) = &prev {
                log.push_str(&format!("{}", crate::stitch::nearly_equal(p, &v) as u8));
            }
            log.push_str(&format!("{:?};", st.push(v.clone(), None)));
            prev = Some(v);
        }
        let out = st.finish().unwrap();
        got.push(format!("{log}{}x{}:{}", out.width(), out.height(), digest(out.as_raw())));
    }
    // Noise only: nearly equal; a scroll by one row: not.
    let a = view(&content, 100, 300);
    let mut b = a.clone();
    for (i, p) in b.pixels_mut().enumerate() {
        if i % 5 == 0 {
            p.0[0] = p.0[0].saturating_add(1);
        }
    }
    got.push(format!(
        "{}{}",
        crate::stitch::nearly_equal(&a, &b) as u8,
        crate::stitch::nearly_equal(&a, &view(&content, 101, 300)) as u8
    ));
    assert_eq!(got, GOLDEN_STITCH, "stitch output changed:\n{got:#?}");
}

/// Records every image the OCR engine is given.
struct Recorder {
    max: u32,
    seen: std::cell::RefCell<Vec<String>>,
}

impl crate::ocr::Engine for Recorder {
    fn max_dimension(&self) -> u32 {
        self.max
    }
    fn read(&self, bgra: &[u8], w: u32, h: u32) -> Result<crate::ocr::Page, crate::ocr::OcrError> {
        self.seen.borrow_mut().push(format!("{w}x{h}:{}", digest(bgra)));
        Ok(crate::ocr::Page::default())
    }
}

#[test]
fn ocr_input_is_unchanged() {
    let mut got = Vec::new();
    // Enlarged (small), as is, shrunk (too wide) and in bands (too tall); light and dark.
    for (w, h, max) in [(300u32, 200u32, 4000u32), (2000, 1500, 4000), (1200, 300, 1000), (200, 3000, 1000)] {
        for dark in [false, true] {
            let img = shot(w, h);
            let mut bgra = img.into_raw();
            if dark {
                for p in bgra.chunks_exact_mut(4) {
                    p[0] /= 6;
                    p[1] /= 6;
                    p[2] /= 6;
                }
            }
            let e = Recorder { max, seen: Default::default() };
            crate::ocr::read_with(&e, &bgra, w, h).unwrap();
            got.push(e.seen.into_inner().join(","));
        }
    }
    let joined = digest(got.join("|").as_bytes());
    assert_eq!(joined, GOLDEN_OCR, "OCR input changed:\n{got:#?}");
}

/// Timings of the pipelines whose memory use changed, at 4K, in release mode:
/// `cargo test --release golden::timing -- --ignored --nocapture`
#[test]
#[ignore]
fn timing() {
    let fonts = Fonts::load();
    let (w, h) = (3840u32, 2160u32);
    let img = shot(w, h);
    let time = |name: &str, n: u32, f: &mut dyn FnMut()| {
        f();
        let t = std::time::Instant::now();
        for _ in 0..n {
            f();
        }
        println!("{name:<38} {:>8.1} ms", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
    };
    time("Renderer::new 4K", 10, &mut || {
        std::hint::black_box(Renderer::new(&img, fonts.clone()));
    });
    let r = Renderer::new(&img, fonts.clone());
    let full_blur = [ann(1, Kind::Redact(RedactMode::Blur), (0.0, 0.0), (w as f32, h as f32), Style::default())];
    time("render, blur over the whole 4K image", 5, &mut || {
        std::hint::black_box(r.render(&full_blur, None));
    });
    let small_blur = [ann(1, Kind::Redact(RedactMode::Blur), (100.0, 100.0), (700.0, 160.0), Style::default())];
    time("render, blur 600x60", 20, &mut || {
        std::hint::black_box(r.render(&small_blur, None));
    });
    let anns = annotations(w as f32, h as f32);
    time("render, mixed annotations 4K", 5, &mut || {
        std::hint::black_box(r.render(&anns, None));
    });
    time("export without crop 4K", 5, &mut || {
        std::hint::black_box(r.export(&small_blur, None));
    });
    let crop = Rect::from_min_max(pos2(100.0, 100.0), pos2(3000.0, 2000.0));
    time("export with crop 4K", 5, &mut || {
        std::hint::black_box(r.export(&small_blur, Some(crop)));
    });
    let pm = r.render(&small_blur, None);
    time("texture image from 4K pixmap", 10, &mut || {
        std::hint::black_box(crate::editor::texture_image(&pm, 0, 0, w as usize, h as usize));
    });
    let src = pixmap_from(&img);
    let s = FrameSettings { background_on: true, ..Default::default() };
    let l = frame::layout(&s, w as f32, h as f32).unwrap();
    time("frame compose 4K at 0.5", 5, &mut || {
        std::hint::black_box(frame::compose(&src, &l, &s, &fonts, 0.5));
    });
    let content = page(1920, 6000);
    let views: Vec<RgbaImage> = (0..12).map(|i| view(&content, i * 400, 1000)).collect();
    time("stitch 12 frames of 1920x1000", 3, &mut || {
        let mut st = crate::stitch::Stitcher::new(100_000);
        for v in &views {
            st.push(v.clone(), None);
        }
        std::hint::black_box(st.finish());
    });
    time("nearly_equal 1920x1000", 20, &mut || {
        std::hint::black_box(crate::stitch::nearly_equal(&views[0], &views[1]));
    });
    let gray_page = img.as_raw();
    time("QR search 4K (nothing found)", 3, &mut || {
        std::hint::black_box(crate::qr::find_luma(&crate::qr::luma(&gray_page.clone()), w as usize, h as usize));
    });
}

const GOLDEN_RENDER: [&str; 14] = [
    "554c39589ebfc5264124",
    "3323e726557e3891b6c5",
    "c313514fb9cc86ce4415",
    "a56f5f7e2de24178fe00",
    "2cf98adf1d2269850592",
    "755e02a98b6c1e4f5e15",
    "3323e726557e3891b6c5",
    "c3ff1f687bac23c5f456",
    "931d9dc5182467e14497",
    "c9bfa6d01d943292188d",
    "421492f88b2ddd631b1b",
    "68105ed742004cce592a",
    "294cc484d6dfb5e1f51e",
    "931d9dc5182467e14497",
];
const GOLDEN_REDACT: &str = "f8cfdbbab6211dd79cf4";
const GOLDEN_FRAME: [&str; 11] = [
    "3ec05ccf389ee8f0c8ed",
    "e58197d23b143e017da8",
    "5041aba8d63dcfea3163",
    "36778d4b7d09120edfc2",
    "4884c2fb03e84b130cd9",
    "8be672fb251724557be3",
    "b33879dca46bf5120702",
    "97f4ab47aed202b25942",
    "70312a9a58aa68771a81",
    "7b8521e1dbf739f88e8b",
    "3b400b43d896623137fc",
];
const GOLDEN_STITCH: [&str; 3] = [
    "Appended(0);0Appended(90);0Appended(90);0Appended(120);0Appended(120);0Appended(140);0Appended(140);0Appended(140);0Appended(150);0Appended(110);260x1400:0018b5dcb110f1205f1e",
    "Appended(0);0Appended(200);1NoChange;0Appended(200);0Appended(1);0Appended(239);260x940:2f74c97cad7642d50e37",
    "10",
];
const GOLDEN_OCR: &str = "a39aa4cd744c6837c35b";
