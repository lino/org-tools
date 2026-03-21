// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use unicode_width::UnicodeWidthStr;

use crate::diagnostic::{Fix, Span};
use crate::rules::{FormatContext, FormatRule};

/// Aligns table columns and normalizes separator rows.
///
/// Spec: [Tables](https://orgmode.org/worg/org-syntax.html#Tables),
/// [§3.6 Tables](https://orgmode.org/manual/Tables.html)
///
/// Pads all cells in a column to the same display width (using Unicode
/// width for correct CJK handling). Numeric columns are right-aligned
/// to match Emacs `org-table-align` behavior; text columns are
/// left-aligned. Separator rows (`|---+---|`) are regenerated to match
/// the computed column widths. Header rows (data rows immediately before
/// a separator) are excluded from numeric-column detection.
///
/// Rule ID: `F004`
pub struct TableFormatter;

/// A parsed org table ready for formatting.
struct Table {
    /// Byte offset of the first character of the table in the source.
    start: usize,
    /// Byte offset just past the last character of the table (including trailing newline).
    end: usize,
    /// The rows of the table, each being either data cells or a separator.
    rows: Vec<TableRow>,
}

enum TableRow {
    /// A separator row like `|---+---|`
    Separator,
    /// A data row with cell contents (trimmed).
    Data(Vec<String>),
}

impl FormatRule for TableFormatter {
    fn id(&self) -> &'static str {
        "F004"
    }

    fn name(&self) -> &'static str {
        "table-format"
    }

    fn description(&self) -> &'static str {
        "Align table columns and normalize separators"
    }

    fn format(&self, ctx: &FormatContext) -> Vec<Fix> {
        let content = &ctx.source.content;
        let tables = find_tables(content);
        let mut fixes = Vec::new();

        for table in tables {
            if let Some(formatted) = format_table(&table) {
                let original = &content[table.start..table.end];
                if formatted != original {
                    fixes.push(Fix::new(Span::new(table.start, table.end), formatted));
                }
            }
        }

        fixes
    }
}

/// Find all table regions in the source text.
fn find_tables(content: &str) -> Vec<Table> {
    let mut tables = Vec::new();
    let lines: Vec<&str> = content.split('\n').collect();
    let mut i = 0;

    while i < lines.len() {
        let raw = lines[i].strip_suffix('\r').unwrap_or(lines[i]);
        if raw.trim_start().starts_with('|') {
            let table_start_line = i;
            let mut table_lines = Vec::new();

            while i < lines.len() {
                let raw = lines[i].strip_suffix('\r').unwrap_or(lines[i]);
                if raw.trim_start().starts_with('|') {
                    table_lines.push(raw);
                    i += 1;
                } else {
                    break;
                }
            }

            let start: usize = lines[..table_start_line].iter().map(|l| l.len() + 1).sum();
            let end: usize = lines[..table_start_line + table_lines.len()]
                .iter()
                .map(|l| l.len() + 1)
                .sum();

            let rows = parse_table_rows(&table_lines);
            tables.push(Table { start, end, rows });
        } else {
            i += 1;
        }
    }

    tables
}

fn parse_table_rows(lines: &[&str]) -> Vec<TableRow> {
    lines
        .iter()
        .map(|line| {
            let trimmed = line.trim();
            // A separator row matches: |---+---| or |-+-| etc.
            // It starts with |, ends with |, and contains only -, +, |, and whitespace.
            if is_separator_row(trimmed) {
                TableRow::Separator
            } else {
                let cells = parse_data_row(trimmed);
                TableRow::Data(cells)
            }
        })
        .collect()
}

fn is_separator_row(line: &str) -> bool {
    if !line.starts_with('|') {
        return false;
    }
    let inner = line.trim_start_matches('|').trim_end_matches('|');
    if inner.is_empty() {
        return false;
    }
    inner
        .chars()
        .all(|c| c == '-' || c == '+' || c == '|' || c == ' ')
        && inner.contains('-')
}

/// Returns true if the string looks like a number (integer, float, or with
/// thousands separators). Matches Emacs org-mode's heuristic for right-aligning.
fn is_numeric(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Strip leading sign.
    let rest = trimmed
        .strip_prefix('-')
        .or_else(|| trimmed.strip_prefix('+'))
        .unwrap_or(trimmed);
    if rest.is_empty() {
        return false;
    }
    // Allow digits, dots, commas (thousands separators), spaces (European style).
    let has_digit = rest.chars().any(|c| c.is_ascii_digit());
    let all_valid = rest.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == ' ' || c == '%' || c == '$' || c == '€');
    has_digit && all_valid
}

