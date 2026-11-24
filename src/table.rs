//! Recognizing a table in read text, so copied text can be pasted as a table.
//!
//! The OCR engine only returns lines of words with their positions; it knows nothing about
//! tables. When the table is drawn with lines (`crate::grid`), those lines say where every row
//! and column is: a cell may hold several lines of text, and a header may start left of the
//! values under it. Otherwise a table is found from the layout alone:
//!
//! 1. words are grouped into rows by their vertical middle (the engine may split one table
//!    row into several lines, or join cells into one line);
//! 2. within a row, a gap much wider than a space starts a new cell;
//! 3. columns are the vertical strips that the cells of nearly every row leave empty
//!    ("gutters"); a gutter also splits two words that are closer together than step 2 needs;
//! 4. every cell goes to the column its left edge is in (a cell that starts in a gutter and
//!    runs past it, to the column on its right).
//!
//! It only counts as a table when most rows have more than one cell and cells are short:
//! normal text, two columns of prose and a page with a menu next to it stay plain text.
//! Copied tables go on the clipboard twice: as HTML (Word, Outlook, Teams, Excel make a real
//! table of it) and as tab-separated text (Excel puts that in cells too, Notepad shows tabs).

use crate::grid::Grid;
use crate::ocr::Word;

/// A gap between two words, in word heights, from which they are in different cells.
const CELL_GAP: f32 = 1.0;
/// The narrowest gutter between two columns, in word heights.
const MIN_GUTTER: f32 = 0.3;
/// A cell (in a column) holds at most this many words in the middle: longer is prose.
const MAX_WORDS_PER_CELL: usize = 8;

/// The cells of a table, row by row; every row has the same number of cells, empty ones are "".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub rows: Vec<Vec<String>>,
}

impl Table {
    pub fn columns(&self) -> usize {
        self.rows.first().map_or(0, Vec::len)
    }

    /// Tab-separated, one line per row: what Excel and Notepad get.
    pub fn tsv(&self) -> String {
        self.rows.iter().map(|r| r.join("\t")).collect::<Vec<_>>().join("\n")
    }

    /// A plain HTML table: what Word, Outlook and Teams paste as a table.
    pub fn html(&self) -> String {
        let mut out = String::from("<table>");
        for row in &self.rows {
            out.push_str("<tr>");
            for cell in row {
                out.push_str("<td>");
                out.push_str(&escape(cell));
                out.push_str("</td>");
            }
            out.push_str("</tr>");
        }
        out.push_str("</table>");
        out
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

/// The words of a run in a row, left to right.
struct Cell<'a> {
    words: Vec<&'a Word>,
}

impl Cell<'_> {
    fn left(&self) -> f32 {
        self.words[0].rect.x
    }
    fn right(&self) -> f32 {
        self.words.iter().map(|w| w.rect.right()).fold(f32::MIN, f32::max)
    }
}

/// Groups words into rows (top to bottom), each left to right.
fn rows_of<'a>(words: &[&'a Word], h: f32) -> Vec<Vec<&'a Word>> {
    let mut sorted: Vec<&Word> = words.to_vec();
    sorted.sort_by(|a, b| a.rect.cy().total_cmp(&b.rect.cy()));
    let mut rows: Vec<(f32, Vec<&Word>)> = Vec::new();
    for w in sorted {
        match rows.last_mut() {
            Some((cy, row)) if (w.rect.cy() - *cy).abs() <= h * 0.5 => {
                row.push(w);
                *cy = row.iter().map(|w| w.rect.cy()).sum::<f32>() / row.len() as f32;
            }
            _ => rows.push((w.rect.cy(), vec![w])),
        }
    }
    rows.into_iter()
        .map(|(_, mut row)| {
            row.sort_by(|a, b| a.rect.x.total_cmp(&b.rect.x));
            row
        })
        .collect()
}

/// A vertical strip between two columns: `(left, right)`.
type Gutter = (f32, f32);

