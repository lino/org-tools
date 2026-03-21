// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Fix, Span};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{FormatContext, FormatRule};

/// Collapses consecutive blank lines to at most one.
///
/// Multiple consecutive blank lines add no semantic value in org-mode and
/// reduce readability. This rule preserves a single blank line as a paragraph
/// separator but removes any additional ones. Content inside
/// [`protected regions`](super::regions) is left untouched.
///
/// Rule ID: `F002`
pub struct BlankLines;

impl FormatRule for BlankLines {
    fn id(&self) -> &'static str {
        "F002"
    }

    fn name(&self) -> &'static str {
        "blank-lines"
    }

    fn description(&self) -> &'static str {
        "Collapse consecutive blank lines to at most one"
    }

    fn format(&self, ctx: &FormatContext) -> Vec<Fix> {
        let content = &ctx.source.content;
        let regions = protected_regions(content);
        let mut fixes = Vec::new();
        let mut consecutive_blanks = 0;
        let mut removal_start: Option<usize> = None;
        let mut offset = 0;

        for (i, line) in content.split('\n').enumerate() {
            let raw_line = line.strip_suffix('\r').unwrap_or(line);
            let is_blank = raw_line.trim().is_empty();
            let protected = is_protected(i, &regions);

            if is_blank && !protected {
                consecutive_blanks += 1;
                if consecutive_blanks > 1 && removal_start.is_none() {
                    // Start removing from this line (keep the first blank line).
                    removal_start = Some(offset);
                }
            } else {
                if let Some(start) = removal_start.take() {
                    // Remove from start up to (but not including) current line.
                    fixes.push(Fix::new(Span::new(start, offset), String::new()));
                }
                consecutive_blanks = 0;
            }

            offset += line.len() + 1; // +1 for '\n'
        }

        // Handle trailing blank lines at end of file.
        if let Some(start) = removal_start {
            if start < content.len() {
                fixes.push(Fix::new(Span::new(start, content.len()), String::new()));
            }
        }

        fixes
    }
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
        let fixes = BlankLines.format(&ctx);
        apply_fixes(input, &fixes)
    }

    #[test]
    fn no_consecutive_blanks() {
        let input = "a\n\nb\n\nc\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn collapses_double_blank() {
        assert_eq!(format_it("a\n\n\nb\n"), "a\n\nb\n");
    }

    #[test]
    fn collapses_triple_blank() {
        assert_eq!(format_it("a\n\n\n\nb\n"), "a\n\nb\n");
    }

    #[test]
    fn preserves_blanks_in_src_block() {
        let input = "#+BEGIN_SRC python\na\n\n\n\nb\n#+END_SRC\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn collapses_outside_but_not_inside_block() {
        let input = "text\n\n\n\n#+BEGIN_SRC python\na\n\n\n\nb\n#+END_SRC\n";
        let expected = "text\n\n#+BEGIN_SRC python\na\n\n\n\nb\n#+END_SRC\n";
        assert_eq!(format_it(input), expected);
    }

    #[test]
    fn handles_trailing_blank_lines() {
        assert_eq!(format_it("text\n\n\n\n"), "text\n\n");
    }

    #[test]
    fn empty_file() {
        assert_eq!(format_it(""), "");
    }
}
