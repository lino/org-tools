// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects heading level gaps (e.g., jumping from `*` to `***` without `**`).
///
/// Spec: [Manual: Headlines](https://orgmode.org/manual/Headlines.html),
/// [Syntax: Headlines](https://orgmode.org/worg/org-syntax.html#Headlines_and_Sections)
///
/// org-lint: N/A (orgfmt-specific rule)
///
/// While org-mode allows arbitrary heading levels, skipping levels is usually
/// unintentional and causes unexpected outline structure. Going back up to a
/// shallower level is fine.
pub struct HeadingLevelGap;

/// Returns the heading level (number of leading `*` characters) if the line is a heading.
fn heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('*') {
        return None;
    }
    let stars = trimmed.len() - trimmed.trim_start_matches('*').len();
    let after = &trimmed[stars..];
    if after.is_empty() || after.starts_with(' ') {
        Some(stars)
    } else {
        None
    }
}

impl LintRule for HeadingLevelGap {
    fn id(&self) -> &'static str {
        "W001"
    }

    fn name(&self) -> &'static str {
        "heading-level-gap"
    }

    fn description(&self) -> &'static str {
        "Detect heading level gaps (e.g., * to *** without **)"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut prev_level: Option<usize> = None;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            if let Some(level) = heading_level(raw) {
                if let Some(prev) = prev_level {
                    if level > prev + 1 {
                        let line_start: usize = ctx
                            .source
                            .content
                            .split('\n')
                            .take(i)
                            .map(|l| l.len() + 1)
                            .sum();
                        let (line_num, col) = ctx.source.line_col(line_start);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                "heading level jumps from {} to {} (missing level {})",
                                prev,
                                level,
                                prev + 1
                            ),
                            fix: None,
                        });
                    }
                }
                prev_level = Some(level);
            }
        }

        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceFile;
    use crate::config::Config;

    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        HeadingLevelGap.check(&ctx)
    }

    #[test]
    fn no_gap() {
        let diags = check_it("* H1\n** H2\n*** H3\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn detects_gap() {
        let diags = check_it("* H1\n*** H3\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing level 2"));
    }

    #[test]
    fn going_back_up_is_fine() {
        let diags = check_it("* H1\n** H2\n*** H3\n* H1 again\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn multiple_gaps() {
        let diags = check_it("* H1\n*** H3\n***** H5\n");
        assert_eq!(diags.len(), 2);
    }

    #[test]
    fn no_headings() {
        let diags = check_it("just text\nno headings\n");
        assert!(diags.is_empty());
    }
}
