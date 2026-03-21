// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects heading level gaps (e.g., jumping from `*` to `***` without `**`).
///
/// Spec: [Manual: Headlines](https://orgmode.org/manual/Headlines.html),
/// [Syntax: Headlines](https://orgmode.org/worg/org-syntax.html#Headlines_and_Sections),
/// [Manual: In-buffer Settings](https://orgmode.org/manual/In_002dbuffer-Settings.html)
///
/// org-lint: N/A (org-tools-specific rule)
///
/// While org-mode allows arbitrary heading levels, skipping levels is usually
/// unintentional and causes unexpected outline structure. Going back up to a
/// shallower level is fine.
///
/// When `#+STARTUP: odd` is set, only odd heading levels (1, 3, 5, …) are
/// considered valid. A step of 2 between odd levels is normal; the rule then
/// flags gaps larger than 2 (e.g., level 1 → 5 skips level 3). The companion
/// setting `#+STARTUP: oddeven` explicitly reverts to the default behaviour.
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
        let odd_mode = has_startup_odd(&ctx.source.content);
        // In odd mode levels step by 2 (1→3→5); in normal mode by 1.
        let step = if odd_mode { 2 } else { 1 };

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            if let Some(level) = heading_level(raw) {
                if odd_mode && level % 2 == 0 {
                    // Even level in odd mode is itself an error.
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
                            "even heading level {} is not valid with #+STARTUP: odd",
                            level
                        ),
                        fix: None,
                    });
                } else if let Some(prev) = prev_level {
                    if level > prev + step {
                        let missing = prev + step;
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
                                prev, level, missing
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

/// Check if the file preamble contains `#+STARTUP:` with the `odd` option.
///
/// The `oddeven` option explicitly reverts to normal mode. Multiple `#+STARTUP:`
/// lines are allowed; the last `odd` or `oddeven` token wins.
fn has_startup_odd(content: &str) -> bool {
    let mut odd = false;
    for line in content.split('\n') {
        let raw = line.strip_suffix('\r').unwrap_or(line);
        // Stop at first heading.
        if heading_level(raw).is_some() {
            break;
        }
        let trimmed = raw.trim();
        if let Some(rest) = trimmed.strip_prefix("#+") {
            if let Some(colon) = rest.find(':') {
                let key = rest[..colon].trim();
                if key.eq_ignore_ascii_case("STARTUP") {
                    let val = &rest[colon + 1..];
                    for token in val.split_whitespace() {
                        if token.eq_ignore_ascii_case("odd") {
                            odd = true;
                        } else if token.eq_ignore_ascii_case("oddeven") {
                            odd = false;
                        }
                    }
                }
            }
        }
    }
    odd
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

    // --- #+STARTUP: odd ---

    #[test]
    fn odd_mode_valid_levels() {
        let diags = check_it("#+STARTUP: odd\n* H1\n*** H3\n***** H5\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn odd_mode_detects_gap() {
        // Level 1 → 5 skips level 3.
        let diags = check_it("#+STARTUP: odd\n* H1\n***** H5\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing level 3"));
    }

    #[test]
    fn odd_mode_even_level_flagged() {
        let diags = check_it("#+STARTUP: odd\n* H1\n** H2\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("even heading level 2"));
    }

    #[test]
    fn odd_mode_going_back_up_is_fine() {
        let diags = check_it("#+STARTUP: odd\n* H1\n*** H3\n***** H5\n* H1 again\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn oddeven_reverts_to_normal() {
        // oddeven explicitly reverts odd mode.
        let diags = check_it("#+STARTUP: odd\n#+STARTUP: oddeven\n* H1\n** H2\n*** H3\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn odd_in_combined_startup() {
        // odd can appear among other startup options.
        let diags = check_it("#+STARTUP: overview odd logdone\n* H1\n*** H3\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn no_startup_uses_normal_mode() {
        // Without #+STARTUP: odd, normal gap detection applies.
        let diags = check_it("* H1\n*** H3\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing level 2"));
    }

    #[test]
    fn has_startup_odd_helper() {
        assert!(has_startup_odd("#+STARTUP: odd\n* H\n"));
        assert!(!has_startup_odd("#+STARTUP: overview\n* H\n"));
        assert!(!has_startup_odd(
            "#+STARTUP: odd\n#+STARTUP: oddeven\n* H\n"
        ));
        assert!(has_startup_odd("#+STARTUP: overview odd logdone\n* H\n"));
        assert!(!has_startup_odd("* H\n"));
    }
}
