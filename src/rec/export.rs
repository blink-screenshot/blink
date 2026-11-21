//! Turning a recording into a video or a GIF: trim, draw the pointer, clicks and keys, and
//! change the frame rate. The decoder (`VideoSource`) and the encoder (`FrameSink`) are traits:
//! on Windows they are Media Foundation, the GIF encoder is portable (and scales the frames
//! down itself), and the tests use frames in memory. Frames stay in NV12 throughout (see
//! `yuv`): only the areas the overlays cover are converted to RGB and back.

use super::compose::{self, Cursors, Overlays, Style};
use super::events::Log;
use super::yuv::Nv12;
use crate::capture_bar::Fonts;

/// Frames of a video, in order.
pub trait VideoSource {
    /// Length in ms.
    fn duration(&self) -> u32;
    /// Goes to `ms`: the next `next` returns the frame shown at `ms` or one before it.
    fn seek(&mut self, ms: u32) -> Result<(), String>;
    /// The next frame and its time in ms, or `None` at the end.
    fn next(&mut self) -> Result<Option<(u32, Nv12)>, String>;
}

/// Receives the output frames, with their time from the start of the output.
pub trait FrameSink {
    fn push(&mut self, frame: &Nv12, t: u32) -> Result<(), String>;
    /// The output ends at `end` ms. Returns its size in bytes.
    fn finish(&mut self, end: u32) -> Result<u64, String>;
}

/// The error of an export that was cancelled.
pub const CANCELLED: &str = "cancelled";

/// The part of the recording to keep, in ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trim {
    pub start: u32,
    pub end: u32,
}

/// Shortest clip an export makes.
pub const MIN_CLIP_MS: u32 = 200;

impl Trim {
    pub fn whole(duration: u32) -> Self {
        Trim { start: 0, end: duration }
    }

    /// Kept inside `0..duration` and at least `MIN_CLIP_MS` long (as far as the recording
    /// is that long).
    pub fn clamped(self, duration: u32) -> Self {
        let min = MIN_CLIP_MS.min(duration);
        let mut start = self.start.min(duration);
        let mut end = self.end.min(duration);
        if end < start + min {
            end = (start + min).min(duration);
            start = start.min(end.saturating_sub(min));
        }
        Trim { start, end }
    }

    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// What the pointer, clicks and keys of a recording need: its event log and pointer pictures.
#[derive(Clone, Debug)]
pub struct Clip {
    pub log: Log,
    pub cursors: Cursors,
}

impl Clip {
    pub fn style(&self) -> Style {
        let zoom = self.log.header.zoom;
        Style { dpi: self.log.header.scale * zoom, zoom }
    }

    /// `frame` (at time `t` of the recording) with the overlays drawn on it.
    pub fn compose(&self, frame: &mut Nv12, t: u32, overlays: &Overlays, fonts: &Fonts) {
        if overlays.any() {
            let f = self.log.frame_at(t);
            compose::draw_nv12(frame, &f, &self.cursors, overlays, &self.style(), fonts);
        }
    }
}

/// What to make.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Job {
    pub trim: Trim,
    pub overlays: Overlays,
    /// Frames per second of the output.
    pub fps: u32,
}

/// Size of the output frames for a source of `w` x `h`.
pub fn output_size(w: u32, h: u32, max_width: Option<u32>) -> (u32, u32) {
    match max_width {
        Some(m) if w > m && m > 0 => (m, ((h as u64 * m as u64 + w as u64 / 2) / w as u64).max(1) as u32),
        _ => (w, h),
    }
}

