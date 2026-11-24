//! The ruling lines of tables: the borders between rows and columns, found in the image.
//!
//! The OCR engine returns words with their positions and knows nothing about tables. When a
//! table has borders, those borders say exactly where every row and column is: a cell with
//! two lines of text is one row, a header that starts to the left of the values under it is
//! still in their column. So the image is searched for thin, long, straight lines (once, next
//! to the reading), and lines that cross each other or share their length form a [`Grid`].
//!
//! A line pixel is darker (or lighter, in dark mode) than the pixels two above and two below
//! it (for a horizontal line), so lines up to three pixels thick are found, a filled area or
//! the edge of a panel is not. Short runs are left out: those are strokes of letters.

use crate::ocr::Bounds;

/// The least difference in brightness between a line and the background next to it.
const CONTRAST: i16 = 16;
/// How far from the line the background is looked at, in pixels.
const REACH: usize = 2;
/// Gaps in a line that are still one line (anti-aliasing, a letter touching it).
const MAX_GAP: usize = 3;
/// The shortest horizontal and vertical lines, in pixels.
const MIN_H: usize = 30;
const MIN_V: usize = 20;
/// How close lines must be to touch, or to be at the same place, in pixels.
const TOL: f32 = 3.0;

/// A straight line: horizontal at `y = pos` from `x = from` to `to`, or vertical at `x = pos`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub pos: f32,
    pub from: f32,
    pub to: f32,
}

impl Seg {
    fn len(&self) -> f32 {
        self.to - self.from
    }
}

/// The ruling lines in an image.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub h: Vec<Seg>,
    pub v: Vec<Seg>,
}

/// Finds the ruling lines in a BGRA image (8 bits per channel, `w * 4` bytes per row).
pub fn find(bgra: &[u8], w: u32, h: u32) -> Rules {
    let (w, h) = (w as usize, h as usize);
    if w <= 2 * REACH || h <= 2 * REACH || bgra.len() < w * h * 4 {
        return Rules::default();
    }
    let luma: Vec<i16> = bgra
        .chunks_exact(4)
        .take(w * h)
        .map(|p| ((p[2] as u32 * 299 + p[1] as u32 * 587 + p[0] as u32 * 114) / 1000) as i16)
        .collect();
    let line =
        |c: i16, a: i16, b: i16| (c + CONTRAST <= a && c + CONTRAST <= b) || (c >= a + CONTRAST && c >= b + CONTRAST);

    let mut hs = Vec::new();
    let mut vs = Vec::new();
    // Per column: where the vertical run started, the last row it was on.
    let mut v_run: Vec<Option<(usize, usize)>> = vec![None; w];
    for y in 0..h {
        let row = &luma[y * w..(y + 1) * w];
        // Horizontal: compare with the rows above and below.
        let mut run: Option<(usize, usize)> = None;
        if y >= REACH && y + REACH < h {
            let (up, down) = (&luma[(y - REACH) * w..], &luma[(y + REACH) * w..]);
            for x in 0..w {
                if line(row[x], up[x], down[x]) {
                    run = match run {
                        Some((s, e)) if x - e <= MAX_GAP + 1 => Some((s, x)),
                        prev => {
                            push_run(&mut hs, prev, y, MIN_H);
                            Some((x, x))
                        }
                    };
                }
            }
        }
        push_run(&mut hs, run, y, MIN_H);
        // Vertical: compare with the pixels to the left and right.
        for x in 0..w {
            let on = x >= REACH && x + REACH < w && line(row[x], row[x - REACH], row[x + REACH]);
            if on {
                v_run[x] = match v_run[x] {
                    Some((s, e)) if y - e <= MAX_GAP + 1 => Some((s, y)),
                    prev => {
                        push_run(&mut vs, prev, x, MIN_V);
                        Some((y, y))
                    }
                };
            } else if let Some((_, e)) = v_run[x] {
                if y - e > MAX_GAP + 1 {
                    push_run(&mut vs, v_run[x].take(), x, MIN_V);
                }
            }
        }
    }
    for (x, run) in v_run.into_iter().enumerate() {
        push_run(&mut vs, run, x, MIN_V);
    }
    Rules { h: merge(hs), v: merge(vs) }
}

fn push_run(out: &mut Vec<Seg>, run: Option<(usize, usize)>, pos: usize, min: usize) {
    if let Some((s, e)) = run {
        if e + 1 - s >= min {
            out.push(Seg { pos: pos as f32 + 0.5, from: s as f32, to: e as f32 + 1.0 });
        }
    }
}

