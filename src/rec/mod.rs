//! Screen recording: the portable part. What is recorded, how the pointer, clicks and keys
//! are drawn back in, trimming and the GIF encoder live here and are tested on any platform;
//! capturing the screen, the keyboard and mouse hooks and the H.264 encoder are in
//! `platform/win/rec`.

pub mod bar;
pub mod compose;
pub mod events;
pub mod export;
pub mod gif;
pub mod keys;
pub mod store;
pub mod yuv;

use crate::i18n::t;

/// Largest frame the H.264 encoders take (the Microsoft encoder and most hardware ones).
pub const MAX_W: u32 = 4096;
pub const MAX_H: u32 = 2304;

/// What to tell someone whose Windows has no Media Foundation (an N edition).
pub fn no_media_foundation() -> &'static str {
    t("Screen recording needs the Media Feature Pack, which this edition of Windows (an N edition) does not include. Install it in Settings > Apps > Optional features > Add a feature > Media Feature Pack, restart, and try again.")
}

/// How much an area of `w` x `h` is scaled down to fit the encoder: 1, 2, 4, ...
#[cfg(test)]
pub fn shrink_factor(w: u32, h: u32) -> u32 {
    shrink_factor_from(w, h, 1)
}

/// `shrink_factor`, starting at `start` (2: half size on a sharp screen).
pub fn shrink_factor_from(w: u32, h: u32, start: u32) -> u32 {
    let mut f = start.max(1);
    while w / f > MAX_W || h / f > MAX_H {
        f *= 2;
    }
    f
}

/// `src` (BGRA rows of `w` pixels, `h` rows) scaled down by `f` (averaging `f` x `f` blocks)
/// into `out`, on every core. Returns the new size.
pub fn shrink_bgra(src: &[u8], w: u32, h: u32, f: u32, out: &mut Vec<u8>) -> (u32, u32) {
    let (nw, nh) = ((w / f) as usize, (h / f) as usize);
    let (w, f) = (w as usize, f as usize);
    out.clear();
    out.resize(nw * nh * 4, 0);
    let n = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let per = nh.div_ceil(n).max(1);
    let shift = (f * f).trailing_zeros();
    std::thread::scope(|s| {
        for (band, chunk) in out.chunks_mut(per * nw * 4).enumerate() {
            s.spawn(move || {
                for (i, row) in chunk.chunks_mut(nw * 4).enumerate() {
                    let y = band * per + i;
                    for x in 0..nw {
                        let mut sum = [0u32; 4];
                        for dy in 0..f {
                            let start = ((y * f + dy) * w + x * f) * 4;
                            for p in src[start..start + f * 4].chunks_exact(4) {
                                for c in 0..4 {
                                    sum[c] += p[c] as u32;
                                }
                            }
                        }
                        for c in 0..4 {
                            row[x * 4 + c] = (sum[c] >> shift) as u8;
                        }
                    }
                }
            });
        }
    });
    (nw as u32, nh as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_areas_are_scaled_to_fit_the_encoder() {
        assert_eq!(shrink_factor(1920, 1080), 1);
        assert_eq!(shrink_factor(4096, 2304), 1);
        assert_eq!(shrink_factor(5120, 1440), 2);
        assert_eq!(shrink_factor(7680, 4320), 2);
        assert_eq!(shrink_factor(10240, 2880), 4);
        // Half size on a sharp screen, smaller still if the encoder needs it.
        assert_eq!(shrink_factor_from(3840, 2160, 2), 2);
        assert_eq!(shrink_factor_from(10240, 4320, 2), 4);
        // 4 x 2 pixels to 2 x 1: each output pixel is the average of a 2 x 2 block.
        let px = |v: u8| [v, v, v, 255];
        let src: Vec<u8> = [px(0), px(100), px(10), px(10), px(200), px(100), px(30), px(50)].concat();
        let mut out = Vec::new();
        assert_eq!(shrink_bgra(&src, 4, 2, 2, &mut out), (2, 1));
        assert_eq!(out, [px(100), px(25)].concat());
    }
}
