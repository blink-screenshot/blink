//! Text recognition (OCR) and what Blink does with the recognized text.
//!
//! The recognition itself is done by the OCR engine built into Windows
//! (`Windows.Media.Ocr`, see `platform::win::ocr`): it runs on this PC, needs no download
//! and reads the languages the user has installed. Everything around it lives here and does
//! not depend on Windows, so it is tested on any platform:
//!
//! - preparing the image: small text is read far better when the image is enlarged, and the
//!   engine has a maximum image size, so a tall (scrolling) capture is read in overlapping
//!   bands that are stitched together again;
//! - the result as a [`Page`] of lines and words with their position in the image;
//! - turning a selection of words into text (reading order, one line per line, no spaces
//!   between Chinese / Japanese characters), and finding the word, line or paragraph at a
//!   point, which the editor and the capture overlay use to select text.

use crate::i18n::t;

/// A rectangle in image pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Bounds {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn union(&self, o: &Bounds) -> Bounds {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        Bounds { x, y, w: self.right().max(o.right()) - x, h: self.bottom().max(o.bottom()) - y }
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    /// Grown by `d` on every side.
    pub fn expand(&self, d: f32) -> Bounds {
        Bounds { x: self.x - d, y: self.y - d, w: self.w + 2.0 * d, h: self.h + 2.0 * d }
    }
    pub fn translate(&self, dx: f32, dy: f32) -> Bounds {
        Bounds { x: self.x + dx, y: self.y + dy, ..*self }
    }
    /// Squared distance from a point to this rectangle (0 inside).
    fn dist2(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x - x).max(0.0).max(x - self.right());
        let dy = (self.y - y).max(0.0).max(y - self.bottom());
        dx * dx + dy * dy
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: Bounds,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub words: Vec<Word>,
}

impl Line {
    pub fn rect(&self) -> Bounds {
        let mut it = self.words.iter().map(|w| w.rect);
        let first = it.next().unwrap_or_default();
        it.fold(first, |a, b| a.union(&b))
    }
}

/// Chinese, Japanese and Korean (Hangul is written with spaces, so it is not included):
/// the engine returns one "word" per character, and they are written without spaces.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // hiragana, katakana
        | 0x3400..=0x4DBF // CJK extension A
        | 0x4E00..=0x9FFF // CJK unified ideographs
        | 0xF900..=0xFAFF // compatibility ideographs
        | 0xFF00..=0xFFEF // full-width forms
        | 0x3000..=0x303F) // CJK punctuation
}

fn join_words<'a>(words: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for w in words {
        let first = w.chars().next();
        if let (Some(p), Some(f)) = (prev, first) {
            if !(is_cjk(p) && is_cjk(f)) {
                out.push(' ');
            }
        }
        out.push_str(w);
        prev = w.chars().last().or(prev);
    }
    out
}

/// Everything the engine read in an image, in reading order. A word is addressed by its
/// index in reading order (all words of the first line, then the second, ...).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub lines: Vec<Line>,
}

