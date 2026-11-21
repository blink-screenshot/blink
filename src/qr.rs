//! Finding and reading QR codes on the screen, for the capture overlay: point at a QR code to
//! copy what it says, or Ctrl+click to open its link. Uses `rqrr` (pure Rust, offline).

/// A QR code found on the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Code {
    /// Bounding box in the image: x, y, width, height.
    pub rect: (i32, i32, i32, i32),
    pub content: String,
}

/// Finds the QR codes in a BGRA (or RGBA) image. Takes a moment on a full screen: call it
/// off the UI thread.
#[cfg(test)]
pub fn find_bgra(bgra: &[u8], w: usize, h: usize) -> Vec<Code> {
    if w == 0 || h == 0 || bgra.len() < w * h * 4 {
        return Vec::new();
    }
    find_luma(&luma(&bgra[..w * h * 4]), w, h)
}

/// The brightness of every pixel of a BGRA (or RGBA) image, one byte per pixel; the channel
/// order hardly matters for black and white codes. A quarter of the image's size: hand this
/// to a worker thread instead of the image itself.
pub fn luma(bgra: &[u8]) -> Vec<u8> {
    bgra.chunks_exact(4).map(|p| ((p[0] as u32 * 29 + p[1] as u32 * 150 + p[2] as u32 * 77) >> 8) as u8).collect()
}

/// Finds the QR codes in a brightness image (see `luma`).
pub fn find_luma(gray: &[u8], w: usize, h: usize) -> Vec<Code> {
    if w == 0 || h == 0 || gray.len() < w * h {
        return Vec::new();
    }
    let found = find_gray(w, h, |i| gray[i]);
    if !found.is_empty() {
        return found;
    }
    // Light codes on a dark page (dark mode) are read inverted (without an inverted copy).
    find_gray(w, h, |i| 255 - gray[i])
}

fn find_gray(w: usize, h: usize, pixel: impl Fn(usize) -> u8) -> Vec<Code> {
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| pixel(y * w + x));
    let mut out: Vec<Code> = Vec::new();
    for grid in img.detect_grids() {
        let mut bytes = Vec::new();
        if grid.decode_to(&mut bytes).is_err() {
            continue;
        }
        let content = String::from_utf8_lossy(&bytes).trim().to_string();
        if content.is_empty() {
            continue;
        }
        let xs = grid.bounds.iter().map(|p| p.x);
        let ys = grid.bounds.iter().map(|p| p.y);
        let (x0, x1) = (xs.clone().min().unwrap_or(0), xs.max().unwrap_or(0));
        let (y0, y1) = (ys.clone().min().unwrap_or(0), ys.max().unwrap_or(0));
        let code = Code { rect: (x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)), content };
        if !out.contains(&code) {
            out.push(code);
        }
    }
    out
}

/// Whether `s` is a web link that is safe to hand to the browser: http or https only (not
/// `file:`, `javascript:` or app protocols), one line, no spaces.
pub fn is_web_link(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"));
    rest.is_some_and(|r| !r.is_empty() && !r.starts_with('/'))
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
        && s.len() <= 4096
}

#[cfg(test)]
mod tests {
    use super::*;

    // "https://blink-screenshot.github.io/blink/" as a version 3 QR code (# = dark module).
    const BLINK_URL: &[&str] = &[
        "#######......###.###..#######",
        "#.....#.#.#..###..##..#.....#",
        "#.###.#....#..###...#.#.###.#",
        "#.###.#...#...##..#...#.###.#",
        "#.###.#.####.###...#..#.###.#",
        "#.....#......#.######.#.....#",
        "#######.#.#.#.#.#.#.#.#######",
        ".........##..##.##...........",
        "#.#.#.#..#####.####.....#..#.",
        ".###...##.#.........###..#..#",
        "#.....#.######...#..#.....###",
        "..#..#.#####.#..##..#.#.#..#.",
        "...##.#.#..#.#.###.##.#..#.##",
        "..#..#...##.....##..###..#..#",
        "#.######.##..##..#...#.###.##",
        "#.##......##.###.####..###.#.",
        "###.#.##.#.#.#####..###..#.##",
        ".##.#..###.#....#...#.#..##.#",
        "#.##..#####.##..#.#...#.#..##",
        ".###.#.##.#.###.#####.#.##.#.",
        "#.#.#.#...#.#.####..#####....",
        "........##..##...####...#.###",
        "#######...###.#.#..##.#.##.##",
        "#.....#.....##...#..#...##...",
        "#.###.#.##....#.##..#####....",
        "#.###.#....#.#..#..##..##.###",
        "#.###.#.#.#.##....#.#..###..#",
        "#.....#...#..#####..##.##..#.",
        "#######.####.....#.###..#..##",
    ];

    /// The code drawn with `px` pixels per module, a quiet zone of 4 modules, at (`ox`, `oy`)
    /// on a `w` x `h` page; `dark_mode` swaps black and white.
    fn render(px: usize, ox: usize, oy: usize, w: usize, h: usize, dark_mode: bool) -> Vec<u8> {
        let (fg, bg) = if dark_mode { (230u8, 30u8) } else { (20, 250) };
        let mut img = vec![bg; w * h * 4];
        for (my, row) in BLINK_URL.iter().enumerate() {
            for (mx, c) in row.chars().enumerate() {
                if c != '#' {
                    continue;
                }
                for y in 0..px {
                    for x in 0..px {
                        let (x, y) = (ox + (mx + 4) * px + x, oy + (my + 4) * px + y);
                        img[(y * w + x) * 4..][..3].copy_from_slice(&[fg, fg, fg]);
                    }
                }
            }
        }
        img
    }

    #[test]
    fn reads_a_code_on_a_page() {
        let (w, h) = (500, 300);
        let img = render(4, 200, 40, w, h, false);
        let found = find_bgra(&img, w, h);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].content, "https://blink-screenshot.github.io/blink/");
        let (x, y, cw, ch) = found[0].rect;
        // The code (without quiet zone) is at 216, 56 and 29 * 4 = 116 px wide.
        assert!((x - 216).abs() <= 3 && (y - 56).abs() <= 3, "{:?}", found[0].rect);
        assert!((cw - 116).abs() <= 4 && (ch - 116).abs() <= 4, "{:?}", found[0].rect);
    }

    #[test]
    fn reads_a_light_code_on_a_dark_page_and_small_codes() {
        let (w, h) = (300, 200);
        let dark = render(3, 10, 10, w, h, true);
        let found = find_bgra(&dark, w, h);
        assert_eq!(found.len(), 1);
        // What the overlay's worker thread gets: the brightness only.
        assert_eq!(find_luma(&luma(&dark), w, h), found);
        let found = find_bgra(&render(2, 30, 30, w, h, false), w, h);
        assert_eq!(found.len(), 1);
        assert!(find_bgra(&vec![255; w * h * 4], w, h).is_empty());
    }

    #[test]
    fn only_web_links_are_opened() {
        assert!(is_web_link("https://example.com/a?b=c"));
        assert!(is_web_link("HTTP://example.com"));
        for bad in [
            "file:///C:/Windows/system32/calc.exe",
            "javascript:alert(1)",
            "ms-settings:privacy",
            "https://",
            "https:///x",
            "https://exa mple.com",
            "https://example.com\nhttps://other",
            "WIFI:S:home;T:WPA;P:secret;;",
            "example.com",
        ] {
            assert!(!is_web_link(bad), "{bad}");
        }
    }
}
