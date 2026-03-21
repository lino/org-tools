// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Fix, Span};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{FormatContext, FormatRule};

/// Normalizes spacing after `#+KEYWORD:` to exactly one space.
///
/// Spec: [Keywords](https://orgmode.org/worg/org-syntax.html#Keywords)
///
/// Org keywords follow the pattern `#+KEY: VALUE`. This rule ensures there
/// is exactly one space between the colon and the value. Block delimiters
/// (`#+BEGIN_`/`#+END_`), `#+CALL`, and `#+RESULTS` are excluded. Content
/// inside [`protected regions`](super::regions) is skipped.
///
/// Examples:
/// - `#+TITLE:My Title` becomes `#+TITLE: My Title`
/// - `#+TITLE:  My Title` becomes `#+TITLE: My Title`
///
/// Rule ID: `F006`
pub struct KeywordSpacing;

impl FormatRule for KeywordSpacing {
    fn id(&self) -> &'static str {
        "F006"
    }

    fn name(&self) -> &'static str {
        "keyword-spacing"
    }

    fn description(&self) -> &'static str {
        "Normalize spacing after #+KEYWORD:"
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

            let trimmed = raw.trim_start();
            let leading = raw.len() - trimmed.len();

            if let Some(rest) = trimmed.strip_prefix("#+") {
                // Skip block delimiters, CALL, RESULTS.
                let rest_upper = rest.to_uppercase();
                if rest_upper.starts_with("BEGIN")
                    || rest_upper.starts_with("END")
                    || rest_upper.starts_with("CALL")
                    || rest_upper.starts_with("RESULTS")
                    || rest.is_empty()
                {
                    offset += line.len() + 1;
                    continue;
                }

                // Find the colon after the keyword name.
                if let Some(colon_pos) = rest.find(':') {
                    let after_colon = &rest[colon_pos + 1..];
                    // Only fix if there's content after the colon.
                    if !after_colon.is_empty() {
                        let spaces = after_colon.len() - after_colon.trim_start().len();
                        if spaces != 1 {
                            let fix_start = offset + leading + 2 + colon_pos + 1;
                            let fix_end = fix_start + spaces;
                            fixes.push(Fix::new(Span::new(fix_start, fix_end), " ".to_string()));
                        }
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
        let fixes = KeywordSpacing.format(&ctx);
        apply_fixes(input, &fixes)
    }

    #[test]
    fn already_correct() {
        assert_eq!(format_it("#+TITLE: My Title\n"), "#+TITLE: My Title\n");
    }

    #[test]
    fn no_space() {
        assert_eq!(format_it("#+TITLE:My Title\n"), "#+TITLE: My Title\n");
    }

    #[test]
    fn extra_spaces() {
        assert_eq!(format_it("#+TITLE:   My Title\n"), "#+TITLE: My Title\n");
    }

    #[test]
    fn empty_value() {
        // No value after colon — leave as-is.
        assert_eq!(format_it("#+STARTUP:\n"), "#+STARTUP:\n");
    }

    #[test]
    fn block_not_touched() {
        let input = "#+BEGIN_SRC python\ncode\n#+END_SRC\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn multiple_keywords() {
        let input = "#+TITLE:A\n#+AUTHOR:  B\n";
        assert_eq!(format_it(input), "#+TITLE: A\n#+AUTHOR: B\n");
    }
}
