//! Tone mapping for screenshots of HDR screens (experimental, off by default).
//!
//! With HDR on, Windows composes a monitor in scRGB: linear light, 16-bit floats per channel,
//! 1.0 = 80 nits, with values above "SDR white" for HDR content (games, HDR video). GDI's
//! BitBlt hands over 8-bit sRGB in which everything above SDR white is clipped per channel:
//! bright parts become flat white and saturated highlights shift in hue.
//!
//! This module turns such a picture into 8-bit sRGB without that:
//!
//! - Light is normalised so SDR white is 1.0 (SDR apps come out as on an SDR screen).
//! - Without anything brighter than SDR white the curve is the identity: the result is the
//!   same as BitBlt's, so screenshots of ordinary apps do not change.
//! - With brighter content, the brightest part of the range (above `KNEE`) is compressed so
//!   the brightest pixel lands on white and highlights keep their detail. The price: the top
//!   of the SDR range is compressed a little too (SDR white ends at about 243 instead of
//!   255), which is unavoidable in 8 bits.
//! - The curve works on the largest of R, G and B and scales the three together, so hues
//!   stay the same. Colours outside sRGB (wide-gamut content) are clipped at 0.
//!
//! Portable and tested; the Windows side (finding HDR monitors, reading the scRGB desktop)
//! is in `platform/win/hdr.rs`.

/// Below this (1.0 = SDR white) the curve is the identity, also when there are highlights.
pub const KNEE: f32 = 0.8;

/// Up to this peak the picture counts as SDR: rounding in Windows' own conversion can put SDR
/// white a hair above 1.0.
const SDR_PEAK: f32 = 1.02;

/// Peaks above this (4,000 nits at the default SDR white of 200 nits) are treated as this, so
/// a broken pixel cannot flatten the rest.
const MAX_PEAK: f32 = 50.0;

/// The tone curve for one picture, from its brightest value (1.0 = SDR white).
#[derive(Clone, Copy, Debug)]
pub struct Curve {
    /// 0 for the identity (no highlights); otherwise the shoulder of the compression above
    /// `KNEE`.
    m: f32,
}

impl Curve {
    pub fn new(peak: f32) -> Self {
        let peak = if peak.is_finite() { peak.min(MAX_PEAK) } else { MAX_PEAK };
        if peak <= SDR_PEAK {
            return Curve { m: 0.0 };
        }
        // Above the knee: k + d / (1 + d / m), with slope 1 at the knee (no visible kink)
        // and the peak landing exactly on 1.0.
        let r = 1.0 - KNEE;
        let d = peak - KNEE;
        Curve { m: r * d / (d - r) }
    }

    /// Whether the curve changes nothing (there are no highlights).
    pub fn is_identity(&self) -> bool {
        self.m == 0.0
    }

    /// Maps a value (1.0 = SDR white) to 0..=1.
    pub fn apply(&self, x: f32) -> f32 {
        if self.m == 0.0 || x <= KNEE {
            return x.clamp(0.0, 1.0);
        }
        let d = x - KNEE;
        (KNEE + d / (1.0 + d / self.m)).min(1.0)
    }
}

