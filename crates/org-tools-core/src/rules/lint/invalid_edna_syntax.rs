// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Validates org-edna `:BLOCKER:` and `:TRIGGER:` property syntax.
//!
//! org-edna uses a mini-language in `:BLOCKER:` and `:TRIGGER:` properties to
//! express task dependencies. This rule parses those values and reports syntax
//! errors: unknown keywords, wrong argument counts, unclosed parentheses.
//!
//! The rule does NOT evaluate edna expressions — only syntactic validity is
//! checked. Content inside protected regions (code blocks) is ignored.
//!
//! Ref: <https://www.nongnu.org/org-edna-el/>

use crate::diagnostic::{Diagnostic, Severity};
use crate::edna::parse_edna;
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Validates org-edna `:BLOCKER:` and `:TRIGGER:` property syntax.
///
/// Checks that the mini-language values use valid keywords, correct argument
/// counts, and properly balanced parentheses.
///
/// Ref: <https://www.nongnu.org/org-edna-el/>
///
/// # Example
///
/// ```org
/// ;; Bad — unknown keyword
/// :BLOCKER: nonexistent-finder
///
/// ;; Bad — missing closing paren
/// :TRIGGER: todo!("DONE"
///
/// ;; Good
/// :BLOCKER: ids("task-1") todo-state?("DONE")
/// :TRIGGER: next-sibling todo!("DONE")
/// ```
pub struct InvalidEdnaSyntax;

impl LintRule for InvalidEdnaSyntax {
    fn id(&self) -> &'static str {
        "W035"
    }

    fn name(&self) -> &'static str {
        "invalid-edna-syntax"
    }

    fn description(&self) -> &'static str {
        "Validate org-edna BLOCKER/TRIGGER property syntax"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);
        let mut offset = 0;

        for (line_idx, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            if !is_protected(line_idx, &regions) {
                // Check for :BLOCKER: or :TRIGGER: property lines.
                let value = if let Some(rest) = trimmed.strip_prefix(":BLOCKER:") {
                    Some(("BLOCKER", rest.trim()))
                } else {
                    trimmed
                        .strip_prefix(":TRIGGER:")
                        .map(|rest| ("TRIGGER", rest.trim()))
                };

                if let Some((prop_name, prop_value)) = value {
                    if !prop_value.is_empty() {
                        let (_, errors) = parse_edna(prop_value);
                        for error in errors {
                            let (line_num, col) = ctx.source.line_col(offset);
                            diagnostics.push(Diagnostic {
                                file: ctx.source.path.clone(),
                                line: line_num,
                                column: col,
                                severity: Severity::Warning,
                                rule_id: self.id(),
                                rule: self.name(),
                                message: format!(":{prop_name}: {}", error.message),
                                fix: None,
                            });
                        }
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
        InvalidEdnaSyntax.check(&ctx)
    }

    #[test]
    fn valid_blocker_ids() {
        assert!(check_it("* Task\n:PROPERTIES:\n:BLOCKER: ids(\"uuid-1\")\n:END:\n").is_empty());
    }

    #[test]
    fn valid_blocker_structural() {
        assert!(check_it("* Task\n:PROPERTIES:\n:BLOCKER: previous-sibling\n:END:\n").is_empty());
    }

    #[test]
    fn valid_trigger_action() {
        assert!(
            check_it("* Task\n:PROPERTIES:\n:TRIGGER: next-sibling todo!(\"DONE\")\n:END:\n")
                .is_empty()
        );
    }

    #[test]
    fn invalid_unknown_finder() {
        let diags = check_it("* Task\n:PROPERTIES:\n:BLOCKER: nonexistent-thing\n:END:\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unknown edna finder"));
    }

    #[test]
    fn invalid_unknown_action() {
        let diags = check_it("* Task\n:PROPERTIES:\n:TRIGGER: fake-action!(\"arg\")\n:END:\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unknown edna action"));
    }

    #[test]
    fn invalid_unclosed_parens() {
        let diags = check_it("* Task\n:PROPERTIES:\n:BLOCKER: ids(\"abc\"\n:END:\n");
        assert!(!diags.is_empty());
    }

    #[test]
    fn skips_protected_regions() {
        let input = "#+begin_src org\n:BLOCKER: nonexistent-thing\n#+end_src\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn no_property_no_diagnostic() {
        assert!(check_it("* Task\n:PROPERTIES:\n:ID: abc\n:END:\n").is_empty());
    }

    #[test]
    fn empty_blocker_value() {
        // Edge case: :BLOCKER: with empty value — no crash.
        assert!(check_it("* Task\n:PROPERTIES:\n:BLOCKER:\n:END:\n").is_empty());
    }
}
