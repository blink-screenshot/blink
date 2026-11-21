//! Video frames in NV12, the format H.264 encoders and decoders work in: a full-size plane
//! of brightness (Y) and a half-size plane of color (U and V interleaved, one pair per 2 x 2
//! pixels).
//!
//! Blink converts the screen to NV12 itself, on every CPU core, instead of handing the
//! encoder RGB: Windows' own converter does that on one core, which at 4K takes longer than
//! a frame lasts. Decoding stays in NV12 too, and the pointer, clicks and keys are drawn by
//! converting only the small areas they cover to RGB and back. A whole frame is only
//! converted to RGB where it has to be shown or put in a GIF, and then straight to the
//! smaller size.
//!
//! Colors follow BT.709 with the studio range (16-235), as HD video does.

use image::RgbaImage;

/// One frame. Width and height are even.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Nv12 {
    pub w: u32,
    pub h: u32,
    /// `w * h` bytes.
    pub y: Vec<u8>,
    /// `w * h / 2` bytes: U, V, U, V, ... for every 2 x 2 block.
    pub uv: Vec<u8>,
}

fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn luma(r: i32, g: i32, b: i32) -> u8 {
    clamp8(16 + ((47 * r + 157 * g + 16 * b + 128) >> 8))
}

fn chroma(r: i32, g: i32, b: i32) -> (u8, u8) {
    (clamp8(128 + ((-26 * r - 87 * g + 112 * b + 128) >> 8)), clamp8(128 + ((112 * r - 102 * g - 10 * b + 128) >> 8)))
}

fn rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let c = 298 * (y as i32 - 16);
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    [clamp8((c + 459 * e + 128) >> 8), clamp8((c - 55 * d - 136 * e + 128) >> 8), clamp8((c + 541 * d + 128) >> 8)]
}

/// How many threads to split a frame of `pixels` over.
fn threads(pixels: usize) -> usize {
    if pixels < 300_000 {
        1
    } else {
        std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8)
    }
}

/// Runs `f(first_pair, y_rows, uv_rows)` for bands of row pairs of an NV12 frame `w` wide, in
/// parallel. `y_rows` holds two image rows per pair, `uv_rows` one.
fn bands(y: &mut [u8], uv: &mut [u8], w: usize, pairs: usize, f: impl Fn(usize, &mut [u8], &mut [u8]) + Sync) {
    let n = threads(w * pairs * 2);
    let per = pairs.div_ceil(n).max(1);
    std::thread::scope(|s| {
        let mut ys = y.chunks_mut(per * 2 * w);
        let mut uvs = uv.chunks_mut(per * w);
        let mut first = 0;
        while let (Some(yb), Some(uvb)) = (ys.next(), uvs.next()) {
            let f = &f;
            let start = first;
            if n == 1 {
                f(start, yb, uvb);
            } else {
                s.spawn(move || f(start, yb, uvb));
            }
            first += per;
        }
    });
}

impl Nv12 {
    fn resize(&mut self, w: u32, h: u32) {
        self.w = w;
        self.h = h;
        self.y.resize((w * h) as usize, 16);
        self.uv.resize((w * h / 2) as usize, 128);
    }

    /// Fills this frame from BGRA rows (`stride` bytes apart), using the even part of `w` x `h`.
    pub fn set_from_bgra(&mut self, src: &[u8], stride: usize, w: u32, h: u32) {
        self.from_rgb_rows(src, stride, w, h, [2, 1, 0]);
    }

    /// From RGBA (for tests and frames made in memory).
    #[cfg(test)]
    pub fn from_rgba(img: &RgbaImage) -> Self {
        let mut n = Nv12::default();
        n.from_rgb_rows(img.as_raw(), img.width() as usize * 4, img.width(), img.height(), [0, 1, 2]);
        n
    }

    fn from_rgb_rows(&mut self, src: &[u8], stride: usize, w: u32, h: u32, [ri, gi, bi]: [usize; 3]) {
        let (w, h) = (w & !1, h & !1);
        self.resize(w, h);
        let wu = w as usize;
        bands(&mut self.y, &mut self.uv, wu, h as usize / 2, |first, yb, uvb| {
            for (i, uvrow) in uvb.chunks_mut(wu).enumerate() {
                let row = (first + i) * 2;
                let (top, bottom) = (&src[row * stride..], &src[(row + 1) * stride..]);
                let (y0, y1) = yb[i * 2 * wu..(i * 2 + 2) * wu].split_at_mut(wu);
                for x in 0..wu / 2 {
                    let mut sum = [0i32; 3];
                    let mut one = |yrow: &mut [u8], srow: &[u8]| {
                        for dx in 0..2 {
                            let p = &srow[(x * 2 + dx) * 4..];
                            let (r, g, b) = (p[ri] as i32, p[gi] as i32, p[bi] as i32);
                            yrow[x * 2 + dx] = luma(r, g, b);
                            sum[0] += r;
                            sum[1] += g;
                            sum[2] += b;
                        }
                    };
                    one(y0, top);
                    one(y1, bottom);
                    let (u, v) = chroma((sum[0] + 2) >> 2, (sum[1] + 2) >> 2, (sum[2] + 2) >> 2);
                    uvrow[x * 2] = u;
                    uvrow[x * 2 + 1] = v;
                }
            }
        });
    }

