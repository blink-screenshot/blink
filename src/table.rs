//! Recognizing a table in read text, so copied text can be pasted as a table.
//!
//! The OCR engine only returns lines of words with their positions; it knows nothing about
//! tables. A table is found from the layout alone:
//!
//! 1. words are grouped into rows by their vertical middle (the engine may split one table
//!    row into several lines, or join cells into one line);
//! 2. within a row, a gap much wider than a space starts a new cell;
//! 3. columns are the vertical strips that the cells of nearly every row leave empty
//!    ("gutters"); a gutter also splits two words that are closer together than step 2 needs;
//! 4. every cell goes to the column its left edge is in.
//!
//! It only counts as a table when most rows have more than one cell and cells are short:
//! normal text, two columns of prose and a page with a menu next to it stay plain text.
//! Copied tables go on the clipboard twice: as HTML (Word, Outlook, Teams, Excel make a real
//! table of it) and as tab-separated text (Excel puts that in cells too, Notepad shows tabs).

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

/// The table that `words` form, if they form one (see the module comment).
pub fn detect(words: &[&Word]) -> Option<Table> {
    if words.len() < 4 {
        return None;
    }
    let h = median(words.iter().map(|w| w.rect.h).collect()).max(1.0);
    let rows = rows_of(words, h);
    if rows.len() < 2 {
        return None;
    }
    // Columns from the rows that clearly have more than one cell.
    let first: Vec<Vec<Cell>> = rows.iter().map(|r| cells_of(r, h * CELL_GAP, &[])).collect();
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
    for row in &rows {
        let mut out: Vec<Vec<&Word>> = vec![Vec::new(); ncols];
        for cell in cells_of(row, h * CELL_GAP, &gutters) {
            let col = gutters.iter().filter(|&&(l, r)| (l + r) / 2.0 < cell.left()).count();
            out[col].extend(cell.words);
        }
        table.push(out);
    }
    // Most rows have more than one cell (a title or a note may have one).
    let filled_rows = table.iter().filter(|r| r.iter().filter(|c| !c.is_empty()).count() > 1).count();
    if filled_rows < 2 || filled_rows * 2 < table.len() {
        return None;
    }
    // Every column is used, and holds short cells, not prose.
    for col in 0..ncols {
        let counts: Vec<f32> = table.iter().map(|r| r[col].len()).filter(|&n| n > 0).map(|n| n as f32).collect();
        if counts.is_empty() || median(counts) > MAX_WORDS_PER_CELL as f32 {
            return None;
        }
    }
    let rows = table
        .into_iter()
        .map(|r| r.into_iter().map(|c| crate::ocr::join_words(c.iter().map(|w| w.text.as_str()))).collect())
        .collect();
    Some(Table { rows })
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
        let t = detect(&words(&lines)).expect("a table");
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
        let t = detect(&words(&lines)).unwrap();
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
        let t = detect(&words(&lines)).unwrap();
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
        let t = detect(&words(&lines)).unwrap();
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
        let t = detect(&words(&lines)).unwrap();
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
        assert_eq!(detect(&words(&lines)), None);
        // One row with a wide gap is not a table either.
        let one = [row(&[("Total", 0.0), ("12", 300.0)], 0.0), line_at("thanks for your order", 0.0, 20.0, 12.0)];
        assert_eq!(detect(&words(&one)), None);
    }

    #[test]
    fn prose_in_two_columns_is_not_a_table() {
        let left = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed";
        let right = "do eiusmod tempor incididunt ut labore et dolore magna";
        let lines: Vec<Line> = (0..6).map(|i| row(&[(left, 0.0), (right, 700.0)], i as f32 * 20.0)).collect();
        assert_eq!(detect(&words(&lines)), None);
    }

    #[test]
    fn menu_next_to_prose_is_not_a_table() {
        let text = "This is a paragraph of text that runs along next to the menu";
        let lines: Vec<Line> = ["Home", "Products", "About us", "Contact"]
            .iter()
            .enumerate()
            .map(|(i, m)| row(&[(m, 0.0), (text, 200.0)], i as f32 * 20.0))
            .collect();
        assert_eq!(detect(&words(&lines)), None);
    }

    #[test]
    fn html_is_escaped() {
        let w = [word("<b>&", 0.0, 0.0, 40.0, 12.0), word("x", 200.0, 0.0, 10.0, 12.0)];
        let w2 = [word("a", 0.0, 20.0, 10.0, 12.0), word("b", 200.0, 20.0, 10.0, 12.0)];
        let all: Vec<&Word> = w.iter().chain(&w2).collect();
        let t = detect(&all).unwrap();
        assert!(t.html().contains("<td>&lt;b&gt;&amp;</td>"));
    }
}
