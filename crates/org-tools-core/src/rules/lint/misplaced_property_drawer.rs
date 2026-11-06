// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lint rule `W003` (`misplaced-property-drawer`) checking `:PROPERTIES:` drawer placement.
//!
//! Spec: [Manual: Property Syntax](https://orgmode.org/manual/Property-Syntax.html),
//! [Syntax: Property Drawers](https://orgmode.org/worg/org-syntax.html#Property_Drawers),
//! [Syntax: Comment Lines](https://orgmode.org/worg/org-syntax.html#Comment_Lines)

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects `:PROPERTIES:` drawers that are not directly after a heading, planning line, or file top.
///
/// Spec: [Manual: Property Syntax](https://orgmode.org/manual/Property-Syntax.html),
/// [Syntax: Property Drawers](https://orgmode.org/worg/org-syntax.html#Property_Drawers)
///
/// org-lint: `misplaced-property-drawer`
///
/// A property drawer must immediately follow a heading (or its planning line).
/// Any intervening content -- including blank lines -- makes the drawer invalid.
/// File-level property drawers at line 1 or preceded exclusively by `#` comment
/// lines (e.g., `# -*- mode: org; -*-` in `org-roam` v2 files) are accepted.
pub struct MisplacedPropertyDrawer;

/// Returns `true` if `line` is an org heading (one or more leading `*` followed by space or EOL).
fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('*') {
        return false;
    }
    let after = trimmed.trim_start_matches('*');
    after.starts_with(' ') || after.is_empty()
}

/// Returns `true` if `line` is a planning line (`SCHEDULED:`, `DEADLINE:`, or `CLOSED:`).
fn is_planning_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

/// Returns `true` if `line` is a Worg-compliant org comment line (`#` followed by space, tab, or EOL).
///
/// Spec: [Syntax: Comment Lines](https://orgmode.org/worg/org-syntax.html#Comment_Lines)
fn is_comment_line(line: &str) -> bool {
    let trimmed = line.strip_suffix('\r').unwrap_or(line).trim_start();
    trimmed == "#" || trimmed.starts_with("# ") || trimmed.starts_with("#\t")
}

/// Returns `true` if the `:PROPERTIES:` drawer at `index` is validly positioned.
fn is_valid_drawer_placement(lines: &[&str], index: usize) -> bool {
    if lines[..index].iter().copied().all(is_comment_line) {
        return true;
    }

    let mut j = index - 1;
    loop {
        let prev = lines[j].strip_suffix('\r').unwrap_or(lines[j]).trim();
        if is_heading(prev) {
            return true;
        }
        if !is_planning_line(prev) || j == 0 {
            return false;
        }
        j -= 1;
    }
}

impl LintRule for MisplacedPropertyDrawer {
    /// Returns the diagnostic rule identifier (`W003`).
    fn id(&self) -> &'static str {
        "W003"
    }

    /// Returns the kebab-case org-lint rule name.
    fn name(&self) -> &'static str {
        "misplaced-property-drawer"
    }

    /// Returns a human-readable summary of the lint rule.
    fn description(&self) -> &'static str {
        "Detect property drawers not directly after headings"
    }

    /// Checks all `:PROPERTIES:` drawers in `ctx` for valid top-of-file or post-heading placement.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let lines: Vec<&str> = ctx.source.content.split('\n').collect();
        let mut offset = 0;

        for (i, &line) in lines.iter().enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            if raw.trim().eq_ignore_ascii_case(":PROPERTIES:")
                && !is_valid_drawer_placement(&lines, i)
            {
                let (line_num, col) = ctx.source.line_col(offset);
                diagnostics.push(Diagnostic {
                    file: ctx.source.path.clone(),
                    line: line_num,
                    column: col,
                    severity: Severity::Warning,
                    rule_id: self.id(),
                    rule: self.name(),
                    message: "property drawer is not directly after a heading".to_string(),
                    fix: None,
                });
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

    /// Runs `MisplacedPropertyDrawer` against `input` using default configuration.
    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        MisplacedPropertyDrawer.check(&ctx)
    }

    /// Verifies that a `:PROPERTIES:` drawer immediately following a heading passes.
    #[test]
    fn properly_placed() {
        let input = "* Heading\n:PROPERTIES:\n:ID: abc\n:END:\n";
        assert!(check_it(input).is_empty());
    }

    /// Verifies that a `:PROPERTIES:` drawer after a heading's planning line passes.
    #[test]
    fn with_planning_line() {
        let input = "* TODO Task\nSCHEDULED: <2024-01-01>\n:PROPERTIES:\n:ID: abc\n:END:\n";
        assert!(check_it(input).is_empty());
    }

    /// Verifies that a `:PROPERTIES:` drawer after body text warns (`W003`).
    #[test]
    fn misplaced_after_text() {
        let input = "* Heading\nSome text\n:PROPERTIES:\n:ID: abc\n:END:\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("not directly after a heading"));
    }

    /// Verifies that a `:PROPERTIES:` drawer separated from its heading by a blank line warns (`W003`).
    #[test]
    fn misplaced_after_blank_line() {
        let input = "* Heading\n\n:PROPERTIES:\n:ID: abc\n:END:\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
    }

    /// Verifies that a `:PROPERTIES:` drawer on line 1 passes.
    #[test]
    fn top_level_properties() {
        let input = ":PROPERTIES:\n:ID: file-level\n:END:\n* Heading\n";
        assert!(check_it(input).is_empty());
    }

    /// Verifies that a top-of-file `:PROPERTIES:` drawer preceded by an Emacs mode comment passes.
    #[test]
    fn top_level_properties_after_mode_comment() {
        let input = "# -*- mode: org; -*-\n:PROPERTIES:\n:ID: roam-1\n:END:\n#+title: Note\n";
        assert!(check_it(input).is_empty());
    }

    /// Verifies that a top-of-file `:PROPERTIES:` drawer preceded by multiple `# comment` lines passes.
    #[test]
    fn top_level_properties_after_multiple_comments() {
        let input = "# First comment\n  # Indented comment\n#\n:PROPERTIES:\n:ID: roam-2\n:END:\n";
        assert!(check_it(input).is_empty());
    }

    /// Verifies that a `:PROPERTIES:` drawer preceded by a blank line at top of file warns (`W003`).
    #[test]
    fn misplaced_top_level_after_blank_line() {
        let input = "\n:PROPERTIES:\n:ID: bad\n:END:\n* Heading\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id, "W003");
    }

    /// Verifies that a `:PROPERTIES:` drawer preceded by `#+TITLE:` warns (`W003`).
    #[test]
    fn misplaced_top_level_after_title_keyword() {
        let input = "#+TITLE: My Note\n:PROPERTIES:\n:ID: bad\n:END:\n* Heading\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id, "W003");
    }

    /// Verifies that a `:PROPERTIES:` drawer preceded by body text before any heading warns (`W003`).
    #[test]
    fn misplaced_top_level_after_body_text() {
        let input = "Preamble paragraph.\n:PROPERTIES:\n:ID: bad\n:END:\n* Heading\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id, "W003");
    }

    /// Verifies that `#hashtag` without a space/tab after `#` is body text and triggers `W003`.
    #[test]
    fn misplaced_top_level_after_hashtag_text() {
        let input = "#hashtag\n:PROPERTIES:\n:ID: bad\n:END:\n* Heading\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id, "W003");
    }
}