impl Page {
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.words.is_empty())
    }

    pub fn word_count(&self) -> usize {
        self.lines.iter().map(|l| l.words.len()).sum()
    }

    /// Every word with its index, and the index of its line.
    pub fn words(&self) -> impl Iterator<Item = (usize, usize, &Word)> {
        self.lines
            .iter()
            .enumerate()
            .flat_map(|(li, l)| l.words.iter().map(move |w| (li, w)))
            .enumerate()
            .map(|(i, (li, w))| (i, li, w))
    }

    pub fn word(&self, i: usize) -> Option<&Word> {
        self.words().nth(i).map(|(_, _, w)| w)
    }

    /// The index of the first word of every line.
    fn line_starts(&self) -> Vec<usize> {
        let mut acc = 0;
        self.lines
            .iter()
            .map(|l| {
                let s = acc;
                acc += l.words.len();
                s
            })
            .collect()
    }

    /// The words of line `li`, as indices.
    pub fn line_words(&self, li: usize) -> std::ops::Range<usize> {
        let starts = self.line_starts();
        match (starts.get(li), self.lines.get(li)) {
            (Some(&s), Some(l)) => s..s + l.words.len(),
            _ => 0..0,
        }
    }

    /// The word under `(x, y)`, allowing `slack` pixels around each word (the gaps between
    /// words count as the nearest word on that line).
    pub fn word_at(&self, x: f32, y: f32, slack: f32) -> Option<usize> {
        let li = self.line_at(x, y, slack)?;
        self.line_words(li)
            .zip(&self.lines[li].words)
            .min_by(|a, b| a.1.rect.dist2(x, y).total_cmp(&b.1.rect.dist2(x, y)))
            .map(|(i, _)| i)
    }

    /// The line under `(x, y)`: the line box grown by `slack` (and by a little of its own
    /// height), the closest one if several lines are that close.
    pub fn line_at(&self, x: f32, y: f32, slack: f32) -> Option<usize> {
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.words.is_empty())
            .map(|(i, l)| {
                let r = l.rect();
                (i, r.expand(slack + r.h * 0.15), r.dist2(x, y))
            })
            .filter(|(_, r, _)| r.contains(x, y))
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(i, _, _)| i)
    }

    /// The word nearest to `(x, y)`, however far: where a selection drag ends when the mouse
    /// is past the end of the text.
    pub fn nearest_word(&self, x: f32, y: f32) -> Option<usize> {
        if let Some(i) = self.word_at(x, y, 0.0) {
            return Some(i);
        }
        // The closest line first (by height), then the closest word on it: dragging to the
        // right of a line selects to its end, not to a word on another line.
        let li = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.words.is_empty())
            .min_by(|a, b| {
                let (ra, rb) = (a.1.rect(), b.1.rect());
                let da = (ra.y - y).max(0.0).max(y - ra.bottom());
                let db = (rb.y - y).max(0.0).max(y - rb.bottom());
                da.total_cmp(&db).then(ra.dist2(x, y).total_cmp(&rb.dist2(x, y)))
            })?
            .0;
        self.line_words(li)
            .zip(&self.lines[li].words)
            .min_by(|a, b| a.1.rect.dist2(x, y).total_cmp(&b.1.rect.dist2(x, y)))
            .map(|(i, _)| i)
    }

    /// The words whose center is inside `r`.
    pub fn words_in(&self, r: &Bounds) -> Vec<usize> {
        self.words().filter(|(_, _, w)| r.contains(w.rect.cx(), w.rect.cy())).map(|(i, _, _)| i).collect()
    }

    /// The lines of the paragraph that line `li` is part of: the lines right above and below
    /// it with about the same text size, that overlap it sideways and leave no more than a
    /// small gap. Lines in a menu or a list have bigger gaps, so they stay apart.
    pub fn paragraph(&self, li: usize) -> std::ops::RangeInclusive<usize> {
        let fits = |a: usize, b: usize| {
            let (ra, rb) = (self.lines[a].rect(), self.lines[b].rect());
            let (ha, hb) = (ra.h.max(1.0), rb.h.max(1.0));
            let ratio = ha / hb;
            let gap = rb.y - ra.bottom();
            let side = ra.x < rb.right() && rb.x < ra.right();
            side && (0.7..=1.45).contains(&ratio) && gap > -ha * 0.5 && gap < ha.min(hb) * 0.75
        };
        if li >= self.lines.len() {
            return li..=li;
        }
        let (mut a, mut b) = (li, li);
        while a > 0 && fits(a - 1, a) {
            a -= 1;
        }
        while b + 1 < self.lines.len() && fits(b, b + 1) {
            b += 1;
        }
        a..=b
    }

    /// The text of the selected words (`keep(index)`), one line of text per line of the page.
    pub fn text_where(&self, keep: impl Fn(usize) -> bool) -> String {
        let mut out: Vec<String> = Vec::new();
        let mut i = 0;
        for l in &self.lines {
            let words: Vec<&str> = l
                .words
                .iter()
                .filter(|_| {
                    let k = keep(i);
                    i += 1;
                    k
                })
                .map(|w| w.text.as_str())
                .collect();
            if !words.is_empty() {
                out.push(join_words(words.into_iter()));
            }
        }
        out.join("\n")
    }

    pub fn text_of(&self, words: &[usize]) -> String {
        let set: std::collections::HashSet<usize> = words.iter().copied().collect();
        self.text_where(|i| set.contains(&i))
    }

    /// The text of whole lines (`lines` are line indices).
    pub fn lines_text(&self, lines: &[usize]) -> String {
        let words: Vec<usize> = lines.iter().flat_map(|&li| self.line_words(li)).collect();
        self.text_of(&words)
    }

    #[cfg(test)]
    pub fn all_text(&self) -> String {
        self.text_where(|_| true)
    }

    /// The rectangles that show a selection: one per run of selected words on a line.
    pub fn selection_rects(&self, selected: &[usize]) -> Vec<Bounds> {
        let set: std::collections::HashSet<usize> = selected.iter().copied().collect();
        let mut out = Vec::new();
        let mut i = 0;
        for l in &self.lines {
            let mut run: Option<Bounds> = None;
            for w in &l.words {
                if set.contains(&i) {
                    run = Some(run.map_or(w.rect, |r| r.union(&w.rect)));
                } else if let Some(r) = run.take() {
                    out.push(r);
                }
                i += 1;
            }
            out.extend(run);
        }
        out
    }

    /// Moves every word by `(dx, dy)`.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        for l in &mut self.lines {
            for w in &mut l.words {
                w.rect = w.rect.translate(dx, dy);
            }
        }
    }

    /// Appends the lines of `other` (read after this page: a band below it, the next screen).
    pub fn append(&mut self, other: Page) {
        self.lines.extend(other.lines.into_iter().filter(|l| !l.words.is_empty()));
    }
}

