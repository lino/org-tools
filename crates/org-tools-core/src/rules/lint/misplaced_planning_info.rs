// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects planning lines (`SCHEDULED`/`DEADLINE`/`CLOSED`) not immediately after a heading.
///
/// Planning information is only recognized by org-mode when it appears on the
/// line directly below a headline (or directly below another planning line).
/// A blank line or body text between the heading and the planning keywords
/// causes org-mode to treat them as plain text.
///
/// **Spec:** [Deadlines and Scheduling](https://orgmode.org/manual/Deadlines-and-Scheduling.html),
/// [Planning (syntax)](https://orgmode.org/worg/org-syntax.html#Planning)
///
/// **org-lint:** `misplaced-planning-info`
///
/// # Example
///
/// ```org
/// ;; Bad — blank line separates heading from planning
/// * TODO Task
///
/// SCHEDULED: <2024-01-15 Mon>
///
/// ;; Good
/// * TODO Task
/// SCHEDULED: <2024-01-15 Mon>
/// ```
pub struct MisplacedPlanningInfo;

/// Returns `true` if the line is an org-mode heading.
fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('*') {
        return false;
    }
    let after = trimmed.trim_start_matches('*');
    after.starts_with(' ') || after.is_empty()
}

/// Returns `true` if the line starts with a planning keyword.
fn is_planning_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

impl LintRule for MisplacedPlanningInfo {
    fn id(&self) -> &'static str {
        "W015"
    }

    fn name(&self) -> &'static str {
        "misplaced-planning-info"
    }

    fn description(&self) -> &'static str {
        "Detect planning lines not immediately after a heading"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let lines: Vec<&str> = ctx.source.content.split('\n').collect();
        let mut offset = 0;

        for (i, &line) in lines.iter().enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_planning_line(raw) {
                // Valid if previous line is a heading or another planning line.
                let valid = if i == 0 {
                    false
                } else {
                    let prev = lines[i - 1].strip_suffix('\r').unwrap_or(lines[i - 1]);
                    is_heading(prev) || is_planning_line(prev)
                };

                if !valid {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "planning line is not immediately after a heading".to_string(),
                        fix: None,
                    });
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
        MisplacedPlanningInfo.check(&ctx)
    }

    #[test]
    fn valid_after_heading() {
        assert!(check_it("* TODO Task\nSCHEDULED: <2024-01-15 Mon>\n").is_empty());
    }

    #[test]
    fn valid_multiple_planning() {
        let input = "* TODO Task\nSCHEDULED: <2024-01-15 Mon>\nDEADLINE: <2024-02-01 Thu>\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn misplaced_after_text() {
        let diags = check_it("Some text.\nSCHEDULED: <2024-01-15 Mon>\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn misplaced_after_blank_line() {
        let diags = check_it("* TODO Task\n\nSCHEDULED: <2024-01-15 Mon>\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn no_planning_lines() {
        assert!(check_it("* Heading\ntext\n").is_empty());
    }
}
