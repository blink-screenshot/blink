//! Stitching of scrolling-capture frames into one tall image.
//!
//! Each new frame is compared with the previous one to find how far the content scrolled.
//! Rows are compared by a coarse signature (the average brightness of segments across the
//! row) with a tolerance, not bit for bit: at 125% / 150% display scaling, and in browsers
//! that scroll by fractional pixels, the same content is re-rasterized slightly differently
//! after every scroll, so exact comparison finds no overlap at all. Rows that stay in place
//! at the top (sticky headers) or bottom (sticky footers) are detected and not repeated.

use image::RgbaImage;

/// Segments per row signature.
const SEGMENTS: usize = 64;
/// Mean brightness difference (0..255) below which two rows count as the same.
const ROW_TOLERANCE: u32 = 4;
/// Brightness range within a row above which it carries enough detail to match on.
const DETAIL: u8 = 6;
/// Share of detailed rows that must line up for an overlap to be accepted.
const MIN_MATCH: f64 = 0.8;

struct Frame {
    img: RgbaImage,
    /// `SEGMENTS` brightness values per row.
    sig: Vec<u8>,
    informative: Vec<bool>,
}

impl Frame {
    fn new(img: RgbaImage) -> Self {
        let w = img.width() as usize;
        // Ignore the right edge: the scrollbar thumb moves on every step.
        let margin = (w / 8).min(24);
        let used = (w - margin).max(1);
        let seg = SEGMENTS.min(used);
        let h = img.height() as usize;
        let mut sig = vec![0u8; h * SEGMENTS];
        let mut informative = Vec::with_capacity(h);
        for (y, row) in img.as_raw().chunks_exact(w * 4).enumerate() {
            let out = &mut sig[y * SEGMENTS..(y + 1) * SEGMENTS];
            for (k, o) in out.iter_mut().enumerate().take(seg) {
                let (x0, x1) = (k * used / seg, ((k + 1) * used / seg).max(k * used / seg + 1));
                let mut sum = 0u32;
                for p in row[x0 * 4..x1 * 4].chunks_exact(4) {
                    sum += p[0] as u32 + p[1] as u32 + p[2] as u32;
                }
                *o = (sum / (3 * (x1 - x0) as u32)) as u8;
            }
            let (lo, hi) = out[..seg].iter().fold((255u8, 0u8), |(lo, hi), &v| (lo.min(v), hi.max(v)));
            // A row with fine detail (thin text) can still average out flat: check pixels too.
            let first = &row[..4];
            let pixel_detail = row[..used * 4]
                .chunks_exact(4)
                .any(|p| p.iter().zip(first).take(3).any(|(a, b)| a.abs_diff(*b) > DETAIL * 3));
            informative.push(hi - lo > DETAIL || pixel_detail);
        }
        Self { img, sig, informative }
    }
    fn h(&self) -> usize {
        self.informative.len()
    }
    fn row(&self, i: usize) -> &[u8] {
        &self.sig[i * SEGMENTS..(i + 1) * SEGMENTS]
    }
}

fn row_distance(a: &[u8], b: &[u8]) -> u32 {
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u32).sum::<u32>() / SEGMENTS as u32
}

fn rows_match(a: &Frame, i: usize, b: &Frame, j: usize) -> bool {
    row_distance(a.row(i), b.row(j)) <= ROW_TOLERANCE
}

/// True when two grabs show (nearly) the same picture: used to wait until scrolling settled.
pub fn nearly_equal(a: &RgbaImage, b: &RgbaImage) -> bool {
    if a.dimensions() != b.dimensions() {
        return false;
    }
    let (fa, fb) = (Frame::new(a.clone()), Frame::new(b.clone()));
    let differ = (0..fa.h()).filter(|&i| row_distance(fa.row(i), fb.row(i)) > 1).count();
    differ * 100 <= fa.h()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// New rows were added; value is the detected scroll distance in pixels.
    Appended(usize),
    /// Frame identical to the previous one: the end of the page was reached.
    NoChange,
    /// No reliable overlap found (content changed too much or scrolled too far).
    Mismatch,
    /// Maximum height reached.
    Full,
}

pub struct Stitcher {
    width: u32,
    out: Vec<u8>,
    rows: usize,
    prev: Option<Frame>,
    max_height: usize,
}

impl Stitcher {
    pub fn new(max_height: u32) -> Self {
        Self { width: 0, out: Vec::new(), rows: 0, prev: None, max_height: max_height.max(1) as usize }
    }

