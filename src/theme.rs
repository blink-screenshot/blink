//! Blink's house style: near-black ink with a single hot pink accent, taken from the logo.
//!
//! Every color the interface uses comes from here, so the editor, the settings and update
//! windows and the selection overlay stay in step. The pink is reserved for what is active,
//! selected or primary; text on top of it is always ink. Scrolling capture gets its own
//! signal cyan, so the two capture modes never look alike.
//!
//! Shapes follow the logo too: its tile has a corner radius of 58/256 of its side, and the corner
//! marks inside it run parallel to that curve. [`radius`] gives interface shapes the same ratio.
//!
//! The windows come in a dark and a light variant (Settings > General > Appearance). The
//! brand colors are the same in both; the surfaces, text and status colors are swapped by
//! [`pal`]. The capture overlay always stays dark: it sits on top of the screen.

use eframe::egui::Color32;
use std::sync::atomic::{AtomicBool, Ordering};

/// The palette as 0xRRGGBB, for code that does not use egui colors (the GDI overlay).
pub mod hex {
    /// Base of every surface, also the eye color of the app icon.
    pub const INK: u32 = 0x101114;
    /// Primary accent, the tile of the app icon: selection, primary buttons, active toggles.
    pub const ACCENT: u32 = 0xFF2E93;
    /// Muted pink, readable as text on the dark panels.
    pub const ACCENT_DIM: u32 = 0xE04C95;
    /// Deeper pink, readable as text on the light panels.
    pub const ACCENT_DEEP: u32 = 0xC41A63;
    /// Scrolling capture.
    pub const SIGNAL: u32 = 0x22D3EE;
    /// Darkest surface (behind the screenshot in the editor).
    pub const CANVAS: u32 = 0x0A0B0D;
    pub const BORDER: u32 = 0x2E3138;
    pub const MUTED: u32 = 0x989CA6;
}

const fn c(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

// Brand colors, the same in both variants.
pub const INK: Color32 = c(hex::INK);
pub const ACCENT: Color32 = c(hex::ACCENT);

/// Surface, text and status colors of one variant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Behind the screenshot in the editor.
    pub canvas: Color32,
    pub panel: Color32,
    pub panel_raised: Color32,
    pub input_bg: Color32,
    /// A group of related controls in the property bar.
    pub group: Color32,
    pub border: Color32,
    // Interactive states of neutral controls.
    pub control: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,
    pub control_strong: Color32,
    // Text.
    pub text: Color32,
    pub muted: Color32,
    pub disabled: Color32,
    // Status.
    pub ok: Color32,
    pub warn: Color32,
    pub error: Color32,
    /// Secondary accent that is readable as text on the panels.
    pub accent_text: Color32,
}

pub const DARK: Palette = Palette {
    dark: true,
    canvas: c(hex::CANVAS),
    panel: INK,
    panel_raised: c(0x17181C),
    input_bg: c(0x0D0E11),
    group: c(0x1F2126),
    border: c(hex::BORDER),
    control: c(0x23252B),
    control_hover: c(0x2D3037),
    control_active: c(0x373B44),
    control_strong: c(0x3A3E47),
    text: c(0xEAECF0),
    muted: c(hex::MUTED),
    disabled: c(0x565A63),
    ok: c(0x4ADE80),
    warn: c(0xFBBF24),
    error: c(0xFF7A6B),
    accent_text: c(hex::ACCENT_DIM),
};

pub const LIGHT: Palette = Palette {
    dark: false,
    canvas: c(0xDCDFE5),
    panel: c(0xF6F7F9),
    panel_raised: c(0xFFFFFF),
    input_bg: c(0xFFFFFF),
    group: c(0xEEF0F3),
    border: c(0xD3D6DC),
    control: c(0xE6E8EC),
    control_hover: c(0xDCDFE4),
    control_active: c(0xCFD3D9),
    control_strong: c(0xC5CAD1),
    text: c(0x15171C),
    muted: c(0x585D68),
    disabled: c(0xA3A8B0),
    ok: c(0x3F6212),
    warn: c(0x92400E),
    error: c(0xB91C1C),
    accent_text: c(hex::ACCENT_DEEP),
};

