// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects affiliated keywords separated from their element by blank lines.
//!
//! Spec: [§12.2 Captions](https://orgmode.org/manual/Captions.html),
//! [Syntax: Affiliated Keywords](https://orgmode.org/worg/org-syntax.html#Affiliated_Keywords)
//!
//! `#+CAPTION:`, `#+NAME:`, `#+ATTR_*:` must appear directly above their target
//! with no blank lines between.
//!
//! Note: this rule specifically checks for blank-line separation. The broader
//! `orphaned-affiliated-keywords` (W021) also checks for non-attachable elements.
//! This rule focuses on the common case of a stray blank line.

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Warns when an affiliated keyword (`#+CAPTION:`, `#+NAME:`, `#+ATTR_*:`,
/// `#+HEADER:`, `#+PLOT:`) is followed by a blank line before its target element.
///
/// Affiliated keywords must appear directly above the element they annotate
/// with no intervening blank lines. This complements the broader
/// `orphaned-affiliated-keywords` rule (W021) by focusing specifically on
/// the blank-line separation case.
///
/// Spec: [§12.2 Captions](https://orgmode.org/manual/Captions.html),
/// [Syntax: Affiliated Keywords](https://orgmode.org/worg/org-syntax.html#Affiliated_Keywords)
pub struct AffiliatedKeywordPlacement;

/// Returns `true` if the line is an affiliated keyword (`#+CAPTION:`,
/// `#+NAME:`, `#+ATTR_*:`, `#+HEADER:`, or `#+PLOT:`).
fn is_affiliated_keyword(line: &str) -> bool {
    let upper = line.to_uppercase();
    upper.starts_with("#+CAPTION:")
        || upper.starts_with("#+NAME:")
        || upper.starts_with("#+ATTR_")
        || upper.starts_with("#+HEADER:")
        || upper.starts_with("#+PLOT:")
}

impl LintRule for AffiliatedKeywordPlacement {
    fn id(&self) -> &'static str {
        "W032"
    }

    fn name(&self) -> &'static str {
        "affiliated-keyword-placement"
    }

    fn description(&self) -> &'static str {
        "Detect affiliated keywords separated from elements by blank lines"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let lines: Vec<&str> = ctx.source.content.split('\n').collect();
        let mut offset = 0;

        for (i, &line) in lines.iter().enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            if is_affiliated_keyword(trimmed) {
                // Check if the next line is blank.
                if i + 1 < lines.len() {
                    let next = lines[i + 1].strip_suffix('\r').unwrap_or(lines[i + 1]);
                    if next.trim().is_empty() {
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message:
                                "affiliated keyword should not be separated from its element by a blank line"
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
        AffiliatedKeywordPlacement.check(&ctx)
    }

    #[test]
    fn properly_placed() {
        assert!(check_it("#+CAPTION: My table\n| a | b |\n").is_empty());
    }

    #[test]
    fn separated_by_blank_line() {
        let diags = check_it("#+CAPTION: My table\n\n| a | b |\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("blank line"));
    }

    #[test]
    fn stacked_keywords_ok() {
        let input = "#+CAPTION: cap\n#+NAME: name\n| a | b |\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn no_keywords() {
        assert!(check_it("text\n").is_empty());
    }

    #[test]
    fn attr_keyword() {
        let diags = check_it("#+ATTR_LATEX: :width 0.5\n\n| a |\n");
        assert_eq!(diags.len(), 1);
    }
}