    /// `hint`: expected scroll distance, used when the overlap is ambiguous (blank areas).
    pub fn push(&mut self, img: RgbaImage, hint: Option<usize>) -> Step {
        let next = Frame::new(img);
        let Some(prev) = &self.prev else {
            self.width = next.img.width();
            self.out = next.img.as_raw().clone();
            self.rows = next.h();
            self.prev = Some(next);
            return Step::Appended(0);
        };
        if next.img.dimensions() != prev.img.dimensions() {
            return Step::Mismatch;
        }
        let h = next.h();
        let same_at = |i: usize| rows_match(&next, i, prev, i);
        if (0..h).all(same_at) {
            return Step::NoChange;
        }
        let fixed_top = (0..h / 2).take_while(|&i| same_at(i)).count();
        let fixed_bottom = (0..h / 3).take_while(|&j| same_at(h - 1 - j)).count();
        let lo = fixed_top;
        let hi = h - fixed_bottom; // exclusive
        if hi <= lo + 1 {
            return Step::Mismatch;
        }

        // Detailed rows in next[lo..k], to know each shift's budget of mismatches up front.
        let mut detailed = vec![0usize; h + 1];
        for i in 0..h {
            detailed[i + 1] = detailed[i] + next.informative[i] as usize;
        }

        // Score every candidate shift s: next[i] ~ prev[i + s].
        // (s, matches, considered, total distance)
        let mut best: Option<(usize, usize, usize, u64)> = None;
        for s in 1..(hi - lo) {
            let considered = detailed[hi - s] - detailed[lo];
            let allowed_misses = considered - (considered as f64 * MIN_MATCH).ceil() as usize;
            let mut matches = 0;
            let mut misses = 0;
            let mut all_equal = true;
            let mut dist: u64 = 0;
            for i in lo..(hi - s) {
                let d = row_distance(next.row(i), prev.row(i + s));
                let eq = d <= ROW_TOLERANCE;
                all_equal &= eq;
                if next.informative[i] {
                    dist += d as u64;
                    if eq {
                        matches += 1;
                    } else {
                        misses += 1;
                        if misses > allowed_misses {
                            break;
                        }
                    }
                }
            }
            let ok = considered > 0 && misses <= allowed_misses && matches >= 3.min(considered);
            let blank_ok = considered == 0 && all_equal && Some(s) == hint;
            if !(ok || blank_ok) {
                continue;
            }
            let better = match best {
                None => true,
                Some((bs, bm, bc, bd)) => {
                    let ratio = |m: usize, c: usize| if c == 0 { 0.0 } else { m as f64 / c as f64 };
                    let (r, br) = (ratio(matches, considered), ratio(bm, bc));
                    let (avg, bavg) = (dist as f64 / considered.max(1) as f64, bd as f64 / bc.max(1) as f64);
                    if (r - br).abs() > 0.02 {
                        r > br
                    } else if (avg - bavg).abs() > 0.25 {
                        avg < bavg
                    } else if matches.abs_diff(bm) * 10 > bm {
                        // Much more overlap is stronger evidence.
                        matches > bm
                    } else {
                        // Tie (repeating content): prefer the expected distance.
                        match hint {
                            Some(hs) => s.abs_diff(hs) < bs.abs_diff(hs),
                            None => false,
                        }
                    }
                }
            };
            if better {
                best = Some((s, matches, considered, dist));
            }
        }
        let Some((s, _, _, _)) = best else {
            return Step::Mismatch;
        };

        // Drop the previous footer, append the newly revealed rows plus the footer.
        let stride = self.width as usize * 4;
        self.rows -= fixed_bottom;
        self.out.truncate(self.rows * stride);
        let start = hi - s;
        let mut add = h - start;
        let room = self.max_height.saturating_sub(self.rows);
        let full = add > room;
        add = add.min(room);
        self.out.extend_from_slice(&next.img.as_raw()[start * stride..(start + add) * stride]);
        self.rows += add;
        self.prev = Some(next);
        if full {
            Step::Full
        } else {
            Step::Appended(s)
        }
    }