/// A short, single-line preview of `text` (for notifications and labels).
pub fn preview(text: &str, max_chars: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let cut: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}\u{2026}", cut.trim_end())
}

// ---- recognition -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OcrError {
    /// This system has no OCR engine (Blink only reads text on Windows).
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
    /// Windows has no OCR language installed.
    NoLanguage,
    Failed(String),
}

impl OcrError {
    pub fn message(&self) -> String {
        match self {
            OcrError::Unsupported => t("Reading text needs Windows 10 or 11.").into(),
            OcrError::NoLanguage => t("Windows has no text recognition language installed. Add one in Settings > \
                 Time & language > Language & region (it comes with the language pack).")
            .into(),
            OcrError::Failed(e) => crate::i18n::tf("Could not read the text: {0}", &[e]),
        }
    }
}

/// An OCR engine that reads one image of at most `max_dimension` pixels on each side.
pub trait Engine {
    fn max_dimension(&self) -> u32;
    /// Reads a BGRA image (8 bits per channel, top-down, `w * 4` bytes per row).
    fn read(&self, bgra: &[u8], w: u32, h: u32) -> Result<Page, OcrError>;
}

/// Overlap between two bands of a tall image, in (enlarged) pixels: more than the height
/// of a line of large text, so every line is entirely inside at least one band.
const BAND_OVERLAP: u32 = 240;

/// How much to enlarge an image before reading it. Screen text is small (9 to 12 pixels on
/// a 100% display), and the engine reads it much better at twice the size. Big images are
/// left as they are, or shrunk when they are wider than the engine allows.
pub fn upscale_for(w: u32, h: u32, max_dimension: u32) -> f32 {
    let want: f32 = if (w as u64 * h as u64) <= 2_600_000 { 2.0 } else { 1.0 };
    let fit = max_dimension as f32 / w.max(1) as f32;
    want.min(fit)
}