    /// The area `(x, y, w, h)` (even numbers, inside the frame) as RGBA.
    pub fn region_to_rgba(&self, x: u32, y: u32, w: u32, h: u32) -> RgbaImage {
        let mut out = vec![0u8; (w * h * 4) as usize];
        let fw = self.w as usize;
        for row in 0..h as usize {
            let sy = y as usize + row;
            for col in 0..w as usize {
                let sx = x as usize + col;
                let c = (sy / 2) * fw + (sx & !1);
                let p = rgb(self.y[sy * fw + sx], self.uv[c], self.uv[c + 1]);
                let o = (row * w as usize + col) * 4;
                out[o..o + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        RgbaImage::from_raw(w, h, out).expect("size matches")
    }

    /// Writes `img` (even size) back at `(x, y)` (even).
    pub fn put_rgba(&mut self, img: &RgbaImage, x: u32, y: u32) {
        let fw = self.w as usize;
        let (w, h) = (img.width() as usize & !1, img.height() as usize & !1);
        let raw = img.as_raw();
        let iw = img.width() as usize;
        for r in (0..h).step_by(2) {
            for c in (0..w).step_by(2) {
                let mut sum = [0i32; 3];
                for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let p = &raw[((r + dy) * iw + c + dx) * 4..];
                    let (rr, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
                    self.y[(y as usize + r + dy) * fw + x as usize + c + dx] = luma(rr, g, b);
                    sum[0] += rr;
                    sum[1] += g;
                    sum[2] += b;
                }
                let (u, v) = chroma((sum[0] + 2) >> 2, (sum[1] + 2) >> 2, (sum[2] + 2) >> 2);
                let o = ((y as usize + r) / 2) * fw + x as usize + c;
                self.uv[o] = u;
                self.uv[o + 1] = v;
            }
        }
    }

    /// The whole frame as RGBA at `ow` x `oh` (at most the frame's size), each output pixel the
    /// average of the area it covers, so text stays readable when it gets smaller.
    pub fn to_rgba_scaled(&self, ow: u32, oh: u32) -> RgbaImage {
        let (ow, oh) = (ow.clamp(1, self.w.max(1)), oh.clamp(1, self.h.max(1)));
        let (fw, fh) = (self.w as usize, self.h as usize);
        let (owu, ohu) = (ow as usize, oh as usize);
        let span = |i: usize, out: usize, full: usize| (i * full / out, ((i + 1) * full / out).max(i * full / out + 1));
        let mut out = vec![0u8; owu * ohu * 4];
        let n = threads(fw * fh);
        let per = ohu.div_ceil(n).max(1);
        std::thread::scope(|s| {
            for (band, chunk) in out.chunks_mut(per * owu * 4).enumerate() {
                let mut work = move || {
                    for (i, row) in chunk.chunks_mut(owu * 4).enumerate() {
                        let oy = band * per + i;
                        let (y0, y1) = span(oy, ohu, fh);
                        for ox in 0..owu {
                            let (x0, x1) = span(ox, owu, fw);
                            let (mut sy, mut su, mut sv, mut ny, mut nc) = (0u32, 0u32, 0u32, 0u32, 0u32);
                            for yy in y0..y1 {
                                for xx in x0..x1 {
                                    sy += self.y[yy * fw + xx] as u32;
                                    ny += 1;
                                }
                            }
                            for cy in y0 / 2..=(y1 - 1) / 2 {
                                for cx in x0 / 2..=(x1 - 1) / 2 {
                                    let c = cy * fw + cx * 2;
                                    su += self.uv[c] as u32;
                                    sv += self.uv[c + 1] as u32;
                                    nc += 1;
                                }
                            }
                            let p = rgb((sy / ny) as u8, (su / nc) as u8, (sv / nc) as u8);
                            row[ox * 4..ox * 4 + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
                        }
                    }
                };
                if n == 1 {
                    work();
                } else {
                    s.spawn(work);
                }
            }
        });
        RgbaImage::from_raw(ow, oh, out).expect("size matches")
    }

    #[cfg(test)]
    pub fn to_rgba(&self) -> RgbaImage {
        self.to_rgba_scaled(self.w, self.h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[u8], b: &[u8], tol: i32) -> bool {
        a.iter().zip(b).all(|(x, y)| (*x as i32 - *y as i32).abs() <= tol)
    }

    fn sample() -> RgbaImage {
        RgbaImage::from_fn(64, 48, |x, y| {
            if (x / 8 + y / 8) % 2 == 0 {
                image::Rgba([(x * 4) as u8, (y * 5) as u8, 200, 255])
            } else {
                image::Rgba([250, 250, 250, 255])
            }
        })
    }

    #[test]
    fn colors_survive_the_round_trip() {
        for c in [[0u8, 0, 0], [255, 255, 255], [255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 46, 147], [90, 90, 90]] {
            let img = RgbaImage::from_pixel(4, 4, image::Rgba([c[0], c[1], c[2], 255]));
            let back = Nv12::from_rgba(&img).to_rgba();
            assert!(close(&back.get_pixel(1, 1).0[..3], &c, 3), "{c:?} -> {:?}", back.get_pixel(1, 1));
        }
        // Black and white land on the studio range.
        let n = Nv12::from_rgba(&RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255])));
        assert_eq!(n.y[0], 16);
        let n = Nv12::from_rgba(&RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 255])));
        assert_eq!(n.y[0], 235);
    }

    #[test]
    fn bgra_and_rgba_give_the_same_frame() {
        let img = sample();
        let bgra: Vec<u8> = img.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let mut n = Nv12::default();
        n.set_from_bgra(&bgra, 64 * 4, 64, 48);
        assert_eq!(n, Nv12::from_rgba(&img));
        // Odd sizes lose the last column and row.
        let mut odd = Nv12::default();
        odd.set_from_bgra(&bgra, 64 * 4, 63, 47);
        assert_eq!((odd.w, odd.h), (62, 46));
    }

    #[test]
    fn large_frames_convert_in_parallel_the_same() {
        // Big enough to be split over threads.
        let img = RgbaImage::from_fn(1280, 720, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
        });
        let n = Nv12::from_rgba(&img);
        // Spot checks against converting single 2 x 2 blocks on their own.
        for (x, y) in [(0, 0), (640, 360), (1278, 718), (2, 500)] {
            let block = image::imageops::crop_imm(&img, x, y, 2, 2).to_image();
            let b = Nv12::from_rgba(&block);
            assert_eq!(b.y[0], n.y[(y * 1280 + x) as usize]);
            assert_eq!(b.uv[0], n.uv[((y / 2) * 1280 + x) as usize]);
        }
    }

    #[test]
    fn regions_go_out_and_back() {
        let mut n = Nv12::from_rgba(&sample());
        let before = n.clone();
        let mut part = n.region_to_rgba(8, 6, 20, 14);
        assert_eq!(part.dimensions(), (20, 14));
        // Unchanged it goes back (nearly) the same.
        n.put_rgba(&part, 8, 6);
        assert!(close(&n.y, &before.y, 2) && close(&n.uv, &before.uv, 3));
        // A red square drawn in it shows up in the frame, and only there.
        for y in 4..8 {
            for x in 4..8 {
                part.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
            }
        }
        n.put_rgba(&part, 8, 6);
        let rgba = n.to_rgba();
        assert!(close(&rgba.get_pixel(14, 12).0[..3], &[255, 0, 0], 4));
        assert!(close(&rgba.get_pixel(40, 40).0[..3], &before.to_rgba().get_pixel(40, 40).0[..3], 0));
    }

    #[test]
    fn scales_down_by_averaging() {
        // Vertical black and white lines, one pixel each: half size is gray.
        let img = RgbaImage::from_fn(64, 32, |x, _| {
            if x % 2 == 0 {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        let small = Nv12::from_rgba(&img).to_rgba_scaled(32, 16);
        assert_eq!(small.dimensions(), (32, 16));
        let p = small.get_pixel(10, 5);
        assert!((120..=136).contains(&p[0]), "{p:?}");
        // Full size is the plain conversion; larger than the frame is not possible.
        let n = Nv12::from_rgba(&sample());
        assert_eq!(n.to_rgba_scaled(64, 48), n.to_rgba());
        assert_eq!(n.to_rgba_scaled(500, 500).dimensions(), (64, 48));
        assert_eq!(n.to_rgba_scaled(33, 25).dimensions(), (33, 25));
    }
}