    pub fn finish(self) -> Option<RgbaImage> {
        if self.rows == 0 {
            return None;
        }
        RgbaImage::from_raw(self.width, self.rows as u32, self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(w: u32, h: u32) -> RgbaImage {
        let mut seed = 12345u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        RgbaImage::from_fn(w, h, |_, y| {
            // A blank band in the middle to exercise ambiguity handling.
            if (400..460).contains(&y) {
                image::Rgba([255, 255, 255, 255])
            } else {
                let v = rnd();
                image::Rgba([v as u8, (v >> 8) as u8, (v >> 16) as u8, 255])
            }
        })
    }

    fn view(content: &RgbaImage, off: u32, header: &RgbaImage, footer: &RgbaImage, vh: u32) -> RgbaImage {
        let w = content.width();
        let ch = vh - header.height() - footer.height();
        RgbaImage::from_fn(w, vh, |x, y| {
            if y < header.height() {
                *header.get_pixel(x, y)
            } else if y < header.height() + ch {
                *content.get_pixel(x, off + y - header.height())
            } else {
                *footer.get_pixel(x, y - header.height() - ch)
            }
        })
    }

    #[test]
    fn stitches_with_sticky_header_and_footer() {
        let w = 120;
        let content = page(w, 2000);
        let header = RgbaImage::from_fn(w, 40, |x, y| image::Rgba([x as u8, y as u8, 7, 255]));
        let footer = RgbaImage::from_fn(w, 25, |x, y| image::Rgba([y as u8, x as u8, 99, 255]));
        let vh = 300;
        let ch = vh - 40 - 25;
        let mut st = Stitcher::new(100_000);
        let mut off = 0;
        let step = 130;
        loop {
            let r = st.push(view(&content, off, &header, &footer, vh), Some(step as usize));
            assert!(matches!(r, Step::Appended(_)), "{r:?} at {off}");
            if off == 2000 - ch {
                break;
            }
            off = (off + step).min(2000 - ch);
        }
        assert_eq!(st.push(view(&content, off, &header, &footer, vh), None), Step::NoChange);
        let out = st.finish().unwrap();
        let expected = RgbaImage::from_fn(w, 40 + 2000 + 25, |x, y| {
            if y < 40 {
                *header.get_pixel(x, y)
            } else if y < 2040 {
                *content.get_pixel(x, y - 40)
            } else {
                *footer.get_pixel(x, y - 2040)
            }
        });
        assert_eq!(out.dimensions(), expected.dimensions());
        assert!(out == expected);
    }

    #[test]
    fn respects_max_height() {
        let content = page(50, 2000);
        let mut st = Stitcher::new(500);
        let crop = |off| image::imageops::crop_imm(&content, 0, off, 50, 300).to_image();
        assert_eq!(st.push(crop(0), None), Step::Appended(0));
        assert_eq!(st.push(crop(150), None), Step::Appended(150));
        assert_eq!(st.push(crop(300), None), Step::Full);
        assert_eq!(st.finish().unwrap().height(), 500);
    }

    /// Every frame re-rasterized slightly differently (as at 125% / 150% scaling): exact
    /// comparison finds nothing, tolerant comparison must still stitch the whole page.
    #[test]
    fn stitches_noisy_frames() {
        let w = 160;
        let content = page(w, 1500);
        let mut seed = 99u32;
        let mut noisy = |img: RgbaImage| {
            RgbaImage::from_fn(img.width(), img.height(), |x, y| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let n = (seed % 7) as i32 - 3;
                let p = img.get_pixel(x, y).0;
                let c = |v: u8| (v as i32 + n).clamp(0, 255) as u8;
                image::Rgba([c(p[0]), c(p[1]), c(p[2]), 255])
            })
        };
        let vh = 280;
        let mut st = Stitcher::new(100_000);
        let mut off = 0;
        loop {
            let frame = noisy(image::imageops::crop_imm(&content, 0, off, w, vh).to_image());
            let r = st.push(frame, None);
            assert!(matches!(r, Step::Appended(_)), "{r:?} at {off}");
            if off == 1500 - vh {
                break;
            }
            off = (off + 150).min(1500 - vh);
        }
        assert_eq!(st.finish().unwrap().height(), 1500);
    }

    #[test]
    fn nearly_equal_ignores_noise_but_not_scrolling() {
        let content = page(100, 600);
        let a = image::imageops::crop_imm(&content, 0, 0, 100, 200).to_image();
        let mut b = a.clone();
        b.put_pixel(3, 3, image::Rgba([0, 0, 0, 255]));
        assert!(nearly_equal(&a, &b));
        let c = image::imageops::crop_imm(&content, 0, 40, 100, 200).to_image();
        assert!(!nearly_equal(&a, &c));
    }
}
