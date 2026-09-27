//! Plain-text tables whose columns stay aligned with Japanese (double-width) text in the console.

/// Columns a string takes in a console: East Asian wide and fullwidth characters take two,
/// combining marks and controls none, everything else one. Ambiguous-width characters count as one.
pub fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn char_width(c: char) -> usize {
    match u32::from(c) {
        0x00..=0x1F | 0x7F..=0x9F => 0,
        0x0300..=0x036F | 0x200B..=0x200F | 0x3099..=0x309A | 0xFE00..=0xFE0F => 0,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x2FFFD
        | 0x30000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// Cuts `s` to at most `max` columns, ending in ASCII `...` when cut (U+2026 is double-width
/// in Japanese consoles). Cuts at the last space instead of mid-word when that keeps at least
/// half of the text.
pub fn truncate(s: &str, max: usize) -> String {
    const ELLIPSIS: &str = "...";
    if display_width(s) <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(ELLIPSIS.len());
    let (mut end, mut width) = (0, 0);
    for (i, c) in s.char_indices() {
        width += char_width(c);
        if width > budget {
            break;
        }
        end = i + c.len_utf8();
    }
    let mut kept = &s[..end];
    if s[end..].starts_with(|c: char| !c.is_whitespace())
        && let Some(space) = kept.rfind(' ')
        && display_width(&kept[..space]) >= budget / 2
    {
        kept = &kept[..space];
    }
    format!("{}{ELLIPSIS}", kept.trim_end())
}

/// A table printed with two spaces between columns and no trailing spaces.
#[derive(Debug, Clone)]
pub struct Table {
    headers: Vec<String>,
    max_widths: Vec<Option<usize>>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(headers: &[&str]) -> Self {
        Self {
            headers: headers.iter().map(|h| h.to_string()).collect(),
            max_widths: vec![None; headers.len()],
            rows: Vec::new(),
        }
    }

    /// Truncates the cells of `column` to `width` columns.
    pub fn max_width(mut self, column: usize, width: usize) -> Self {
        self.max_widths[column] = Some(width);
        self
    }

    /// Adds a row; missing cells are empty and extra cells are dropped.
    pub fn row(&mut self, mut cells: Vec<String>) {
        cells.resize(self.headers.len(), String::new());
        self.rows.push(cells);
    }

    /// Renders every line with `indent` in front, each ending in `\n`.
    pub fn render(&self, indent: &str) -> String {
        let fit = |column: usize, cell: &str| match self.max_widths[column] {
            Some(max) => truncate(cell, max),
            None => cell.to_string(),
        };
        let lines: Vec<Vec<String>> = std::iter::once(&self.headers)
            .chain(&self.rows)
            .map(|cells| {
                cells
                    .iter()
                    .enumerate()
                    .map(|(column, cell)| fit(column, cell))
                    .collect()
            })
            .collect();
        let mut widths = vec![0; self.headers.len()];
        for cells in &lines {
            for (width, cell) in widths.iter_mut().zip(cells) {
                *width = (*width).max(display_width(cell));
            }
        }
        let mut out = String::new();
        for cells in &lines {
            let mut line = String::from(indent);
            for (column, cell) in cells.iter().enumerate() {
                if column > 0 {
                    line.push_str("  ");
                }
                line.push_str(cell);
                line.extend(std::iter::repeat_n(
                    ' ',
                    widths[column] - display_width(cell),
                ));
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths() {
        assert_eq!(display_width("Keychron Receiver"), 17);
        assert_eq!(display_width("日本語 PS/2"), 11);
        assert_eq!(display_width("HID キーボード デバイス"), 23);
        assert_eq!(display_width("ｶﾀｶﾅ"), 4);
        assert_eq!(display_width("Ａ"), 2);
        assert_eq!(display_width("e\u{301}"), 1);
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactly10!", 10), "exactly10!");
        assert_eq!(truncate("Keychron Receiver", 10), "Keychro...");
        // Cut at the last space when that keeps at least half, else mid-word.
        assert_eq!(truncate("Keychron Receiver 2", 16), "Keychron...");
        assert_eq!(truncate("a bcdefghijkl", 10), "a bcdef...");
        assert_eq!(
            truncate("日本語 PS/2 キーボード (106/109 キー Ctrl+英数)", 28),
            "日本語 PS/2 キーボード..."
        );
        // A wide character never straddles the limit.
        assert_eq!(truncate("日本語キーボード", 8), "日本...");
        assert_eq!(truncate("日本語キーボード", 9), "日本語...");
    }

    #[test]
    fn aligned_columns() {
        let mut table = Table::new(&["Name", "Type"]).max_width(0, 16);
        table.row(vec!["日本語 PS/2 キーボード".into(), "0x7/0x2".into()]);
        table.row(vec!["Keychron".into(), "0x4/0x0".into()]);
        table.row(vec!["short".into()]);
        let text = table.render("  ");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "  Name            Type");
        assert_eq!(lines[1], "  日本語 PS/2...  0x7/0x2");
        assert_eq!(lines[2], "  Keychron        0x4/0x0");
        assert_eq!(lines[3], "  short");
        // The second column starts at the same console column on every line.
        let column = |line: &str, needle: &str| display_width(&line[..line.find(needle).unwrap()]);
        assert_eq!(column(lines[1], "0x"), column(lines[0], "Type"));
        assert_eq!(column(lines[2], "0x"), column(lines[0], "Type"));
    }
}