static LIGHT_ON: AtomicBool = AtomicBool::new(false);

/// Chooses the variant for this process (call before the first frame).
pub fn set_light(light: bool) {
    LIGHT_ON.store(light, Ordering::Relaxed);
}

/// The palette of the current variant.
pub fn pal() -> &'static Palette {
    if LIGHT_ON.load(Ordering::Relaxed) {
        &LIGHT
    } else {
        &DARK
    }
}

/// The colors offered for annotations. Functional, not branded, except for the pink swatch.
pub const ANNOTATION_COLORS: [Color32; 10] = [
    c(0xEF4444),
    c(0xF97316),
    c(0xFFE81A),
    c(0x22C55E),
    c(0x0EA5E9),
    c(0x8B5CF6),
    ACCENT,
    c(0xFFFFFF),
    c(0x78787F),
    c(0x111111),
];

/// Corner radius of the logo tile relative to its side (58 of 256).
pub const TILE_RADIUS_RATIO: f32 = 58.0 / 256.0;

/// Corner radius, in points, for an element whose shorter side is `side`: the logo's ratio,
/// kept between 3 and 18 so small controls stay crisp and large panels do not turn into blobs.
pub fn radius(side: f32) -> u8 {
    (side * TILE_RADIUS_RATIO).round().clamp(3.0, 18.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance of an sRGB color.
    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    fn distance(a: Color32, b: Color32) -> f64 {
        let d = |x: u8, y: u8| (x as f64 - y as f64).powi(2);
        (d(a.r(), b.r()) + d(a.g(), b.g()) + d(a.b(), b.b())).sqrt()
    }

    #[test]
    fn ink_on_accent_is_readable() {
        // Ink on hot pink is 5.5:1, above the 4.5:1 that WCAG AA asks for body text.
        assert!(contrast(INK, ACCENT) >= 4.5, "{}", contrast(INK, ACCENT));
    }

    #[test]
    fn signal_colors_are_readable_with_ink_on_top() {
        assert!(contrast(INK, c(hex::SIGNAL)) >= 7.0, "{}", contrast(INK, c(hex::SIGNAL)));
    }

    #[test]
    fn the_accent_is_not_confused_with_a_status_color() {
        for p in [DARK, LIGHT] {
            for status in [p.ok, p.warn, p.error] {
                assert!(distance(ACCENT, status) >= 80.0, "{status:?} dark={}: {}", p.dark, distance(ACCENT, status));
            }
        }
        assert!(distance(ACCENT, c(hex::SIGNAL)) >= 150.0);
    }

    #[test]
    fn radius_follows_the_logo_ratio() {
        assert_eq!(radius(32.0), 7); // an icon button
        assert_eq!(radius(26.0), 6); // a primary button
        assert_eq!(radius(10.0), 3); // never below 3
        assert_eq!(radius(300.0), 18); // never above 18
    }

    #[test]
    fn text_is_readable_on_every_surface() {
        for p in [DARK, LIGHT] {
            for bg in [p.canvas, p.panel, p.panel_raised, p.group, p.control] {
                assert!(contrast(p.text, bg) >= 7.0, "{bg:?} dark={}", p.dark);
                assert!(contrast(p.muted, bg) >= 4.5, "{bg:?} dark={}", p.dark);
            }
        }
    }

    #[test]
    fn status_colors_are_readable_on_the_panels() {
        for p in [DARK, LIGHT] {
            for fg in [p.ok, p.warn, p.error, p.accent_text] {
                for bg in [p.panel, p.panel_raised] {
                    assert!(contrast(fg, bg) >= 4.5, "{fg:?} on {bg:?}");
                }
            }
        }
    }

    #[test]
    fn hex_and_color32_agree() {
        assert_eq!(ACCENT, Color32::from_rgb(0xFF, 0x2E, 0x93));
        assert_eq!(INK, Color32::from_rgb(0x10, 0x11, 0x14));
    }
}