/// Reads a BGRA image of any size with `engine`: enlarged for small text, and in overlapping
/// bands when it is taller than the engine allows. Word positions are in the pixels of the
/// image passed in.
pub fn read_with(engine: &dyn Engine, bgra: &[u8], w: u32, h: u32) -> Result<Page, OcrError> {
    if w == 0 || h == 0 || bgra.len() < (w * h * 4) as usize {
        return Ok(Page::default());
    }
    let max = engine.max_dimension().max(BAND_OVERLAP * 3);
    let scale = upscale_for(w, h, max);
    let (sw, sh) = (((w as f32 * scale).round() as u32).clamp(1, max), ((h as f32 * scale).round() as u32).max(1));
    let scaled;
    let img: &[u8] = if (sw, sh) == (w, h) {
        bgra
    } else {
        // The channel order does not matter for resizing. Read where it is, not copied.
        let src = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(w, h, &bgra[..(w * h * 4) as usize])
            .ok_or_else(|| OcrError::Failed("image size".into()))?;
        let filter =
            if scale > 1.0 { image::imageops::FilterType::CatmullRom } else { image::imageops::FilterType::Triangle };
        scaled = image::imageops::resize(&src, sw, sh, filter).into_raw();
        &scaled
    };
    let (kx, ky) = (w as f32 / sw as f32, h as f32 / sh as f32);
    let mut page = Page::default();
    for band in bands(sh, max) {
        let part = &img[(band.top * sw * 4) as usize..(band.bottom * sw * 4) as usize];
        let bh = band.bottom - band.top;
        let mut read = engine.read(part, sw, bh)?;
        // OCR engines are made for dark text on a light background. Light text on a dark
        // background (dark mode, terminals, code editors) is read far better inverted (a
        // quarter more words on a dark OneNote page), so a band with a large dark area is read
        // a second time inverted. The engine skips words it is unsure of, and not the same
        // ones in both readings, so per line the reading with the most text is kept.
        if dark_share(part) >= DARK_SHARE_TO_INVERT {
            let inverted: Vec<u8> =
                part.chunks_exact(4).flat_map(|p| [255 - p[0], 255 - p[1], 255 - p[2], p[3]]).collect();
            read.lines = merge_readings(read.lines, engine.read(&inverted, sw, bh)?.lines);
        }
        for mut line in read.lines {
            // A line in the overlap of two bands is read twice: keep it in the band that has
            // its middle in its own part.
            let cy = line.rect().cy() + band.top as f32;
            if cy < band.own_top as f32 || cy >= band.own_bottom as f32 {
                continue;
            }
            for word in &mut line.words {
                let r = word.rect;
                word.rect = Bounds::new(r.x * kx, (r.y + band.top as f32) * ky, r.w * kx, r.h * ky);
            }
            page.lines.push(line);
        }
    }
    page.lines.retain(|l| !l.words.is_empty());
    Ok(page)
}

/// The share of dark pixels from which an image is also read inverted.
const DARK_SHARE_TO_INVERT: f32 = 0.2;

/// The share of dark pixels in a BGRA image (every 7th pixel is enough to tell).
fn dark_share(bgra: &[u8]) -> f32 {
    let (mut dark, mut all) = (0u32, 0u32);
    for p in bgra.chunks_exact(4).step_by(7) {
        let luma = (p[2] as u32 * 299 + p[1] as u32 * 587 + p[0] as u32 * 114) / 1000;
        dark += (luma < 96) as u32;
        all += 1;
    }
    dark as f32 / all.max(1) as f32
}

/// Combines two readings of the same image. A line of `other` in a new place is added; a line
/// of `other` that holds more text than the lines of `lines` in its place replaces them (one
/// reading may have skipped a word, or split the line in two); otherwise `lines` is kept.
/// Replaced lines keep their place in the reading order.
fn merge_readings(mut lines: Vec<Line>, other: Vec<Line>) -> Vec<Line> {
    let chars = |l: &Line| l.words.iter().map(|w| w.text.chars().count()).sum::<usize>();
    for line in other {
        let r = line.rect();
        let same: Vec<usize> = (0..lines.len()).filter(|&i| overlaps(&lines[i].rect(), &r)).collect();
        if same.is_empty() {
            lines.push(line);
        } else if chars(&line) > same.iter().map(|&i| chars(&lines[i])).sum() {
            for &i in same.iter().rev() {
                lines.remove(i);
            }
            lines.insert(same[0], line);
        }
    }
    lines
}

/// Whether two line boxes are mostly the same place (the same line read twice).
fn overlaps(a: &Bounds, b: &Bounds) -> bool {
    let w = a.right().min(b.right()) - a.x.max(b.x);
    let h = a.bottom().min(b.bottom()) - a.y.max(b.y);
    w > 0.0 && h > 0.0 && w * h > 0.5 * (a.w * a.h).min(b.w * b.h)
}