fn parse_data_row(line: &str) -> Vec<String> {
    let stripped = line.strip_prefix('|').unwrap_or(line);
    let stripped = stripped.strip_suffix('|').unwrap_or(stripped);
    stripped
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn format_table(table: &Table) -> Option<String> {
    // Determine the number of columns.
    let num_cols = table
        .rows
        .iter()
        .filter_map(|r| match r {
            TableRow::Data(cells) => Some(cells.len()),
            TableRow::Separator => None,
        })
        .max()?;

    // Calculate max display width per column.
    let mut col_widths = vec![1usize; num_cols];
    for row in &table.rows {
        if let TableRow::Data(cells) = row {
            for (j, cell) in cells.iter().enumerate() {
                if j < num_cols {
                    col_widths[j] = col_widths[j].max(UnicodeWidthStr::width(cell.as_str()));
                }
            }
        }
    }

    // Detect numeric columns: a column is numeric if all non-empty data cells
    // (excluding header rows) are numbers. This matches Emacs org-mode behavior.
    // A header row is defined as a data row immediately before a separator row.
    let mut is_header_row = vec![false; table.rows.len()];
    for (i, row) in table.rows.iter().enumerate() {
        if matches!(row, TableRow::Separator) && i > 0 {
            is_header_row[i - 1] = true;
        }
    }

    let mut col_numeric = vec![true; num_cols];
    let mut col_has_number = vec![false; num_cols];
    for (i, row) in table.rows.iter().enumerate() {
        if is_header_row[i] {
            continue; // Skip header rows for numeric detection.
        }
        if let TableRow::Data(cells) = row {
            for (j, cell) in cells.iter().enumerate() {
                if j < num_cols && !cell.is_empty() {
                    if is_numeric(cell) {
                        col_has_number[j] = true;
                    } else {
                        col_numeric[j] = false;
                    }
                }
            }
        }
    }
    // A column is only right-aligned if it actually contains numbers.
    for j in 0..num_cols {
        if !col_has_number[j] {
            col_numeric[j] = false;
        }
    }

    // Build formatted output.
    let mut result = String::new();
    for row in &table.rows {
        match row {
            TableRow::Separator => {
                result.push('|');
                for (j, &w) in col_widths.iter().enumerate() {
                    result.push('-');
                    for _ in 0..w {
                        result.push('-');
                    }
                    result.push('-');
                    if j < num_cols - 1 {
                        result.push('+');
                    }
                }
                result.push('|');
                result.push('\n');
            }
            TableRow::Data(cells) => {
                result.push('|');
                for (j, &col_width) in col_widths.iter().enumerate() {
                    let cell = cells.get(j).map(|s| s.as_str()).unwrap_or("");
                    let display_width = UnicodeWidthStr::width(cell);
                    let padding = col_width - display_width;
                    result.push(' ');
                    if col_numeric[j] {
                        // Right-align numeric columns.
                        for _ in 0..padding {
                            result.push(' ');
                        }
                        result.push_str(cell);
                    } else {
                        // Left-align text columns.
                        result.push_str(cell);
                        for _ in 0..padding {
                            result.push(' ');
                        }
                    }
                    result.push(' ');
                    result.push('|');
                }
                result.push('\n');
            }
        }
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::formatter::apply_fixes;
    use crate::source::SourceFile;

    fn format_it(input: &str) -> String {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = FormatContext::new(&source, &config);
        let fixes = TableFormatter.format(&ctx);
        apply_fixes(input, &fixes)
    }

    #[test]
    fn aligns_simple_table() {
        let input = "| a | bb | ccc |\n| dd | e | f |\n";
        let expected = "| a  | bb | ccc |\n| dd | e  | f   |\n";
        assert_eq!(format_it(input), expected);
    }

    #[test]
    fn normalizes_separator() {
        let input = "| Name | Age |\n|---+---|\n| Alice | 30 |\n";
        // Age column is numeric (header excluded from check), so 30 is right-aligned.
        let expected = "| Name  | Age |\n|-------+-----|\n| Alice |  30 |\n";
        assert_eq!(format_it(input), expected);
    }

    #[test]
    fn right_aligns_numbers() {
        let input = "| Item | Price |\n|---+---|\n| Apple | 1 |\n| Banana | 250 |\n";
        let expected = "| Item   | Price |\n|--------+-------|\n| Apple  |     1 |\n| Banana |   250 |\n";
        assert_eq!(format_it(input), expected);
    }

    #[test]
    fn mixed_column_left_aligns() {
        // Column 2 has both text and numbers → left-aligned.
        let input = "| a | b |\n| c | 5 |\n";
        let expected = "| a | b |\n| c | 5 |\n";
        assert_eq!(format_it(input), expected);
    }

    #[test]
    fn handles_uneven_columns() {
        let input = "| a | b |\n| cc |\n";
        let expected = "| a  | b |\n| cc |   |\n";
        assert_eq!(format_it(input), expected);
    }

    #[test]
    fn preserves_text_around_table() {
        let input = "before\n| a | b |\n| c | d |\nafter\n";
        let expected = "before\n| a | b |\n| c | d |\nafter\n";
        assert_eq!(format_it(input), expected);
    }

    #[test]
    fn already_formatted() {
        let input = "| a | b |\n| c | d |\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn empty_cells() {
        let input = "| a |  |\n|  | b |\n";
        let expected = "| a |   |\n|   | b |\n";
        assert_eq!(format_it(input), expected);
    }

    #[test]
    fn no_table() {
        let input = "just text\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn multiple_tables() {
        let input = "| a | bb |\ntext\n| ccc | d |\n";
        let expected = "| a | bb |\ntext\n| ccc | d |\n";
        assert_eq!(format_it(input), expected);
    }
}
