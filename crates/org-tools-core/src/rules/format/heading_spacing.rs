// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Fix, Span};
use crate::rules::{FormatContext, FormatRule};

/// Ensures exactly one blank line before each heading.
///
/// Spec: [Headings](https://orgmode.org/worg/org-syntax.html#Headlines_and_Sections)
///
/// Headings are visually distinct elements and benefit from consistent
/// vertical spacing. This rule inserts one blank line before a heading
/// when there is none, removes extras when there are more than one, and
/// skips spacing between consecutive headings (no blank line needed).
/// The first line of the file is never preceded by a blank line.
///
/// Note: Emacs does not auto-format heading spacing. This is an org-tools
/// convention that diverges from Emacs behavior.
///
/// Rule ID: `F003`
pub struct HeadingSpacing;

/// Returns true if the line is an org heading (starts with one or more `*` followed by a space).
fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('*') {
        return false;
    }
    let after_stars = trimmed.trim_start_matches('*');
    after_stars.starts_with(' ') || after_stars.is_empty()
}

impl FormatRule for HeadingSpacing {
    fn id(&self) -> &'static str {
        "F003"
    }

    fn name(&self) -> &'static str {
        "heading-spacing"
    }

    fn description(&self) -> &'static str {
        "Ensure one blank line before headings"
    }

    fn format(&self, ctx: &FormatContext) -> Vec<Fix> {
        let content = &ctx.source.content;
        let lines: Vec<&str> = content.split('\n').collect();
        let mut fixes = Vec::new();
        let mut offset = 0;

        for (i, &line) in lines.iter().enumerate() {
            let raw_line = line.strip_suffix('\r').unwrap_or(line);

            if is_heading(raw_line) && i > 0 {
                // Count blank lines immediately before this heading.
                let mut blank_count = 0;
                let mut j = i - 1;
                loop {
                    let prev = lines[j].strip_suffix('\r').unwrap_or(lines[j]);
                    if prev.trim().is_empty() {
                        blank_count += 1;
                    } else {
                        break;
                    }
                    if j == 0 {
                        break;
                    }
                    j -= 1;
                }

                let prev_content_line = if blank_count < i {
                    let idx = i - blank_count - 1;
                    Some(lines[idx].strip_suffix('\r').unwrap_or(lines[idx]))
                } else {
                    None
                };

                // Skip if previous content line is also a heading (consecutive headings).
                let prev_is_heading = prev_content_line.is_some_and(is_heading);

                // We want exactly 1 blank line before a heading, unless:
                // - It's the first line of the file (no blank line needed)
                // - Previous non-blank line is a heading (no blank line needed)
                let desired = if prev_is_heading { 0 } else { 1 };

                if blank_count != desired {
                    // Calculate the span of blank lines before this heading.
                    let blank_region_start: usize =
                        lines[..i - blank_count].iter().map(|l| l.len() + 1).sum();
                    let blank_region_end = offset;

                    let replacement = "\n".repeat(desired);
                    fixes.push(Fix::new(
                        Span::new(blank_region_start, blank_region_end),
                        replacement,
                    ));
                }
            }

            offset += line.len() + 1;
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
        let fixes = HeadingSpacing.format(&ctx);
        apply_fixes(input, &fixes)
    }

    #[test]
    fn adds_blank_line_before_heading() {
        assert_eq!(format_it("text\n* Heading\n"), "text\n\n* Heading\n");
    }

    #[test]
    fn already_correct() {
        let input = "text\n\n* Heading\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn removes_extra_blank_lines() {
        assert_eq!(format_it("text\n\n\n\n* Heading\n"), "text\n\n* Heading\n");
    }

    #[test]
    fn no_blank_between_consecutive_headings() {
        let input = "* Heading 1\n* Heading 2\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn first_line_heading() {
        let input = "* Heading\ntext\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn heading_after_content_under_heading() {
        assert_eq!(format_it("* H1\ntext\n* H2\n"), "* H1\ntext\n\n* H2\n");
    }
}
