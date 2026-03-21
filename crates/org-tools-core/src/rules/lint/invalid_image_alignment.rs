// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects invalid `:align` or `:center` values in `#+ATTR_ORG:`.
//!
//! org-lint: `invalid-image-alignment`
//!
//! Valid `:align` values: `left`, `center`, `right`.
//! Valid `:center` value: `t`.

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Validates `:align` and `:center` attribute values on `#+ATTR_ORG:` lines.
///
/// The `:align` property accepts `left`, `center`, or `right`. The `:center`
/// property accepts only `t`. Any other value is reported as a warning.
///
/// org-lint: `invalid-image-alignment`
pub struct InvalidImageAlignment;

impl LintRule for InvalidImageAlignment {
    fn id(&self) -> &'static str {
        "W026"
    }

    fn name(&self) -> &'static str {
        "invalid-image-alignment"
    }

    fn description(&self) -> &'static str {
        "Detect invalid :align or :center in #+ATTR_ORG:"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();
            let upper = trimmed.to_uppercase();

            if upper.starts_with("#+ATTR_ORG:") {
                let rest = &trimmed[11..];

                // Check :align value.
                if let Some(align_pos) = rest.find(":align") {
                    let after = &rest[align_pos + 6..].trim_start();
                    let value = after.split_whitespace().next().unwrap_or("");
                    if !value.is_empty()
                        && value != "left"
                        && value != "center"
                        && value != "right"
                    {
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                "invalid :align value '{}' — expected left, center, or right",
                                value
                            ),
                            fix: None,
                        });
                    }
                }

                // Check :center value.
                if let Some(center_pos) = rest.find(":center") {
                    let after = &rest[center_pos + 7..].trim_start();
                    let value = after.split_whitespace().next().unwrap_or("");
                    if !value.is_empty() && value != "t" {
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                "invalid :center value '{}' — expected t",
                                value
                            ),
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
        InvalidImageAlignment.check(&ctx)
    }

    #[test]
    fn valid_align() {
        assert!(check_it("#+ATTR_ORG: :align center\n").is_empty());
    }

    #[test]
    fn valid_center() {
        assert!(check_it("#+ATTR_ORG: :center t\n").is_empty());
    }

    #[test]
    fn invalid_align() {
        let diags = check_it("#+ATTR_ORG: :align middle\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("middle"));
    }

    #[test]
    fn invalid_center() {
        let diags = check_it("#+ATTR_ORG: :center yes\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn no_attr_org() {
        assert!(check_it("#+ATTR_LATEX: :width 0.5\n").is_empty());
    }
}
