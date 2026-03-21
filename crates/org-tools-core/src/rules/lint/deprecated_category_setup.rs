// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects multiple `#+CATEGORY:` keywords in a single file.
///
/// Spec: [Manual: Categories](https://orgmode.org/manual/Categories.html)
///
/// org-lint: `deprecated-category-setup`
///
/// Only the first `#+CATEGORY:` keyword is effective at the file level.
/// Subsequent occurrences should use the `:CATEGORY:` property on individual
/// headings instead.
pub struct DeprecatedCategorySetup;

impl LintRule for DeprecatedCategorySetup {
    fn id(&self) -> &'static str {
        "W008"
    }

    fn name(&self) -> &'static str {
        "deprecated-category-setup"
    }

    fn description(&self) -> &'static str {
        "Detect multiple #+CATEGORY: keywords (only the first is effective)"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut first_seen = false;
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();
            let upper = trimmed.to_uppercase();

            if upper.starts_with("#+CATEGORY:") {
                if first_seen {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "only the first #+CATEGORY: is effective — use :CATEGORY: property instead".to_string(),
                        fix: None,
                    });
                } else {
                    first_seen = true;
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
        DeprecatedCategorySetup.check(&ctx)
    }

    #[test]
    fn single_category() {
        assert!(check_it("#+CATEGORY: test\n").is_empty());
    }

    #[test]
    fn multiple_categories() {
        let diags = check_it("#+CATEGORY: first\n#+CATEGORY: second\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn no_category() {
        assert!(check_it("#+TITLE: test\n").is_empty());
    }
}
