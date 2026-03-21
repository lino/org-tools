// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Detects heading-like stars that appear mid-line, suggesting accidentally concatenated lines.
///
/// A heading must start at the beginning of a line. If a `\n*` sequence
/// followed by a space appears embedded within a single line, it likely means
/// two lines were accidentally concatenated. This rule skips content inside
/// protected regions.
///
/// **org-lint:** `misplaced-heading`
///
/// # Example
///
/// ```text
/// some text\n** Heading
/// ```
///
/// The `**` was probably meant to start a new line.
pub struct MisplacedHeading;

impl LintRule for MisplacedHeading {
    fn id(&self) -> &'static str {
        "W020"
    }

    fn name(&self) -> &'static str {
        "misplaced-heading"
    }

    fn description(&self) -> &'static str {
        "Detect heading stars appearing mid-line"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);
        let mut offset = 0;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            // Look for `\n*` pattern mid-line: a non-empty prefix followed by
            // one or more `*` and then a space (heading pattern not at BOL).
            // Skip lines that start with `*` (valid headings) or are empty.
            if !raw.is_empty() && !raw.starts_with('*') {
                // Search for `* ` pattern after a newline-like break.
                // In practice, look for sequences like `\n*` that ended up on one line.
                if let Some(pos) = raw.find("\n*") {
                    let after = &raw[pos + 1..];
                    let stars_end = after.len() - after.trim_start_matches('*').len();
                    if stars_end > 0
                        && after.len() > stars_end
                        && after.as_bytes()[stars_end] == b' '
                    {
                        let (line_num, _) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: pos + 1,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: "possible misplaced heading — stars appear mid-line"
                                .to_string(),
                            fix: None,
                        });
                    }
                }
            }

            offset += line.len() + 1;
        }

        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        MisplacedHeading.check(&ctx)
    }

    #[test]
    fn normal_heading() {
        assert!(check_it("* Heading\n").is_empty());
    }

    #[test]
    fn normal_text() {
        assert!(check_it("just text\n").is_empty());
    }

    #[test]
    fn bold_text_not_flagged() {
        // *bold* in text is not a heading.
        assert!(check_it("some *bold* text\n").is_empty());
    }

    #[test]
    fn no_false_positive_on_stars_in_text() {
        assert!(check_it("rating: ****\n").is_empty());
    }
}
