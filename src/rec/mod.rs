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

use crate::i18n::t;

/// Largest frame the H.264 encoders take (the Microsoft encoder and most hardware ones).
pub const MAX_W: u32 = 4096;
pub const MAX_H: u32 = 2304;

/// How much an area of `w` x `h` is scaled down to fit the encoder: 1, 2, 4, ...
pub fn shrink_factor(w: u32, h: u32) -> u32 {
    let mut f = 1;
    while w / f > MAX_W || h / f > MAX_H {
        f *= 2;
    }
    f
}

/// `src` (BGRA rows of `w` pixels, `h` rows) scaled down by `f` (averaging `f` x `f` blocks)
/// into `out`. Returns the new size. Fast enough for every frame of an ultrawide screen.
pub fn shrink_bgra(src: &[u8], w: u32, h: u32, f: u32, out: &mut Vec<u8>) -> (u32, u32) {
    let (nw, nh) = (w / f, h / f);
    out.clear();
    out.reserve((nw * nh * 4) as usize);
    let n = f * f;
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0u32; 4];
            for dy in 0..f {
                let row = ((y * f + dy) * w + x * f) as usize * 4;
                for dx in 0..f as usize {
                    for c in 0..4 {
                        sum[c] += src[row + dx * 4 + c] as u32;
                    }
                }
            }
            out.extend(sum.iter().map(|s| (s / n) as u8));
        }
    }
    (nw, nh)
}

/// What to tell someone whose Windows has no Media Foundation (an N edition).
pub fn no_media_foundation() -> &'static str {
    t("Screen recording needs the Media Feature Pack, which this edition of Windows (an N edition) does not include. Install it in Settings > Apps > Optional features > Add a feature > Media Feature Pack, restart, and try again.")
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
        // 4 x 2 pixels to 2 x 1: each output pixel is the average of a 2 x 2 block.
        let px = |v: u8| [v, v, v, 255];
        let src: Vec<u8> = [px(0), px(100), px(10), px(10), px(200), px(100), px(30), px(50)].concat();
        let mut out = Vec::new();
        assert_eq!(shrink_bgra(&src, 4, 2, 2, &mut out), (2, 1));
        assert_eq!(out, [px(100), px(25)].concat());
    }
}