/// Joins the runs of one line: a line two or three pixels thick is found on each of its rows,
/// and a line may be cut where something touches it.
fn merge(mut segs: Vec<Seg>) -> Vec<Seg> {
    segs.sort_by(|a, b| a.pos.total_cmp(&b.pos).then(a.from.total_cmp(&b.from)));
    let mut out: Vec<(Seg, f32)> = Vec::new(); // the line, and the highest row of it so far
    for s in segs {
        let gap = (MAX_GAP + 1) as f32;
        let hit =
            out.iter_mut().rev().find(|(m, last)| s.pos - *last <= 1.5 && s.from <= m.to + gap && m.from <= s.to + gap);
        match hit {
            Some((m, last)) => {
                // The middle of a thick line, weighted by length.
                let (lm, ls) = (m.len(), s.len());
                m.pos = (m.pos * lm + s.pos * ls) / (lm + ls);
                m.from = m.from.min(s.from);
                m.to = m.to.max(s.to);
                *last = s.pos;
            }
            None => out.push((s, s.pos)),
        }
    }
    out.into_iter().map(|(s, _)| s).collect()
}

/// A table drawn with lines: the y of every line between (and around) its rows, and the x of
/// every line between its columns (empty when only the rows have lines).
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub bounds: Bounds,
    pub rows: Vec<f32>,
    pub cols: Vec<f32>,
    /// The vertical lines, to tell which column edges a merged cell spans.
    pub v: Vec<Seg>,
}

impl Grid {
    /// The row that `y` is in.
    pub fn row_at(&self, y: f32) -> Option<usize> {
        if y < self.rows[0] || y > *self.rows.last()? {
            return None;
        }
        Some(self.rows.iter().filter(|&&r| r < y).count().saturating_sub(1).min(self.rows.len() - 2))
    }

    /// The column that `x` is in, in row `row`: a cell that spans several columns (its
    /// lines stop above and below it) belongs to the first.
    pub fn col_at(&self, row: usize, x: f32) -> usize {
        if self.cols.len() < 2 {
            return 0;
        }
        let mid = (self.rows[row] + self.rows[row + 1]) / 2.0;
        // The nearest line to the left that runs through this row.
        let left = self
            .v
            .iter()
            .filter(|s| s.pos <= x + TOL && s.from - TOL <= mid && mid <= s.to + TOL)
            .map(|s| s.pos)
            .fold(f32::MIN, f32::max);
        if left == f32::MIN {
            return 0;
        }
        let nearest = (0..self.cols.len())
            .min_by(|&a, &b| (self.cols[a] - left).abs().total_cmp(&(self.cols[b] - left).abs()))
            .unwrap_or(0);
        nearest.min(self.cols.len() - 2)
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.bounds = self.bounds.translate(dx, dy);
        self.rows.iter_mut().for_each(|y| *y += dy);
        self.cols.iter_mut().for_each(|x| *x += dx);
        for s in &mut self.v {
            s.pos += dx;
            s.from += dy;
            s.to += dy;
        }
    }

    pub fn columns(&self) -> usize {
        self.cols.len().saturating_sub(1).max(1)
    }
}

/// Positions closer together than `TOL` are one, sorted.
fn distinct(mut v: Vec<f32>) -> Vec<f32> {
    v.sort_by(f32::total_cmp);
    let mut out: Vec<f32> = Vec::new();
    for p in v {
        match out.last() {
            Some(&l) if p - l <= TOL * 2.0 => {}
            _ => out.push(p),
        }
    }
    out
}

