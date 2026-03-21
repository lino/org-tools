// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Fix, Span};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::list::{parse_list_item, ListMarker};
use crate::rules::{FormatContext, FormatRule};

/// Normalizes unordered list markers to `-` and checkbox case to uppercase `[X]`.
///
/// Spec: [Plain Lists](https://orgmode.org/manual/Plain-Lists.html),
/// [Checkboxes](https://orgmode.org/manual/Checkboxes.html),
/// [Plain Lists syntax](https://orgmode.org/worg/org-syntax.html#Plain_Lists_and_Items)
///
/// Org-mode allows `+`, `*`, and `-` as unordered list markers. This rule
/// standardizes them all to `-` for consistency. Lowercase checkbox markers
/// `[x]` are normalized to uppercase `[X]`. Ordered list items (`1.`, `1)`)
/// are left unchanged. Content inside [`protected regions`](super::regions)
/// is skipped.
///
/// Rule ID: `F008`
pub struct ListFormat;

impl FormatRule for ListFormat {
    fn id(&self) -> &'static str {
        "F008"
    }

    fn name(&self) -> &'static str {
        "list-format"
    }

    fn description(&self) -> &'static str {
        "Normalize list markers to - and checkbox case to [X]"
    }

    fn format(&self, ctx: &FormatContext) -> Vec<Fix> {
        let content = &ctx.source.content;
        let regions = protected_regions(content);
        let mut fixes = Vec::new();
        let mut offset = 0;

        for (i, line) in content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            if let Some(item) = parse_list_item(raw) {
                let needs_marker_fix = matches!(item.marker, ListMarker::Plus | ListMarker::Star);
                let needs_checkbox_fix = item.checkbox == Some('x');

                if needs_marker_fix || needs_checkbox_fix {
                    // Reconstruct the line.
                    let indent = " ".repeat(item.indent);
                    let marker = match &item.marker {
                        ListMarker::Plus | ListMarker::Star => "-",
                        ListMarker::Dash => "-",
                        ListMarker::OrderedDot(_) => {
                            offset += line.len() + 1;
                            continue;
                        }
                        ListMarker::OrderedParen(_) => {
                            offset += line.len() + 1;
                            continue;
                        }
                    };

                    let checkbox_str = match item.checkbox {
                        Some('x') => " [X]",
                        Some('X') => " [X]",
                        Some(' ') => " [ ]",
                        Some('-') => " [-]",
                        Some(_) => "",
                        None => "",
                    };

                    let content_part = if item.content.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", item.content)
                    };

                    let new_line = format!("{}{}{}{}", indent, marker, checkbox_str, content_part);

                    if new_line != raw {
                        fixes.push(Fix::new(
                            Span::new(offset, offset + raw.len()),
                            new_line,
                        ));
                    }
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
        let fixes = ListFormat.format(&ctx);
        apply_fixes(input, &fixes)
    }

    #[test]
    fn dash_unchanged() {
        assert_eq!(format_it("- Item\n"), "- Item\n");
    }

    #[test]
    fn plus_to_dash() {
        assert_eq!(format_it("+ Item\n"), "- Item\n");
    }

    #[test]
    fn star_to_dash() {
        assert_eq!(format_it("  * Item\n"), "  - Item\n");
    }

    #[test]
    fn lowercase_checkbox() {
        assert_eq!(format_it("- [x] Done\n"), "- [X] Done\n");
    }

    #[test]
    fn uppercase_checkbox_unchanged() {
        assert_eq!(format_it("- [X] Done\n"), "- [X] Done\n");
    }

    #[test]
    fn ordered_list_unchanged() {
        assert_eq!(format_it("1. First\n"), "1. First\n");
    }

    #[test]
    fn nested_plus() {
        assert_eq!(format_it("  + Nested\n"), "  - Nested\n");
    }

    #[test]
    fn in_code_block_unchanged() {
        let input = "#+BEGIN_SRC org\n+ Item\n#+END_SRC\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn not_a_list() {
        assert_eq!(format_it("just text\n"), "just text\n");
    }
}