/// Runs `job`: reads `src`, draws the overlays and feeds `sink`. `progress` gets 0..1 and
/// returns false to cancel. Returns the size of the output in bytes.
pub fn export(
    src: &mut dyn VideoSource,
    clip: &Clip,
    job: &Job,
    sink: &mut dyn FrameSink,
    fonts: &Fonts,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<u64, String> {
    let trim = job.trim.clamped(src.duration());
    let fps = job.fps.clamp(1, 60);
    src.seek(trim.start)?;
    // The frame shown now, and the one after it.
    let mut current: Option<(u32, Nv12)> = None;
    let mut ahead: Option<(u32, Nv12)> = None;
    let mut ended = false;
    let mut out = Nv12::default();
    let mut k: u64 = 0;
    loop {
        let out_t = (k * 1000 / fps as u64) as u32;
        let t = trim.start + out_t;
        if t >= trim.end {
            break;
        }
        // Read on until the next frame lies after `t`.
        loop {
            if ahead.is_none() && !ended {
                match src.next()? {
                    Some(f) => ahead = Some(f),
                    None => ended = true,
                }
            }
            match &ahead {
                Some((ft, _)) if *ft <= t || current.is_none() => current = ahead.take(),
                _ => break,
            }
        }
        let Some((_, frame)) = &current else { break };
        out.clone_from(frame);
        clip.compose(&mut out, t, &job.overlays, fonts);
        sink.push(&out, out_t)?;
        k += 1;
        if !progress(out_t as f32 / trim.len().max(1) as f32) {
            return Err(CANCELLED.into());
        }
    }
    let bytes = sink.finish(trim.len())?;
    progress(1.0);
    Ok(bytes)
}

// ---- size estimates ----------------------------------------------------------------------

/// Bitrate (bits per second) of an exported MP4: about 0.06 bits per pixel at 30 fps, a
/// little more per frame at lower frame rates. Screen content compresses well; text stays
/// sharp at this rate.
pub fn mp4_bitrate(w: u32, h: u32, fps: u32) -> u32 {
    let pixels = w as f64 * h as f64;
    let rate = pixels * 0.06 * 30.0 * (fps.max(1) as f64 / 30.0).sqrt();
    rate.clamp(400_000.0, 24_000_000.0) as u32
}

/// Bitrate of the clean master the recorder writes: higher, so exporting from it (a second
/// encode) loses little.
pub fn master_bitrate(w: u32, h: u32, fps: u32) -> u32 {
    (mp4_bitrate(w, h, fps) as u64 * 5 / 2).min(50_000_000) as u32
}

/// Expected size of an MP4 export in bytes: the bitrate over the length, plus the container.
pub fn estimate_mp4(w: u32, h: u32, fps: u32, ms: u32) -> u64 {
    mp4_bitrate(w, h, fps) as u64 * ms as u64 / 8000 + 4096
}

/// What a GIF of `job` would take: (bytes of its first frame with the header, bytes of the
/// rest), in one pass.
fn gif_bytes(
    src: &mut dyn VideoSource,
    clip: &Clip,
    job: &Job,
    max_width: Option<u32>,
    fonts: &Fonts,
    cancel: &dyn Fn() -> bool,
) -> Result<(u64, u64), String> {
    let mut sink = super::gif::GifSink::new(std::io::sink()).max_width(max_width);
    let total = export(src, clip, job, &mut sink, fonts, &mut |_| !cancel())?;
    let first = sink.first_frame_bytes().unwrap_or(total).min(total);
    Ok((first, total - first))
}

/// Expected size of a GIF export in bytes. Short clips are encoded whole (exact); for longer
/// ones two stretches of two seconds are encoded and the rest is extrapolated: the first
/// frame is whole, every later one only holds what changed, so the cost per second of the
/// later frames is what scales with the length.
pub fn estimate_gif(
    src: &mut dyn VideoSource,
    clip: &Clip,
    job: &Job,
    max_width: Option<u32>,
    fonts: &Fonts,
    cancel: &dyn Fn() -> bool,
) -> Result<u64, String> {
    const SAMPLE: u32 = 2000;
    let trim = job.trim.clamped(src.duration());
    let job = Job { trim, ..*job };
    if trim.len() <= 2 * SAMPLE + 1000 {
        let mut sink = super::gif::GifSink::new(std::io::sink()).max_width(max_width);
        return export(src, clip, &job, &mut sink, fonts, &mut |_| !cancel());
    }
    let mut firsts = Vec::new();
    let mut rest_bytes = 0u64;
    let mut rest_ms = 0u64;
    for frac in [0.2f64, 0.7] {
        let start = trim.start + ((trim.len() - SAMPLE) as f64 * frac) as u32;
        let part = Job { trim: Trim { start, end: start + SAMPLE }, ..job };
        let (first, rest) = gif_bytes(src, clip, &part, max_width, fonts, cancel)?;
        firsts.push(first);
        rest_bytes += rest;
        rest_ms += (SAMPLE - 1000 / job.fps.max(1)) as u64;
    }
    let first = firsts.iter().sum::<u64>() / firsts.len() as u64;
    let rest = rest_bytes as f64 / rest_ms.max(1) as f64 * (trim.len() as f64 - 1000.0 / job.fps.max(1) as f64);
    Ok(first + rest.max(0.0) as u64)
}

/// "3.4 MB", "820 KB": a size for people, with the decimal separator of the language.
pub fn human_size(bytes: u64) -> String {
    size_text(bytes, crate::i18n::language() != crate::i18n::Language::English)
}

fn size_text(bytes: u64, comma: bool) -> String {
    let (v, unit) =
        if bytes >= 1_000_000 { (bytes as f64 / 1_000_000.0, "MB") } else { ((bytes as f64 / 1000.0).max(1.0), "KB") };
    let s = if v < 10.0 && unit == "MB" { format!("{v:.1}") } else { format!("{v:.0}") };
    let s = if comma { s.replace('.', ",") } else { s };
    format!("{s} {unit}")
}

/// "0:12", "1:05", "1:02:03": a length for people.
pub fn human_duration(ms: u32) -> String {
    let s = (ms + 500) / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Frames in memory, for tests.
#[cfg(test)]
pub struct MemorySource {
    pub frames: Vec<(u32, Nv12)>,
    pub duration: u32,
    pub pos: usize,
}

#[cfg(test)]
impl VideoSource for MemorySource {
    fn duration(&self) -> u32 {
        self.duration
    }
    fn seek(&mut self, ms: u32) -> Result<(), String> {
        // Like a decoder that can only start at a key frame: a little before `ms`.
        let i = self.frames.partition_point(|f| f.0 <= ms).saturating_sub(1);
        self.pos = i.saturating_sub(2);
        Ok(())
    }
    fn next(&mut self) -> Result<Option<(u32, Nv12)>, String> {
        let f = self.frames.get(self.pos).cloned();
        self.pos += 1;
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rec::events::{Event, Header, Kind, VERSION};
    use crate::rec::keys::KeyMode;
    use image::RgbaImage;

    /// Frame `i` is filled with gray level `i` (so each output frame tells which source frame
    /// it came from) and has a square that moves.
    fn source(frames: u32, fps: u32) -> MemorySource {
        let frames = (0..frames)
            .map(|i| {
                let mut img = RgbaImage::from_pixel(320, 180, image::Rgba([i as u8, i as u8, i as u8, 255]));
                let x = (i * 6) % 280;
                for y in 60..100 {
                    for xx in x..x + 40 {
                        img.put_pixel(xx, y, image::Rgba([240, 200, 40, 255]));
                    }
                }
                (i * 1000 / fps, Nv12::from_rgba(&img))
            })
            .collect::<Vec<_>>();
        let duration = frames.len() as u32 * 1000 / fps;
        MemorySource { frames, duration, pos: 0 }
    }

    /// The index of the source frame `f` is.
    fn which(src: &MemorySource, f: &Nv12) -> usize {
        src.frames.iter().position(|(_, s)| s == f).expect("a source frame")
    }

    /// Like a screen: a detailed background that stands still, a pointer-sized square that
    /// moves, and every two seconds a "window" that opens or closes.
    fn screen_like(frames: u32, fps: u32) -> MemorySource {
        let bg = RgbaImage::from_fn(320, 180, |x, y| {
            let v = ((x * 7 + y * 13) % 64) as u8;
            image::Rgba([200 + v / 2, 205 + v / 3, 210, 255])
        });
        let frames = (0..frames)
            .map(|i| {
                let mut img = bg.clone();
                let t = i * 1000 / fps;
                if (t / 2000) % 2 == 1 {
                    for y in 40..140 {
                        for x in 60..260 {
                            img.put_pixel(x, y, image::Rgba([((x * y) % 255) as u8, 90, 160, 255]));
                        }
                    }
                }
                let (px, py) = ((i * 5) % 300, 20 + (i * 3) % 140);
                for y in py..py + 12 {
                    for x in px..px + 8 {
                        img.put_pixel(x, y, image::Rgba([20, 20, 20, 255]));
                    }
                }
                (t, Nv12::from_rgba(&img))
            })
            .collect::<Vec<_>>();
        let duration = frames.len() as u32 * 1000 / fps;
        MemorySource { frames, duration, pos: 0 }
    }

    fn clip(events: Vec<Event>) -> Clip {
        let header = Header { v: VERSION, w: 320, h: 180, fps: 30, scale: 1.0, zoom: 1.0, keys: KeyMode::Off };
        Clip { log: Log { header, events }, cursors: Cursors::default() }
    }

    struct Collect(Vec<(u32, Nv12)>, Option<u32>);
    impl FrameSink for Collect {
        fn push(&mut self, frame: &Nv12, t: u32) -> Result<(), String> {
            self.0.push((t, frame.clone()));
            Ok(())
        }
        fn finish(&mut self, end: u32) -> Result<u64, String> {
            self.1 = Some(end);
            Ok(self.0.len() as u64)
        }
    }

    fn job(start: u32, end: u32, fps: u32) -> Job {
        Job { trim: Trim { start, end }, overlays: Overlays::none(), fps }
    }

    fn run(src: &mut MemorySource, j: &Job) -> Collect {
        let mut out = Collect(Vec::new(), None);
        export(src, &clip(vec![]), j, &mut out, crate::capture_bar::fonts(), &mut |_| true).unwrap();
        out
    }

    #[test]
    fn trims_to_the_chosen_part() {
        let mut src = source(90, 30); // 3 s
        let out = run(&mut src, &job(1000, 2000, 30));
        assert_eq!(out.0.len(), 30);
        assert_eq!(out.1, Some(1000));
        // Times start at 0; the first frame is source frame 30 (at 1 s), the last 59.
        assert_eq!(out.0[0].0, 0);
        assert_eq!(which(&src, &out.0[0].1), 30);
        assert_eq!(which(&src, &out.0[29].1), 59);
    }

    #[test]
    fn changes_the_frame_rate_by_holding_or_skipping_frames() {
        let mut src = source(60, 30);
        let out = run(&mut src, &job(0, 2000, 12));
        assert_eq!(out.0.len(), 24);
        // Output frame k at k/12 s shows the last source frame at or before it.
        for (k, (t, f)) in out.0.iter().enumerate() {
            assert_eq!(*t, k as u32 * 1000 / 12);
            let want = (0..60u32).filter(|i| i * 1000 / 30 <= *t).last().unwrap() as usize;
            assert_eq!(which(&src, f), want, "frame {k}");
        }
        // Higher than the source: frames are held.
        let mut src = source(15, 15);
        let out = run(&mut src, &job(0, 1000, 30));
        assert_eq!(out.0.len(), 30);
        assert_eq!(out.0[0].1, out.0[1].1);
    }

    #[test]
    fn trims_are_kept_sensible() {
        assert_eq!(Trim { start: 500, end: 9000 }.clamped(3000), Trim { start: 500, end: 3000 });
        assert_eq!(Trim { start: 3000, end: 3000 }.clamped(3000), Trim { start: 3000 - MIN_CLIP_MS, end: 3000 });
        assert_eq!(Trim { start: 100, end: 150 }.clamped(3000), Trim { start: 100, end: 100 + MIN_CLIP_MS });
        assert_eq!(Trim { start: 0, end: 50 }.clamped(100), Trim { start: 0, end: 100 });
        assert_eq!(Trim::whole(1234).len(), 1234);
    }

    #[test]
    fn gifs_are_scaled_down_to_the_maximum_width() {
        assert_eq!(output_size(1920, 1080, Some(960)), (960, 540));
        assert_eq!(output_size(800, 600, Some(960)), (800, 600));
        assert_eq!(output_size(1000, 3, Some(10)), (10, 1));
        let mut src = source(10, 30);
        let mut sink = crate::rec::gif::GifSink::new(Vec::new()).max_width(Some(160));
        export(&mut src, &clip(vec![]), &job(0, 300, 30), &mut sink, crate::capture_bar::fonts(), &mut |_| true)
            .unwrap();
        let data = sink.into_output().unwrap();
        let dec = ::gif::DecodeOptions::new().read_info(&data[..]).unwrap();
        assert_eq!((dec.width(), dec.height()), (160, 90));
    }

    #[test]
    fn draws_the_overlays_at_the_output_time() {
        let events = vec![Event {
            t: 500,
            kind: Kind::Down { b: crate::rec::events::Button::Left, x: 160, y: 20, mods: Default::default() },
        }];
        let mut src = source(60, 30);
        let mut out = Collect(Vec::new(), None);
        let j = Job { overlays: Overlays::default(), ..job(0, 2000, 10) };
        export(&mut src, &clip(events), &j, &mut out, crate::capture_bar::fonts(), &mut |_| true).unwrap();
        // Output frame i (at i/10 s) is source frame 3i, with or without a ring on it.
        let ring_at =
            |i: usize| out.0[i].1.region_to_rgba(160, 20, 2, 2) != src.frames[i * 3].1.region_to_rgba(160, 20, 2, 2);
        assert!(!ring_at(4), "before the click");
        assert!(ring_at(5) && ring_at(6), "during the click");
        assert!(!ring_at(10), "after it");
    }

    #[test]
    fn can_be_cancelled() {
        let mut src = source(60, 30);
        let mut out = Collect(Vec::new(), None);
        let mut n = 0;
        let r = export(&mut src, &clip(vec![]), &job(0, 2000, 30), &mut out, crate::capture_bar::fonts(), &mut |_| {
            n += 1;
            n < 5
        });
        assert_eq!(r, Err(CANCELLED.to_string()));
        assert_eq!(out.1, None, "not finished");
    }

    #[test]
    fn gif_estimate_is_close_to_the_real_size() {
        let fonts = crate::capture_bar::fonts();
        // 20 s at 15 fps: long enough to be sampled.
        let mut src = screen_like(300, 15);
        let j = Job { trim: Trim::whole(20_000), overlays: Overlays::none(), fps: 12 };
        let w = Some(320);
        let estimate = estimate_gif(&mut src, &clip(vec![]), &j, w, fonts, &|| false).unwrap();
        let mut sink = crate::rec::gif::GifSink::new(Vec::new()).max_width(w);
        let real = export(&mut src, &clip(vec![]), &j, &mut sink, fonts, &mut |_| true).unwrap();
        let ratio = estimate as f64 / real as f64;
        assert!((0.75..=1.33).contains(&ratio), "estimate {estimate}, real {real}");
        // A short clip is encoded whole: exact.
        let short = Job { trim: Trim { start: 0, end: 3000 }, ..j };
        let estimate = estimate_gif(&mut src, &clip(vec![]), &short, w, fonts, &|| false).unwrap();
        let mut sink = crate::rec::gif::GifSink::new(Vec::new()).max_width(w);
        let real = export(&mut src, &clip(vec![]), &short, &mut sink, fonts, &mut |_| true).unwrap();
        assert_eq!(estimate, real);
    }

    #[test]
    fn mp4_sizes_follow_the_bitrate() {
        let r = mp4_bitrate(1920, 1080, 30);
        assert!((3_000_000..=4_500_000).contains(&r), "{r}");
        assert!(mp4_bitrate(1920, 1080, 15) < r);
        assert!(master_bitrate(1920, 1080, 30) > r * 2);
        assert_eq!(mp4_bitrate(10, 10, 30), 400_000, "a floor for tiny videos");
        // 10 s of 1080p30: about 4.7 MB.
        let s = estimate_mp4(1920, 1080, 30, 10_000);
        assert!((4_000_000..=5_500_000).contains(&s), "{s}");
    }

    #[test]
    fn sizes_and_lengths_for_people() {
        assert_eq!(size_text(3_400_000, false), "3.4 MB");
        assert_eq!(size_text(3_400_000, true), "3,4 MB");
        assert_eq!(size_text(12_345_678, false), "12 MB");
        assert_eq!(size_text(820_000, false), "820 KB");
        assert_eq!(size_text(10, false), "1 KB");
        assert_eq!(human_duration(12_400), "0:12");
        assert_eq!(human_duration(65_000), "1:05");
        assert_eq!(human_duration(3_723_000), "1:02:03");
    }
}