/// An IEEE 754 half-precision float (as in DXGI_FORMAT_R16G16B16A16_FLOAT) as f32.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let frac = (h & 0x3ff) as f32;
    match exp {
        0 => sign * frac * (1.0 / 16_777_216.0), // subnormal: frac * 2^-24
        31 => {
            if frac == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// Linear light 0..=1 to an 8-bit sRGB value, through a table.
pub struct Srgb {
    lut: Vec<u8>,
}

const LUT_SIZE: usize = 16_384;

impl Default for Srgb {
    fn default() -> Self {
        let lut = (0..LUT_SIZE)
            .map(|i| {
                let l = i as f32 / (LUT_SIZE - 1) as f32;
                let v = if l <= 0.003_130_8 { 12.92 * l } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
                (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            })
            .collect();
        Srgb { lut }
    }
}

impl Srgb {
    pub fn encode(&self, l: f32) -> u8 {
        // NaN becomes 0 through `as usize`.
        self.lut[((l.clamp(0.0, 1.0) * (LUT_SIZE - 1) as f32) + 0.5) as usize]
    }
}

/// R, G and B of one RGBA half-float pixel (8 bytes, little-endian), divided by `white`.
fn pixel(p: &[u8], white: f32) -> [f32; 3] {
    let c = |i: usize| half_to_f32(u16::from_le_bytes([p[i], p[i + 1]])) / white;
    [c(0), c(2), c(4)]
}

/// The brightest channel in a row of RGBA half floats (8 bytes per pixel), where `white` is
/// SDR white in scRGB units (1.0 = 80 nits). NaN and infinity are skipped.
pub fn row_peak(row: &[u8], white: f32) -> f32 {
    let mut peak = 0.0f32;
    for p in row.chunks_exact(8) {
        for v in pixel(p, white) {
            if v.is_finite() && v > peak {
                peak = v;
            }
        }
    }
    peak
}

/// Tone maps a row of RGBA half floats (8 bytes per pixel) into BGRA (4 bytes per pixel),
/// leaving the alpha bytes of `out` as they are. `out` must hold as many pixels as `row`.
pub fn map_row(row: &[u8], white: f32, curve: &Curve, srgb: &Srgb, out: &mut [u8]) {
    for (p, o) in row.chunks_exact(8).zip(out.chunks_exact_mut(4)) {
        let [r, g, b] = pixel(p, white).map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
        let max = r.max(g).max(b);
        // Scale the three channels together: same hue, brightness from the curve.
        let s = if max > KNEE && !curve.is_identity() { curve.apply(max) / max } else { 1.0 };
        o[0] = srgb.encode(b * s);
        o[1] = srgb.encode(g * s);
        o[2] = srgb.encode(r * s);
    }
}

/// Every how many pixels (and rows) `mean_mapped` and `mean_bgra` look at one.
pub const SAMPLE: usize = 4;

/// The sum and count of the sRGB values (0..=255, R, G and B) that `map_row` would write for
/// every `SAMPLE`th pixel of a row of RGBA half floats.
pub fn mean_mapped(row: &[u8], white: f32, curve: &Curve, srgb: &Srgb) -> (u64, u64) {
    let mut out = [0u8; 4];
    let (mut sum, mut n) = (0u64, 0u64);
    for p in row.chunks_exact(8).step_by(SAMPLE) {
        map_row(p, white, curve, srgb, &mut out);
        sum += out[..3].iter().map(|&v| v as u64).sum::<u64>();
        n += 3;
    }
    (sum, n)
}

/// The sum and count of B, G and R of every `SAMPLE`th pixel of a row of BGRA.
pub fn mean_bgra(row: &[u8]) -> (u64, u64) {
    let (mut sum, mut n) = (0u64, 0u64);
    for p in row.chunks_exact(4).step_by(SAMPLE) {
        sum += p[..3].iter().map(|&v| v as u64).sum::<u64>();
        n += 3;
    }
    (sum, n)
}

/// Whether a picture read through Desktop Duplication (average sRGB value `hdr`, 0..=255) is
/// not a picture of the screen that BitBlt saw (average `sdr`): some drivers hand over an
/// empty, black frame. Tone mapping darkens only highlights, and only a little, so a frame
/// far darker than BitBlt's is wrong. When BitBlt's picture is (nearly) black itself, there
/// is nothing to compare with and the frame counts as good.
pub fn looks_blank(hdr: f32, sdr: f32) -> bool {
    sdr > 8.0 && hdr < sdr * 0.25
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_to_half(v: f32) -> u16 {
        // Enough for test values: normal numbers, rounded to nearest.
        if v == 0.0 {
            return 0;
        }
        let bits = v.to_bits();
        let sign = ((bits >> 16) & 0x8000) as u16;
        let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
        let frac = ((bits & 0x7f_ffff) + 0x1000) >> 13;
        // A rounding carry out of the fraction correctly bumps the exponent.
        sign | (((exp as u16) << 10) + frac as u16)
    }

    fn rgba_half(px: &[[f32; 3]]) -> Vec<u8> {
        px.iter().flat_map(|[r, g, b]| [*r, *g, *b, 1.0]).flat_map(|v| f32_to_half(v).to_le_bytes()).collect()
    }

    #[test]
    fn half_floats() {
        assert_eq!(half_to_f32(0x0000), 0.0);
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0x3555), 0.333_251_95);
        assert_eq!(half_to_f32(0x7bff), 65504.0);
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(half_to_f32(0x7c00), f32::INFINITY);
        assert!(half_to_f32(0x7e00).is_nan());
        for v in [0.5f32, 1.0, 2.5, 12.0, 0.01] {
            assert!((half_to_f32(f32_to_half(v)) - v).abs() / v < 1e-3, "{v}");
        }
    }

    #[test]
    fn sdr_content_is_left_alone() {
        let c = Curve::new(1.0);
        assert!(c.is_identity());
        for x in [0.0, 0.2, 0.8, 0.95, 1.0] {
            assert_eq!(c.apply(x), x);
        }
        // Above SDR white without highlights: clipped, as BitBlt does.
        assert_eq!(Curve::new(1.01).apply(1.01), 1.0);
    }

    #[test]
    fn highlights_are_compressed_into_white() {
        for peak in [1.5f32, 2.0, 4.0, 12.5, 50.0, 1000.0, f32::INFINITY] {
            let c = Curve::new(peak);
            assert!(!c.is_identity());
            // Identity below the knee.
            assert_eq!(c.apply(0.5), 0.5);
            assert_eq!(c.apply(KNEE), KNEE);
            // The peak lands on white, and the curve keeps rising up to it.
            let top = peak.min(MAX_PEAK);
            assert!((c.apply(top) - 1.0).abs() < 1e-4, "peak {peak}");
            let mut last = 0.0;
            for i in 0..=100 {
                let x = KNEE + (top - KNEE) * i as f32 / 100.0;
                let y = c.apply(x);
                assert!(y >= last && y <= 1.0, "peak {peak}, x {x}");
                last = y;
            }
            // No kink at the knee: slope about 1 just above it.
            let slope = (c.apply(KNEE + 1e-3) - KNEE) / 1e-3;
            assert!((slope - 1.0).abs() < 0.02, "peak {peak}: slope {slope}");
            // SDR white gets a little darker, never much.
            assert!(c.apply(1.0) > 0.89 && c.apply(1.0) < 1.0, "peak {peak}");
        }
    }

    #[test]
    fn rows_keep_hue_and_sdr_pixels() {
        let srgb = Srgb::default();
        let white = 2.5; // 200 nits
                         // SDR white, mid grey, a bright orange highlight, an out-of-gamut colour.
        let row = rgba_half(&[[2.5, 2.5, 2.5], [0.5, 0.5, 0.5], [10.0, 5.0, 0.0], [-0.1, 0.5, 1.0]]);
        let peak = row_peak(&row, white);
        assert!((peak - 4.0).abs() < 0.01);

        // Without highlights, as an SDR screen: white is 255, mid grey (0.2 linear) is 124.
        let sdr = rgba_half(&[[2.5, 2.5, 2.5], [0.5, 0.5, 0.5]]);
        let curve = Curve::new(row_peak(&sdr, white));
        let mut out = vec![7u8; 8];
        map_row(&sdr, white, &curve, &srgb, &mut out);
        assert_eq!(out, [255, 255, 255, 7, 124, 124, 124, 7]);

        let curve = Curve::new(peak);
        let mut out = vec![0u8; 16];
        map_row(&row, white, &curve, &srgb, &mut out);
        // White is compressed a little, mid grey not at all.
        assert!(out[0] > 235 && out[0] < 255 && out[0] == out[1] && out[1] == out[2]);
        assert_eq!(&out[4..7], [124, 124, 124]);
        // The highlight keeps its orange (R at the top, G at half the light, B 0) instead of
        // clipping to yellow.
        let (b, g, r) = (out[8], out[9], out[10]);
        assert_eq!((r, b), (255, 0));
        assert!(g > 150 && g < 200, "g {g}");
        // Negative light is clipped to 0.
        assert_eq!(out[14], 0);
    }

    #[test]
    fn empty_frames_are_caught() {
        let srgb = Srgb::default();
        let white = 2.5;
        // A black frame next to BitBlt's picture of a light window: wrong.
        let black = rgba_half(&[[0.0; 3]; 8]);
        let curve = Curve::new(row_peak(&black, white));
        let (s, n) = mean_mapped(&black, white, &curve, &srgb);
        assert_eq!((s, n), (0, 6));
        let light: Vec<u8> = [[200u8, 210, 220, 0]; 8].concat();
        let (ls, ln) = mean_bgra(&light);
        assert_eq!((ls, ln), (1260, 6));
        assert!(looks_blank(s as f32 / n as f32, ls as f32 / ln as f32));
        // The same picture in scRGB: right, also with highlights compressed.
        let same = rgba_half(&[[1.5, 1.5, 1.5], [0.5, 0.5, 0.5], [10.0, 10.0, 10.0], [2.5, 2.5, 2.5]]);
        let curve = Curve::new(row_peak(&same, white));
        let (s, n) = mean_mapped(&same, white, &curve, &srgb);
        assert!(!looks_blank(s as f32 / n as f32, 210.0));
        // A dark screen cannot be judged: never blank.
        assert!(!looks_blank(0.0, 5.0));
        assert!(!looks_blank(60.0, 100.0));
    }

    #[test]
    fn broken_values_do_not_spread() {
        let srgb = Srgb::default();
        let row: Vec<u8> = [0x7e00u16, 0x7c00, 0x3c00, 0x3c00].iter().flat_map(|h| h.to_le_bytes()).collect();
        assert_eq!(row_peak(&row, 1.0), 1.0);
        let mut out = vec![0u8; 4];
        map_row(&row, 1.0, &Curve::new(1.0), &srgb, &mut out);
        assert_eq!(&out[..3], [255, 0, 0]);
    }
}
