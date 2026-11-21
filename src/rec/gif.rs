//! Animated GIF from the frames of a recording.
//!
//! A screen recording mostly stands still: a pointer moves, a menu opens. So after the first
//! frame each frame only stores the rectangle that changed, with the pixels inside it that
//! stayed the same left transparent (they compress to almost nothing), and frames without a
//! change only make the previous one last longer. Every frame gets its own palette (NeuQuant,
//! from the `gif` crate's `color_quant`), which keeps colors right where a frame has few.
//!
//! Decoded video is not exactly the same from frame to frame where nothing moved, so small
//! differences (below `THRESHOLD` per channel) do not count as a change. Differences are
//! measured against what the GIF shows at that moment, so slow changes still get through
//! once they add up.

use super::export::{output_size, FrameSink};
use super::yuv::Nv12;
use image::RgbaImage;
use std::io::Write;

/// Largest difference per color channel that is still "the same pixel".
const THRESHOLD: i32 = 10;
/// NeuQuant speed (1 best .. 30 fastest).
const SPEED: i32 = 10;

/// Counts the bytes that pass through.
pub struct Counter<W: Write> {
    pub inner: W,
    pub bytes: u64,
}

impl<W: Write> Write for Counter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// A frame waiting for the next change, which decides how long it lasts.
struct Pending {
    frame: ::gif::Frame<'static>,
    t: u32,
}

pub struct GifSink<W: Write> {
    enc: Option<::gif::Encoder<Counter<W>>>,
    out: Option<W>,
    /// What the GIF shows now (the intended colors, before the palette).
    shown: Option<RgbaImage>,
    pending: Option<Pending>,
    bytes: u64,
    /// Video frames wider than this are scaled down.
    max_width: Option<u32>,
    /// Bytes written once the first frame was out (the header and the one whole frame).
    first_bytes: Option<u64>,
}

impl<W: Write> GifSink<W> {
    pub fn new(out: W) -> Self {
        Self { enc: None, out: Some(out), shown: None, pending: None, bytes: 0, max_width: None, first_bytes: None }
    }

    /// Scales video frames down to at most `w` pixels wide.
    pub fn max_width(mut self, w: Option<u32>) -> Self {
        self.max_width = w;
        self
    }

    /// The bytes of the header and the first frame, once it is written.
    pub fn first_frame_bytes(&self) -> Option<u64> {
        self.first_bytes
    }

    fn encoder(&mut self, w: u32, h: u32) -> Result<&mut ::gif::Encoder<Counter<W>>, String> {
        if self.enc.is_none() {
            let out = self.out.take().ok_or("GIF already finished")?;
            let mut enc = ::gif::Encoder::new(Counter { inner: out, bytes: 0 }, w as u16, h as u16, &[])
                .map_err(|e| e.to_string())?;
            enc.set_repeat(::gif::Repeat::Infinite).map_err(|e| e.to_string())?;
            self.enc = Some(enc);
        }
        Ok(self.enc.as_mut().expect("just made"))
    }

    /// Writes the pending frame, lasting until `t`.
    fn flush_pending(&mut self, t: u32) -> Result<(), String> {
        if let Some(mut p) = self.pending.take() {
            // In hundredths of a second, rounded per frame from the start so the total stays
            // right. Browsers show delays under 2 as 10, so never below 2.
            let delay = (t as i64 + 5) / 10 - (p.t as i64 + 5) / 10;
            p.frame.delay = delay.clamp(2, u16::MAX as i64) as u16;
            let (w, h) = self.shown.as_ref().map_or((1, 1), |s| s.dimensions());
            let enc = self.encoder(w, h)?;
            enc.write_frame(&p.frame).map_err(|e| e.to_string())?;
            let written = enc.get_ref().bytes;
            self.first_bytes.get_or_insert(written);
        }
        Ok(())
    }
}

fn differs(a: &[u8], b: &[u8]) -> bool {
    (0..3).any(|i| (a[i] as i32 - b[i] as i32).abs() > THRESHOLD)
}