/// Splits a row into cells where two words are more than `gap` apart, or where the next word
/// starts at the far side of a gutter (columns can be closer together than `gap` in one row
/// when the other rows show the column clearly).
fn cells_of<'a>(row: &[&'a Word], gap: f32, gutters: &[Gutter]) -> Vec<Cell<'a>> {
    let mut cells: Vec<Cell> = Vec::new();
    for &w in row {
        let split = match cells.last() {
            None => true,
            Some(c) => {
                let (end, x) = (c.right(), w.rect.x);
                x - end > gap || gutters.iter().any(|&(l, r)| end < r && x > l && x >= r - gap * 0.5)
            }
        };
        if split {
            cells.push(Cell { words: vec![w] });
        } else if let Some(c) = cells.last_mut() {
            c.words.push(w);
        }
    }
    cells
}

/// The gutters: x ranges that the cells of (nearly) every row leave empty, left to right.
fn gutters(cells: &[&Cell], rows: usize, h: f32) -> Vec<Gutter> {
    let mut events: Vec<(f32, i32)> = Vec::new();
    for c in cells {
        events.push((c.left(), 1));
        events.push((c.right(), -1));
    }
    // Ends before starts at the same place: two cells that touch leave no gutter, but are
    // not counted as overlapping either.
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    // One cell in eight rows may run through a gutter (a long title, a misread word).
    let allowed = (rows / 8) as i32;
    let (Some(first), Some(last)) = (events.first().map(|e| e.0), events.last().map(|e| e.0)) else {
        return Vec::new();
    };
    let mut bounds = Vec::new();
    let mut count = 0;
    let mut open: Option<f32> = None;
    for (x, d) in events {
        let was_free = count <= allowed;
        count += d;
        let free = count <= allowed;
        if !was_free && free {
            open = Some(x);
        } else if was_free && !free {
            if let Some(start) = open.take() {
                if x - start >= h * MIN_GUTTER && start > first && x < last {
                    bounds.push((start, x));
                }
            }
        }
    }
    bounds
}

/// The column of a cell: right of every gutter it starts after. A cell that starts inside a
/// gutter and runs past it (a header written from the left over values in the middle of
/// their column) belongs to the column on the right of it.
fn col_of(cell: &Cell, gutters: &[Gutter]) -> usize {
    let (left, right) = (cell.left(), cell.right());
    gutters.iter().filter(|&&(l, r)| left >= r || (left > l && right > r)).count()
}

/// Puts the words of every band (one table row: one or more lines of text) in columns, from
/// the gutters between them. `None` when the words have no columns.
fn columns_of<'a>(bands: &[Vec<&'a Word>], h: f32) -> Option<Vec<Vec<Vec<&'a Word>>>> {
    let lines: Vec<Vec<Vec<&Word>>> = bands.iter().map(|b| rows_of(b, h)).collect();
    // Columns from the lines that clearly have more than one cell.
    let first: Vec<Vec<Cell>> = lines.iter().flatten().map(|l| cells_of(l, h * CELL_GAP, &[])).collect();
    let multi: Vec<&Cell> = first.iter().filter(|c| c.len() > 1).flatten().collect();
    let multi_rows = first.iter().filter(|c| c.len() > 1).count();
    if multi_rows < 2 {
        return None;
    }
    let gutters = gutters(&multi, multi_rows, h);
    if gutters.is_empty() || gutters.len() > 40 {
        return None;
    }
    let ncols = gutters.len() + 1;
    let mut table: Vec<Vec<Vec<&Word>>> = Vec::new();
    for band in &lines {
        let mut out: Vec<Vec<&Word>> = vec![Vec::new(); ncols];
        for line in band {
            for cell in cells_of(line, h * CELL_GAP, &gutters) {
                out[col_of(&cell, &gutters)].extend(cell.words);
            }
        }
        table.push(out);
    }
    Some(table)
}