/// The tables drawn with lines: lines that cross or touch each other, and horizontal lines of
/// the same length stacked above each other (tables with lines between the rows only). A grid
/// has at least two rows.
pub fn grids(rules: &Rules) -> Vec<Grid> {
    let meets =
        |h: &Seg, v: &Seg| v.pos >= h.from - TOL && v.pos <= h.to + TOL && h.pos >= v.from - TOL && h.pos <= v.to + TOL;
    // A vertical line that touches fewer than two horizontal ones is not a column border
    // (a letter, a scroll bar, a divider).
    let v: Vec<Seg> = rules.v.iter().filter(|v| rules.h.iter().filter(|h| meets(h, v)).count() >= 2).copied().collect();
    let h = &rules.h;
    let n = h.len() + v.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let join = |p: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (root(p, a), root(p, b));
        p[ra] = rb;
    };
    for (i, hs) in h.iter().enumerate() {
        for (j, vs) in v.iter().enumerate() {
            if meets(hs, vs) {
                join(&mut parent, i, h.len() + j);
            }
        }
        for (j, other) in h.iter().enumerate().skip(i + 1) {
            let slack = TOL * 2.0 + hs.len().max(other.len()) * 0.02;
            if (hs.from - other.from).abs() <= slack && (hs.to - other.to).abs() <= slack {
                join(&mut parent, i, j);
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, (Vec<Seg>, Vec<Seg>)> = Default::default();
    for i in 0..n {
        let g = groups.entry(root(&mut parent, i)).or_default();
        if i < h.len() {
            g.0.push(h[i]);
        } else {
            g.1.push(v[i - h.len()]);
        }
    }
    let mut out = Vec::new();
    for (_, (hs, vs)) in groups {
        if hs.is_empty() {
            continue;
        }
        // Rows: between the horizontal lines, and where the column lines end when the table
        // has no line at its top or bottom (where at least two of them end: one line that
        // runs on is a handle or a frame).
        let mut ys: Vec<f32> = hs.iter().map(|s| s.pos).collect();
        let ends: Vec<f32> = vs.iter().flat_map(|s| [s.from, s.to]).collect();
        ys.extend(ends.iter().filter(|&&e| ends.iter().filter(|&&o| (o - e).abs() <= TOL * 2.0).count() >= 2));
        let rows = distinct(ys);
        if rows.len() < 3 {
            continue;
        }
        let left = hs.iter().map(|s| s.from).chain(vs.iter().map(|s| s.pos)).fold(f32::MAX, f32::min);
        let right = hs.iter().map(|s| s.to).chain(vs.iter().map(|s| s.pos)).fold(f32::MIN, f32::max);
        let (top, bottom) = (rows[0], rows[rows.len() - 1]);
        // Columns: between the vertical lines and the ends of the table (a sheet cut off at
        // the edge of the screen has no line there). One column is no columns: the lines
        // are a frame.
        let cols =
            if vs.is_empty() { Vec::new() } else { distinct(vs.iter().map(|s| s.pos).chain([left, right]).collect()) };
        let cols = if cols.len() >= 3 { cols } else { Vec::new() };
        out.push(Grid { bounds: Bounds::new(left, top, right - left, bottom - top), rows, cols, v: vs });
    }
    // Small grids inside a bigger one (a box in a cell) are part of that one.
    let all = out.clone();
    out.retain(|g| {
        !all.iter().any(|o| {
            o != g
                && o.bounds.w * o.bounds.h > g.bounds.w * g.bounds.h
                && o.bounds.expand(TOL).contains(g.bounds.x, g.bounds.y)
                && o.bounds.expand(TOL).contains(g.bounds.right() - 0.1, g.bounds.bottom() - 0.1)
        })
    });
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A white BGRA image.
    pub fn white(w: usize, h: usize) -> Vec<u8> {
        vec![255u8; w * h * 4]
    }

    pub fn hline(img: &mut [u8], w: usize, y: usize, x0: usize, x1: usize, ink: u8) {
        for x in x0..x1 {
            img[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[ink; 3]);
        }
    }

    pub fn vline(img: &mut [u8], w: usize, x: usize, y0: usize, y1: usize, ink: u8) {
        for y in y0..y1 {
            img[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[ink; 3]);
        }
    }

    /// A grid with lines at `ys` and `xs`, each running the full table.
    pub fn table_image(w: usize, h: usize, xs: &[usize], ys: &[usize]) -> Vec<u8> {
        let mut img = white(w, h);
        let (x0, x1) = (xs[0], *xs.last().unwrap() + 1);
        let (y0, y1) = (ys[0], *ys.last().unwrap() + 1);
        for &y in ys {
            hline(&mut img, w, y, x0, x1, 40);
        }
        for &x in xs {
            vline(&mut img, w, x, y0, y1, 40);
        }
        img
    }

    #[test]
    fn finds_thin_lines_and_leaves_out_short_ones() {
        let (w, h) = (300, 200);
        let mut img = white(w, h);
        hline(&mut img, w, 50, 10, 290, 0);
        // Two pixels thick, light grey (a spreadsheet's grid).
        hline(&mut img, w, 100, 10, 290, 225);
        hline(&mut img, w, 101, 10, 290, 225);
        vline(&mut img, w, 150, 20, 180, 0);
        // A stroke of a letter: too short.
        hline(&mut img, w, 150, 10, 22, 0);
        // A filled block: no line.
        for y in 160..190 {
            hline(&mut img, w, y, 200, 290, 0);
        }
        let r = find(&img, w as u32, h as u32);
        let ys: Vec<f32> = r.h.iter().map(|s| s.pos).collect();
        assert_eq!(ys.len(), 2, "{:?}", r.h);
        assert!((ys[0] - 50.5).abs() < 0.01);
        assert!((ys[1] - 101.0).abs() < 0.01);
        assert_eq!((r.h[0].from, r.h[0].to), (10.0, 290.0));
        assert_eq!(r.v.len(), 1);
        assert_eq!((r.v[0].pos, r.v[0].from, r.v[0].to), (150.5, 20.0, 180.0));
    }

    #[test]
    fn light_lines_on_dark_are_found() {
        let (w, h) = (200, 100);
        let mut img = vec![30u8; w * h * 4];
        hline(&mut img, w, 40, 0, 200, 90);
        assert_eq!(find(&img, w as u32, h as u32).h.len(), 1);
    }

    #[test]
    fn a_line_cut_by_a_letter_is_one_line() {
        let (w, h) = (300, 100);
        let mut img = white(w, h);
        hline(&mut img, w, 50, 10, 140, 0);
        hline(&mut img, w, 50, 143, 290, 0);
        let r = find(&img, w as u32, h as u32);
        assert_eq!(r.h.len(), 1);
        assert_eq!((r.h[0].from, r.h[0].to), (10.0, 290.0));
    }

    #[test]
    fn crossing_lines_make_a_grid() {
        let img = table_image(400, 300, &[10, 150, 250, 390], &[20, 60, 140, 180]);
        let g = grids(&find(&img, 400, 300));
        assert_eq!(g.len(), 1);
        let g = &g[0];
        assert_eq!(g.rows.len(), 4);
        assert_eq!(g.cols.len(), 4);
        assert_eq!(g.row_at(100.0), Some(1));
        assert_eq!(g.row_at(10.0), None);
        assert_eq!(g.col_at(1, 200.0), 1);
        assert_eq!(g.col_at(1, 389.0), 2);
    }

    #[test]
    fn a_merged_cell_belongs_to_its_first_column() {
        // The line between columns 1 and 2 stops under the header.
        let (w, h) = (400, 300);
        let mut img = white(w, h);
        for y in [20, 60, 100, 140] {
            hline(&mut img, w, y, 10, 391, 40);
        }
        for x in [10, 150, 390] {
            vline(&mut img, w, x, 20, 141, 40);
        }
        vline(&mut img, w, 270, 60, 141, 40);
        let g = grids(&find(&img, w as u32, h as u32)).remove(0);
        assert_eq!(g.cols.len(), 4);
        // Header row: right of the start of the merged cell, also past where the line would be.
        assert_eq!(g.col_at(0, 300.0), 1);
        assert_eq!(g.col_at(1, 300.0), 2);
    }

    #[test]
    fn stacked_lines_of_one_length_make_a_grid_without_columns() {
        let (w, h) = (400, 300);
        let mut img = white(w, h);
        for y in [20, 60, 100] {
            hline(&mut img, w, y, 10, 390, 40);
        }
        let g = grids(&find(&img, w as u32, h as u32));
        assert_eq!(g.len(), 1);
        assert!(g[0].cols.is_empty());
        assert_eq!(g[0].rows.len(), 3);
    }

    #[test]
    fn a_frame_or_loose_lines_are_no_grid() {
        let (w, h) = (400, 300);
        let mut img = white(w, h);
        // A box: one row.
        hline(&mut img, w, 20, 10, 390, 40);
        hline(&mut img, w, 120, 10, 390, 40);
        vline(&mut img, w, 10, 20, 121, 40);
        vline(&mut img, w, 389, 20, 121, 40);
        // Lines of different lengths.
        hline(&mut img, w, 200, 10, 100, 40);
        hline(&mut img, w, 230, 10, 300, 40);
        hline(&mut img, w, 260, 50, 390, 40);
        assert!(grids(&find(&img, w as u32, h as u32)).is_empty());
    }

    #[test]
    #[ignore]
    fn timing_4k() {
        let (w, h) = (3840usize, 2160usize);
        let mut img = white(w, h);
        for y in (100..2000).step_by(40) {
            hline(&mut img, w, y, 100, 3700, 40);
        }
        let t = std::time::Instant::now();
        let r = find(&img, w as u32, h as u32);
        let g = grids(&r);
        eprintln!("4K: {:?}, {} lines, {} grids", t.elapsed(), r.h.len(), g.len());
    }
}