/// The rectangle (x, y, w, h) that holds every pixel of `new` that differs from `old`.
fn changed_rect(old: &RgbaImage, new: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = new.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..h {
        let (ro, rn) = (row(old, y), row(new, y));
        let mut first = None;
        let mut last = 0;
        for x in 0..w as usize {
            if differs(&ro[x * 4..x * 4 + 4], &rn[x * 4..x * 4 + 4]) {
                first.get_or_insert(x);
                last = x;
            }
        }
        if let Some(f) = first {
            x0 = x0.min(f as u32);
            x1 = x1.max(last as u32);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
    }
    (x0 != u32::MAX).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

fn row(img: &RgbaImage, y: u32) -> &[u8] {
    let w = img.width() as usize * 4;
    &img.as_raw()[y as usize * w..(y as usize + 1) * w]
}

impl<W: Write> FrameSink for GifSink<W> {
    fn push(&mut self, frame: &Nv12, t: u32) -> Result<(), String> {
        let (ow, oh) = output_size(frame.w, frame.h, self.max_width);
        let rgba = frame.to_rgba_scaled(ow, oh);
        self.push_rgba(&rgba, t)
    }

    fn finish(&mut self, end: u32) -> Result<u64, String> {
        self.finish_gif(end)
    }
}

impl<W: Write> GifSink<W> {
    /// Adds a frame shown from `t` ms.
    pub fn push_rgba(&mut self, frame: &RgbaImage, t: u32) -> Result<(), String> {
        let (w, h) = frame.dimensions();
        let (rect, rgba) = match &mut self.shown {
            None => {
                let rgba: Vec<u8> = frame.pixels().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
                self.shown = Some(frame.clone());
                ((0, 0, w, h), rgba)
            }
            Some(shown) => {
                if shown.dimensions() != (w, h) {
                    return Err("GIF frames differ in size".into());
                }
                let Some((x, y, rw, rh)) = changed_rect(shown, frame) else { return Ok(()) };
                let mut rgba = Vec::with_capacity((rw * rh * 4) as usize);
                for yy in y..y + rh {
                    for xx in x..x + rw {
                        let n = frame.get_pixel(xx, yy);
                        let s = shown.get_pixel_mut(xx, yy);
                        if differs(&s.0, &n.0) {
                            *s = *n;
                            rgba.extend_from_slice(&[n[0], n[1], n[2], 255]);
                        } else {
                            rgba.extend_from_slice(&[0, 0, 0, 0]);
                        }
                    }
                }
                ((x, y, rw, rh), rgba)
            }
        };
        self.flush_pending(t)?;
        let mut rgba = rgba;
        let mut f = ::gif::Frame::from_rgba_speed(rect.2 as u16, rect.3 as u16, &mut rgba, SPEED);
        f.left = rect.0 as u16;
        f.top = rect.1 as u16;
        f.dispose = ::gif::DisposalMethod::Keep;
        self.pending = Some(Pending { frame: f, t });
        Ok(())
    }

    /// Ends the GIF at `end` ms; returns its size.
    pub fn finish_gif(&mut self, end: u32) -> Result<u64, String> {
        self.flush_pending(end)?;
        if let Some(enc) = self.enc.take() {
            let mut counter = enc.into_inner().map_err(|e| e.to_string())?;
            counter.flush().map_err(|e| e.to_string())?;
            self.bytes = counter.bytes;
            self.out = Some(counter.inner);
        }
        Ok(self.bytes)
    }
}

impl<W: Write> GifSink<W> {
    /// The output, after `finish`.
    #[cfg(test)]
    pub fn into_output(self) -> Option<W> {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(t: u32) -> RgbaImage {
        // A static background with some detail and a square that moves 4 px per frame.
        let mut img = RgbaImage::from_fn(160, 90, |x, y| image::Rgba([(x * 3) as u8, (y * 5) as u8, 120, 255]));
        let sx = (t / 40 * 4) % 120;
        for y in 30..50 {
            for x in sx..sx + 20 {
                img.put_pixel(x, y, image::Rgba([250, 40, 40, 255]));
            }
        }
        img
    }

    fn encode(frames: &[(u32, RgbaImage)], end: u32) -> (Vec<u8>, u64) {
        let mut sink = GifSink::new(Vec::new());
        for (t, f) in frames {
            sink.push_rgba(f, *t).unwrap();
        }
        let bytes = sink.finish_gif(end).unwrap();
        (sink.into_output().unwrap(), bytes)
    }

    fn decode(data: &[u8]) -> Vec<(u16, u16, u16, u16, u16)> {
        let mut opts = ::gif::DecodeOptions::new();
        opts.set_color_output(::gif::ColorOutput::RGBA);
        let mut dec = opts.read_info(data).unwrap();
        let mut out = Vec::new();
        while let Some(f) = dec.read_next_frame().unwrap() {
            out.push((f.left, f.top, f.width, f.height, f.delay));
        }
        out
    }

    /// Decodes with the image crate (which composes the frames) and returns every frame.
    fn composed(data: &[u8]) -> Vec<RgbaImage> {
        use image::AnimationDecoder;
        let dec = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(data)).unwrap();
        dec.into_frames().map(|f| f.unwrap().into_buffer()).collect()
    }

    #[test]
    fn writes_a_valid_gif_that_looks_like_the_frames() {
        let frames: Vec<(u32, RgbaImage)> = (0..10).map(|i| (i * 80, scene(i * 80))).collect();
        let (data, counted) = encode(&frames, 800);
        assert_eq!(data.len() as u64, counted, "the counter counts every byte");
        assert_eq!(&data[..6], b"GIF89a");
        let shown = composed(&data);
        assert_eq!(shown.len(), 10);
        for ((_, want), got) in frames.iter().zip(&shown) {
            let mut worst = 0;
            for (a, b) in want.pixels().zip(got.pixels()) {
                for i in 0..3 {
                    worst = worst.max((a[i] as i32 - b[i] as i32).abs());
                }
            }
            assert!(worst <= 48, "colors stay close: {worst}");
        }
    }

    #[test]
    fn later_frames_only_store_what_changed() {
        let frames: Vec<(u32, RgbaImage)> = (0..5).map(|i| (i * 80, scene(i * 80))).collect();
        let (data, _) = encode(&frames, 400);
        let f = decode(&data);
        assert_eq!((f[0].0, f[0].1, f[0].2, f[0].3), (0, 0, 160, 90), "the first frame is whole");
        for fr in &f[1..] {
            assert!(fr.2 <= 30 && fr.3 == 20, "only around the square: {fr:?}");
            assert_eq!(fr.1, 30);
        }
    }

    #[test]
    fn frames_without_change_make_the_previous_one_last_longer() {
        let still = scene(0);
        let moved = scene(80);
        let frames = vec![(0, still.clone()), (100, still.clone()), (200, still.clone()), (300, moved)];
        let (data, _) = encode(&frames, 500);
        let f = decode(&data);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].4, 30, "0.3 s");
        assert_eq!(f[1].4, 20, "until the end");
        // Noise below the threshold is no change.
        let mut noisy = still.clone();
        for p in noisy.pixels_mut() {
            p[0] = p[0].saturating_add(3);
        }
        let (data, _) = encode(&[(0, still), (100, noisy)], 200);
        assert_eq!(decode(&data).len(), 1);
    }

    #[test]
    fn delays_add_up_and_never_drop_below_two() {
        // 30 fps: 3.33 cs per frame, rounded per frame from the start.
        let frames: Vec<(u32, RgbaImage)> = (0..30).map(|i| (i * 1000 / 30, scene(i * 40))).collect();
        let (data, _) = encode(&frames, 1000);
        let f = decode(&data);
        let total: u32 = f.iter().map(|x| x.4 as u32).sum();
        assert!((99..=101).contains(&total), "{total}");
        assert!(f.iter().all(|x| x.4 >= 2));
    }

    #[test]
    fn a_frame_of_another_size_is_refused() {
        let mut sink = GifSink::new(Vec::new());
        sink.push_rgba(&scene(0), 0).unwrap();
        assert!(sink.push_rgba(&RgbaImage::new(10, 10), 100).is_err());
    }
}