/// Whether cells found from gutters look like a table: most rows have more than one cell (a
/// title or a note may have one), every column is used and holds short cells, not prose.
fn looks_like_table(table: &[Vec<Vec<&Word>>], words_per_cell: usize) -> bool {
    let filled_rows = table.iter().filter(|r| r.iter().filter(|c| !c.is_empty()).count() > 1).count();
    if filled_rows < 2 || filled_rows * 2 < table.len() {
        return false;
    }
    let ncols = table.first().map_or(0, Vec::len);
    (0..ncols).all(|col| {
        let counts: Vec<f32> = table.iter().map(|r| r[col].len()).filter(|&n| n > 0).map(|n| n as f32).collect();
        !counts.is_empty() && median(counts) <= words_per_cell as f32
    })
}

/// The text of a cell: its lines top to bottom, each left to right, on one line.
fn cell_text(words: &[&Word], h: f32) -> String {
    let lines = rows_of(words, h);
    crate::ocr::join_words(lines.iter().flatten().map(|w| w.text.as_str()))
}

fn to_table(cells: Vec<Vec<Vec<&Word>>>, h: f32) -> Table {
    Table { rows: cells.into_iter().map(|r| r.into_iter().map(|c| cell_text(&c, h)).collect()).collect() }
}

/// The table that `words` form, if they form one: in a table drawn with lines (`grids`) when
/// most of the words are in one, else from the layout alone (see the module comment).
pub fn detect(words: &[&Word], grids: &[Grid]) -> Option<Table> {
    if words.len() < 2 {
        return None;
    }
    let h = median(words.iter().map(|w| w.rect.h).collect()).max(1.0);
    if let Some(t) = in_grid(words, grids, h) {
        return Some(t);
    }
    if words.len() < 4 {
        return None;
    }
    let rows = rows_of(words, h);
    if rows.len() < 2 {
        return None;
    }
    let table = columns_of(&rows, h)?;
    looks_like_table(&table, MAX_WORDS_PER_CELL).then(|| to_table(table, h))
}

/// The word's middle is in the grid.
fn inside(g: &Grid, w: &Word) -> bool {
    g.bounds.expand(2.0).contains(w.rect.cx(), w.rect.cy())
}