/// A horizontal strip of the image that is read on its own: rows `top..bottom`; lines whose
/// middle is in `own_top..own_bottom` belong to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Band {
    top: u32,
    bottom: u32,
    own_top: u32,
    own_bottom: u32,
}

fn bands(h: u32, max: u32) -> Vec<Band> {
    if h <= max {
        return vec![Band { top: 0, bottom: h, own_top: 0, own_bottom: h }];
    }
    let step = max - BAND_OVERLAP;
    let mut out = Vec::new();
    let mut top = 0;
    loop {
        let bottom = (top + max).min(h);
        let own_top = if top == 0 { 0 } else { top + BAND_OVERLAP / 2 };
        let last = bottom == h;
        let own_bottom = if last { h } else { bottom - BAND_OVERLAP / 2 };
        out.push(Band { top, bottom, own_top, own_bottom });
        if last {
            break;
        }
        top += step;
    }
    out
}

/// Reads the text in a BGRA image with the system's OCR engine. Slow (a few hundred
/// milliseconds for a screen): call it on a worker thread.
pub fn read_bgra(bgra: &[u8], w: u32, h: u32) -> Result<Page, OcrError> {
    let engine = crate::platform::ocr_engine()?;
    read_with(engine.as_ref(), bgra, w, h)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn word(text: &str, x: f32, y: f32, w: f32, h: f32) -> Word {
        Word { text: text.into(), rect: Bounds::new(x, y, w, h) }
    }

    /// A line of words, 10 px per character plus 5 px spaces, `h` high, starting at `(x, y)`.
    pub fn line_at(text: &str, x: f32, y: f32, h: f32) -> Line {
        let mut cx = x;
        let words = text
            .split(' ')
            .filter(|w| !w.is_empty())
            .map(|w| {
                let width = w.chars().count() as f32 * 10.0;
                let out = word(w, cx, y, width, h);
                cx += width + 5.0;
                out
            })
            .collect();
        Line { words }
    }

    pub fn page(lines: &[(&str, f32, f32)]) -> Page {
        Page { lines: lines.iter().map(|(t, x, y)| line_at(t, *x, *y, 12.0)).collect() }
    }

    #[test]
    fn joins_words_and_lines() {
        let p = page(&[("Hello world", 0.0, 0.0), ("second line", 0.0, 20.0)]);
        assert_eq!(p.all_text(), "Hello world\nsecond line");
        assert_eq!(p.word_count(), 4);
        assert_eq!(p.text_of(&[1, 2]), "world\nsecond");
        // Chinese characters are not separated by spaces, Latin words around them are.
        let cjk = Page { lines: vec![line_at("在 这 里 OK", 0.0, 0.0, 12.0)] };
        assert_eq!(cjk.all_text(), "在这里 OK");
    }

    #[test]
    fn finds_words_lines_and_paragraphs() {
        let p = page(&[
            ("The quick brown", 0.0, 0.0),
            ("fox jumps over", 0.0, 15.0),
            // A bigger gap: a new paragraph.
            ("Menu item", 0.0, 60.0),
        ]);
        assert_eq!(p.word_at(2.0, 5.0, 0.0), Some(0));
        // Between "quick" (x 55..105) and "brown" (110..160): the nearest of the two.
        assert_eq!(p.word_at(108.0, 5.0, 2.0), Some(2));
        assert_eq!(p.line_at(5.0, 20.0, 2.0), Some(1));
        assert_eq!(p.line_at(5.0, 45.0, 2.0), None);
        assert_eq!(p.paragraph(0), 0..=1);
        assert_eq!(p.paragraph(1), 0..=1);
        assert_eq!(p.paragraph(2), 2..=2);
        // Past the end of the first line: its last word.
        assert_eq!(p.nearest_word(500.0, 6.0), Some(2));
        assert_eq!(p.words_in(&Bounds::new(-1.0, -1.0, 60.0, 40.0)), vec![0, 3]);
    }

    #[test]
    fn selection_rects_cover_runs_per_line() {
        let p = page(&[("a b c", 0.0, 0.0), ("d e", 0.0, 20.0)]);
        let r = p.selection_rects(&[1, 2, 3]);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].x, r[0].right()), (15.0, 40.0));
        assert_eq!((r[1].x, r[1].right()), (0.0, 10.0));
        assert_eq!(p.selection_rects(&[0, 2]).len(), 2);
    }

    #[test]
    fn preview_is_short_and_on_one_line() {
        assert_eq!(preview("a\n b", 10), "a b");
        assert_eq!(preview("abcdefghijkl", 6), "abcde\u{2026}");
    }

    #[test]
    fn bands_overlap_and_cover_everything() {
        assert_eq!(bands(100, 1000), vec![Band { top: 0, bottom: 100, own_top: 0, own_bottom: 100 }]);
        let b = bands(5000, 2000);
        assert_eq!(b.first().unwrap().top, 0);
        assert_eq!(b.last().unwrap().bottom, 5000);
        for pair in b.windows(2) {
            // Each band overlaps the next, and their own parts meet exactly.
            assert!(pair[1].top < pair[0].bottom);
            assert_eq!(pair[0].own_bottom, pair[1].own_top);
            assert!(pair[0].bottom - pair[0].top <= 2000);
        }
    }

    /// Pretends to read: returns one word per row that starts dark ("ink") and ends light
    /// ("paper"), so tests can check where words end up after scaling and banding, and that
    /// light text on a dark background is only read inverted, like a real engine prefers.
    struct Fake {
        max: u32,
        calls: std::cell::Cell<u32>,
    }

    impl Engine for Fake {
        fn max_dimension(&self) -> u32 {
            self.max
        }
        fn read(&self, bgra: &[u8], w: u32, h: u32) -> Result<Page, OcrError> {
            assert!(w <= self.max && h <= self.max, "{w} x {h} is too big");
            self.calls.set(self.calls.get() + 1);
            let mut page = Page::default();
            for y in 0..h {
                let row = &bgra[(y * w * 4) as usize..((y + 1) * w * 4) as usize];
                if row[0] < 128 && row[row.len() - 4] >= 128 {
                    page.lines.push(Line { words: vec![word(&format!("row{y}"), 0.0, y as f32, 20.0, 10.0)] });
                }
            }
            Ok(page)
        }
    }

    #[test]
    fn tall_images_are_read_in_bands_without_duplicates() {
        // 500 x 6000: too tall for the engine, too big to enlarge.
        let (w, h) = (500u32, 6000u32);
        let mut img = vec![255u8; (w * h * 4) as usize];
        let marks = [0u32, 1990, 2500, 4100, 5900];
        for y in marks {
            for dy in 0..10 {
                img[((y + dy) * w * 4) as usize] = 0;
            }
        }
        let engine = Fake { max: 2600, calls: Default::default() };
        let page = read_with(&engine, &img, w, h).unwrap();
        assert!(engine.calls.get() >= 3);
        // Each mark is a block of 10 rows; the fake reads one "line" per row, so 10 per mark,
        // each exactly once, at its own place.
        assert_eq!(page.lines.len(), marks.len() * 10);
        let ys: Vec<f32> = page.lines.iter().map(|l| l.rect().y).collect();
        for y in marks {
            for dy in 0..10 {
                assert!(ys.iter().any(|v| (*v - (y + dy) as f32).abs() < 0.01), "row {}", y + dy);
            }
        }
    }

    #[test]
    fn small_images_are_enlarged_and_positions_map_back() {
        let (w, h) = (300u32, 200u32);
        let mut img = vec![255u8; (w * h * 4) as usize];
        for x in 0..w / 2 {
            for dy in 0..4 {
                img[(((100 + dy) * w + x) * 4) as usize] = 0;
            }
        }
        assert_eq!(upscale_for(w, h, 10000), 2.0);
        let engine = Fake { max: 10000, calls: Default::default() };
        let page = read_with(&engine, &img, w, h).unwrap();
        // Found in the enlarged image, reported in the original pixels.
        assert!(!page.lines.is_empty());
        assert!(page.lines.iter().all(|l| (98.0..106.0).contains(&l.rect().y)));
        assert!(page.lines.iter().all(|l| (l.rect().w - 10.0).abs() < 0.01));
    }

    /// A BGRA image of `w` x `h` in `paper`, with "text" rows `ink` on the left half.
    fn rows(w: u32, h: u32, paper: u8, ink: u8, text_rows: &[u32]) -> Vec<u8> {
        let mut img = vec![paper; (w * h * 4) as usize];
        for &y in text_rows {
            for x in 0..w / 2 {
                let i = ((y * w + x) * 4) as usize;
                img[i..i + 3].copy_from_slice(&[ink; 3]);
            }
        }
        img
    }

    #[test]
    fn light_text_on_dark_is_read_inverted() {
        // Dark mode: light text on a near-black background, like a code editor.
        let (w, h) = (3000u32, 1000u32);
        let img = rows(w, h, 31, 235, &[200, 600]);
        let engine = Fake { max: 10000, calls: Default::default() };
        let page = read_with(&engine, &img, w, h).unwrap();
        assert_eq!(engine.calls.get(), 2);
        let ys: Vec<f32> = page.lines.iter().map(|l| l.rect().y).collect();
        assert_eq!(ys, vec![200.0, 600.0]);
    }

    #[test]
    fn light_images_are_read_once() {
        let (w, h) = (3000u32, 1000u32);
        let img = rows(w, h, 250, 20, &[200, 600]);
        let engine = Fake { max: 10000, calls: Default::default() };
        let page = read_with(&engine, &img, w, h).unwrap();
        assert_eq!(engine.calls.get(), 1);
        assert_eq!(page.lines.len(), 2);
    }

    #[test]
    fn a_dark_area_on_a_light_page_adds_its_lines_once() {
        // Top half a light page with dark text, bottom half a dark code block with light text.
        let (w, h) = (3000u32, 1000u32);
        let mut img = rows(w, h, 250, 20, &[100, 300]);
        let dark = rows(w, 500, 31, 235, &[200]);
        img[(500 * w * 4) as usize..].copy_from_slice(&dark);
        let engine = Fake { max: 10000, calls: Default::default() };
        let page = read_with(&engine, &img, w, h).unwrap();
        let mut ys: Vec<f32> = page.lines.iter().map(|l| l.rect().y).collect();
        ys.sort_by(f32::total_cmp);
        // The page's lines come from the normal reading, the code block's from the inverted
        // one, each exactly once.
        assert_eq!(ys, vec![100.0, 300.0, 700.0]);
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")).collect()
    }

    #[test]
    fn merged_readings_keep_the_fullest_line() {
        let line =
            |ws: &[(&str, f32)], y: f32| Line { words: ws.iter().map(|(t, x)| word(t, *x, y, 60.0, 20.0)).collect() };
        let normal = vec![
            line(&[("Review", 0.0), ("Excel", 70.0)], 0.0),
            // "Sandor follows" skipped.
            line(&[("paste", 140.0), ("data", 210.0)], 40.0),
            line(&[("Keanu", 0.0), ("shares", 70.0), ("template", 140.0)], 80.0),
            // A line split in two.
            line(&[("First", 0.0)], 120.0),
            line(&[("action", 70.0)], 120.0),
        ];
        let inverted = vec![
            line(&[("Review", 0.0), ("Excel", 70.0)], 0.0),
            line(&[("Sandor", 0.0), ("follows", 70.0), ("paste", 140.0), ("data", 210.0)], 40.0),
            // Less text than the normal reading: not taken.
            line(&[("Keanu", 0.0), ("template", 140.0)], 80.0),
            line(&[("First", 0.0), ("action:", 70.0)], 120.0),
            // Only in this reading.
            line(&[("Stedin", 300.0)], 160.0),
        ];
        assert_eq!(
            texts(&merge_readings(normal, inverted)),
            ["Review Excel", "Sandor follows paste data", "Keanu shares template", "First action:", "Stedin"]
        );
    }

    #[test]
    fn wide_images_are_shrunk_to_fit() {
        assert!(upscale_for(8000, 1000, 4000) < 1.0);
        assert_eq!(upscale_for(3840, 2160, 10000), 1.0);
    }
}
