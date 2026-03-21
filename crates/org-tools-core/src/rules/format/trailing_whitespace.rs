// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Fix, Span};
use crate::rules::{FormatContext, FormatRule};

/// Removes trailing whitespace (spaces and tabs) from all lines.
///
/// Trailing whitespace is universally undesirable in text files and not
/// meaningful in org-mode syntax. This rule strips it from every line,
/// including lines inside protected regions (trailing whitespace is never
/// significant content).
///
/// Rule ID: `F001`
pub struct TrailingWhitespace;

impl FormatRule for TrailingWhitespace {
    fn id(&self) -> &'static str {
        "F001"
    }

    fn name(&self) -> &'static str {
        "trailing-whitespace"
    }

    fn description(&self) -> &'static str {
        "Remove trailing whitespace from all lines"
    }

    fn format(&self, ctx: &FormatContext) -> Vec<Fix> {
        let mut fixes = Vec::new();
        let content = &ctx.source.content;
        let mut offset = 0;

        for line in content.split('\n') {
            let raw_line = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw_line.trim_end();

            if trimmed.len() < raw_line.len() {
                let trail_start = offset + trimmed.len();
                let trail_end = offset + raw_line.len();
                fixes.push(Fix::new(Span::new(trail_start, trail_end), String::new()));
            }

            // +1 for the '\n' delimiter (except possibly the last line)
            offset += line.len() + 1;
        }

        fixes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    fn format_with_rule(input: &str) -> Vec<Fix> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = FormatContext::new(&source, &config);
        TrailingWhitespace.format(&ctx)
    }

    #[test]
    fn no_trailing_whitespace() {
        let fixes = format_with_rule("* Heading\nSome text\n");
        assert!(fixes.is_empty());
    }

    #[test]
    fn removes_trailing_spaces() {
        let fixes = format_with_rule("* Heading   \nSome text  \n");
        assert_eq!(fixes.len(), 2);
        assert_eq!(fixes[0].span, Span::new(9, 12));
        assert_eq!(fixes[0].replacement, "");
        assert_eq!(fixes[1].span, Span::new(22, 24));
    }

    #[test]
    fn removes_trailing_tabs() {
        let fixes = format_with_rule("text\t\t\n");
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].span, Span::new(4, 6));
    }

    #[test]
    fn handles_empty_file() {
        let fixes = format_with_rule("");
        assert!(fixes.is_empty());
    }

    #[test]
    fn handles_no_trailing_newline() {
        let fixes = format_with_rule("text   ");
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].span, Span::new(4, 7));
    }
}