/// The table of the words in a table drawn with lines: every word goes to the row between
/// the lines around it, and to the column between the lines left and right of it (or, when
/// only the rows have lines, to the column the gutters say). A cell may hold several lines
/// of text. Words above or below the grid (a title, a note) become rows of their own.
fn in_grid(words: &[&Word], grids: &[Grid], h: f32) -> Option<Table> {
    let (grid, count) =
        grids.iter().map(|g| (g, words.iter().filter(|w| inside(g, w)).count())).max_by_key(|(_, n)| *n)?;
    // Most of the picked text is in this table.
    if count < 2 || count * 3 < words.len() * 2 {
        return None;
    }
    let nrows = grid.rows.len() - 1;
    let mut cells: Vec<Vec<Vec<&Word>>> = if grid.cols.len() >= 2 {
        let mut cells = vec![vec![Vec::new(); grid.columns()]; nrows];
        for w in words.iter().filter(|w| inside(grid, w)) {
            if let Some(r) = grid.row_at(w.rect.cy()) {
                cells[r][grid.col_at(r, w.rect.cx())].push(*w);
            }
        }
        cells
    } else {
        let mut bands: Vec<Vec<&Word>> = vec![Vec::new(); nrows];
        for w in words.iter().filter(|w| inside(grid, w)) {
            if let Some(r) = grid.row_at(w.rect.cy()) {
                bands[r].push(*w);
            }
        }
        bands.retain(|b| !b.is_empty());
        let cells = columns_of(&bands, h)?;
        if !looks_like_table(&cells, MAX_WORDS_PER_CELL * 3) {
            return None;
        }
        cells
    };
    // Only the part with text: a selection of a few cells of a big sheet is those cells.
    cells.retain(|r| r.iter().any(|c| !c.is_empty()));
    let used: Vec<bool> = (0..cells.first()?.len()).map(|c| cells.iter().any(|r| !r[c].is_empty())).collect();
    let (first, last) = (used.iter().position(|&u| u)?, used.iter().rposition(|&u| u)?);
    for r in &mut cells {
        r.truncate(last + 1);
        r.drain(..first);
    }
    let ncols = last + 1 - first;
    if cells.len() < 2
        || ncols < 2
        || cells.iter().filter(|r| r.iter().filter(|c| !c.is_empty()).count() > 1).count() == 0
    {
        return None;
    }
    // Words outside the grid, as rows of their own above or below it.
    let outside: Vec<&Word> = words.iter().filter(|w| !inside(grid, w)).copied().collect();
    let mut above = Vec::new();
    let mut below = Vec::new();
    for line in rows_of(&outside, h) {
        let mut row = vec![Vec::new(); ncols];
        let top = line[0].rect.cy() < grid.bounds.y;
        row[0] = line;
        if top {
            above.push(row);
        } else {
            below.push(row);
        }
    }
    let all: Vec<Vec<Vec<&Word>>> = above.into_iter().chain(cells).chain(below).collect();
    Some(to_table(all, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::tests::{line_at, word};
    use crate::ocr::Line;

    fn words(lines: &[Line]) -> Vec<&Word> {
        lines.iter().flat_map(|l| &l.words).collect()
    }

    /// A row of cells: each (text, x), words 10 px per character, 12 px high.
    fn row(cells: &[(&str, f32)], y: f32) -> Line {
        Line { words: cells.iter().flat_map(|(t, x)| line_at(t, *x, y, 12.0).words).collect() }
    }

    #[test]
    fn finds_a_simple_table() {
        let lines = [
            row(&[("Name", 0.0), ("Amount", 200.0), ("Due date", 320.0)], 0.0),
            row(&[("Jane Doe", 0.0), ("120", 230.0), ("1 May", 320.0)], 20.0),
            row(&[("John Smith", 0.0), ("80", 240.0), ("12 June", 320.0)], 40.0),
        ];
        let t = detect(&words(&lines), &[]).expect("a table");
        assert_eq!(t.columns(), 3);
        assert_eq!(t.tsv(), "Name\tAmount\tDue date\nJane Doe\t120\t1 May\nJohn Smith\t80\t12 June");
    }

    #[test]
    fn empty_cells_stay_in_their_column() {
        let lines = [
            row(&[("Item", 0.0), ("Q1", 200.0), ("Q2", 300.0)], 0.0),
            row(&[("Coffee", 0.0), ("4", 300.0)], 20.0),
            row(&[("Tea", 0.0), ("2", 200.0), ("3", 300.0)], 40.0),
        ];
        let t = detect(&words(&lines), &[]).unwrap();
        assert_eq!(t.rows[1], vec!["Coffee", "", "4"]);
        assert_eq!(
            t.html(),
            "<table><tr><td>Item</td><td>Q1</td><td>Q2</td></tr><tr><td>Coffee</td><td></td>\
             <td>4</td></tr><tr><td>Tea</td><td>2</td><td>3</td></tr></table>"
        );
    }

    #[test]
    fn rows_split_into_several_lines_are_joined() {
        // The engine read every cell as its own line, and not in row order.
        let lines =
            [row(&[("B", 200.0)], 0.0), row(&[("A", 0.0)], 1.0), row(&[("2", 200.0)], 21.0), row(&[("1", 0.0)], 20.0)];
        let t = detect(&words(&lines), &[]).unwrap();
        assert_eq!(t.tsv(), "A\tB\n1\t2");
    }

    #[test]
    fn a_column_gap_narrower_than_a_cell_gap_still_splits() {
        // Column two starts 8 px after the longest cell of column one (less than a word
        // height): the other rows show where the column is.
        let lines = [
            row(&[("Longest", 0.0), ("x", 78.0)], 0.0),
            row(&[("A", 0.0), ("y", 78.0)], 20.0),
            row(&[("B", 0.0), ("z", 78.0)], 40.0),
        ];
        let t = detect(&words(&lines), &[]).unwrap();
        assert_eq!(t.rows[0], vec!["Longest", "x"]);
    }

    #[test]
    fn a_title_over_the_table_is_kept() {
        let lines = [
            row(&[("Sales per region in 2026", 0.0)], 0.0),
            row(&[("North", 0.0), ("12", 300.0)], 20.0),
            row(&[("South", 0.0), ("9", 300.0)], 40.0),
            row(&[("East", 0.0), ("4", 300.0)], 60.0),
        ];
        let t = detect(&words(&lines), &[]).unwrap();
        assert_eq!(t.rows[0], vec!["Sales per region in 2026", ""]);
        assert_eq!(t.rows.len(), 4);
    }

    #[test]
    fn plain_text_is_not_a_table() {
        let lines = [
            line_at("The quick brown fox jumps over the lazy dog", 0.0, 0.0, 12.0),
            line_at("and runs off into the woods again", 0.0, 20.0, 12.0),
            line_at("never to be seen again", 0.0, 40.0, 12.0),
        ];
        assert_eq!(detect(&words(&lines), &[]), None);
        // One row with a wide gap is not a table either.
        let one = [row(&[("Total", 0.0), ("12", 300.0)], 0.0), line_at("thanks for your order", 0.0, 20.0, 12.0)];
        assert_eq!(detect(&words(&one), &[]), None);
    }

    #[test]
    fn prose_in_two_columns_is_not_a_table() {
        let left = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed";
        let right = "do eiusmod tempor incididunt ut labore et dolore magna";
        let lines: Vec<Line> = (0..6).map(|i| row(&[(left, 0.0), (right, 700.0)], i as f32 * 20.0)).collect();
        assert_eq!(detect(&words(&lines), &[]), None);
    }

    #[test]
    fn menu_next_to_prose_is_not_a_table() {
        let text = "This is a paragraph of text that runs along next to the menu";
        let lines: Vec<Line> = ["Home", "Products", "About us", "Contact"]
            .iter()
            .enumerate()
            .map(|(i, m)| row(&[(m, 0.0), (text, 200.0)], i as f32 * 20.0))
            .collect();
        assert_eq!(detect(&words(&lines), &[]), None);
    }

    #[test]
    fn html_is_escaped() {
        let w = [word("<b>&", 0.0, 0.0, 40.0, 12.0), word("x", 200.0, 0.0, 10.0, 12.0)];
        let w2 = [word("a", 0.0, 20.0, 10.0, 12.0), word("b", 200.0, 20.0, 10.0, 12.0)];
        let all: Vec<&Word> = w.iter().chain(&w2).collect();
        let t = detect(&all, &[]).unwrap();
        assert!(t.html().contains("<td>&lt;b&gt;&amp;</td>"));
    }

    /// A table with borders: two lines of text in some cells of the first column, header
    /// cells with several lines, centred crosses under headers that start at the left.
    fn plans() -> (Vec<Word>, Vec<Grid>) {
        let xs = [20, 400, 700, 950, 1200, 1560];
        let ys = [30, 250, 310, 410, 470, 530, 590, 690, 790, 850];
        let img = crate::grid::tests::table_image(1600, 900, &xs, &ys);
        let grids = crate::grid::grids(&crate::grid::find(&img, 1600, 900));
        let w = |t: &str, x: f32, y: f32| word(t, x, y, t.chars().count() as f32 * 14.0, 30.0);
        let mut out = vec![
            w("Feature", 40.0, 80.0),
            w("Plan", 40.0, 200.0),
            w("Starter", 420.0, 40.0),
            w("bundle", 530.0, 40.0),
            w("with", 420.0, 92.0),
            w("storage", 488.0, 92.0),
            w("Team", 720.0, 40.0),
            w("plan", 788.0, 40.0),
            w("with", 720.0, 92.0),
            w("sharing", 788.0, 92.0),
            w("monthly", 720.0, 144.0),
            w("Business-", 970.0, 40.0),
            w("annually", 970.0, 92.0),
            w("Organisation-wide", 1220.0, 40.0),
        ];
        let rows: [(&[&str], f32, &[usize]); 8] = [
            (&["Shared folders"], 263.0, &[0]),
            (&["Version", "history"], 323.0, &[0]),
            (&["Offline access"], 423.0, &[0]),
            (&["Audit log *"], 483.0, &[1, 2, 3]),
            (&["Single sign-on"], 543.0, &[0, 1, 2, 3]),
            (&["Priority", "support"], 603.0, &[0, 1, 2, 3]),
            (&["Data", "export"], 703.0, &[0, 1, 2, 3]),
            (&["API access"], 803.0, &[0]),
        ];
        let cross = [550.0, 825.0, 1075.0, 1380.0];
        for (lines, y, xs) in rows {
            for (i, l) in lines.iter().enumerate() {
                let mut x = 40.0;
                for t in l.split(' ') {
                    out.push(w(t, x, y + i as f32 * 52.0));
                    x += t.chars().count() as f32 * 14.0 + 12.0;
                }
            }
            for &c in xs {
                out.push(word("X", cross[c] - 11.0, y, 22.0, 30.0));
            }
        }
        (out, grids)
    }

    #[test]
    fn rows_and_columns_follow_the_borders() {
        let (words, grids) = plans();
        assert_eq!(grids.len(), 1);
        let refs: Vec<&Word> = words.iter().collect();
        let t = detect(&refs, &grids).expect("a table");
        assert_eq!(
            t.tsv(),
            "Feature Plan\tStarter bundle with storage\tTeam plan with sharing monthly\tBusiness- annually\tOrganisation-wide\n\
             Shared folders\tX\t\t\t\n\
             Version history\tX\t\t\t\n\
             Offline access\tX\t\t\t\n\
             Audit log *\t\tX\tX\tX\n\
             Single sign-on\tX\tX\tX\tX\n\
             Priority support\tX\tX\tX\tX\n\
             Data export\tX\tX\tX\tX\n\
             API access\tX\t\t\t"
        );
        // A few cells of it: those cells.
        let some: Vec<&Word> = words.iter().filter(|w| w.rect.y > 530.0 && w.rect.y < 650.0).collect();
        let t = detect(&some, &grids).unwrap();
        assert_eq!(t.rows[0], vec!["Single sign-on", "X", "X", "X", "X"]);
        assert_eq!(t.rows[1], vec!["Priority", "X", "X", "X", "X"]);
    }

    #[test]
    fn a_header_left_of_its_column_without_borders() {
        // The same table without borders: rows cannot be told apart, but the header that
        // starts left of the crosses under it is in their column.
        let (words, _) = plans();
        let refs: Vec<&Word> = words.iter().collect();
        let t = detect(&refs, &[]).expect("a table");
        assert_eq!(t.columns(), 5);
        assert_eq!(t.rows[0], vec!["", "Starter bundle", "Team plan", "Business-", "Organisation-wide"]);
    }

    #[test]
    fn rows_follow_lines_between_rows_only() {
        // Lines between the rows, none between the columns: columns from the gutters.
        let (words, _) = plans();
        let (w, h) = (1600usize, 900usize);
        let mut img = crate::grid::tests::white(w, h);
        for y in [30, 250, 310, 410, 470, 530, 590, 690, 790, 850] {
            crate::grid::tests::hline(&mut img, w, y, 20, 1561, 40);
        }
        let grids = crate::grid::grids(&crate::grid::find(&img, w as u32, h as u32));
        assert!(grids[0].cols.is_empty());
        let refs: Vec<&Word> = words.iter().collect();
        let t = detect(&refs, &grids).expect("a table");
        assert_eq!(t.rows.len(), 9);
        assert_eq!(t.rows[2], vec!["Version history", "X", "", "", ""]);
        assert_eq!(t.rows[0][4], "Organisation-wide");
    }
}
