// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects affiliated keywords not attached to any element.
///
/// Affiliated keywords (`#+CAPTION:`, `#+NAME:`, `#+ATTR_*:`, `#+HEADER:`,
/// `#+PLOT:`) must appear directly above a block, table, or other attachable
/// element. A blank line between the keyword and its target, plain text
/// following the keyword, or the keyword appearing at end-of-file all
/// indicate an orphaned keyword.
///
/// **Spec:** [Affiliated Keywords (syntax)](https://orgmode.org/worg/org-syntax.html#Affiliated_Keywords)
///
/// **org-lint:** `orphaned-affiliated-keywords`
///
/// # Example
///
/// ```org
/// ;; Bad — blank line separates keyword from table
/// #+CAPTION: My table
///
/// | a | b |
///
/// ;; Good
/// #+CAPTION: My table
/// | a | b |
/// ```
pub struct OrphanedAffiliatedKeywords;

/// Returns `true` if the line is an affiliated keyword.
fn is_affiliated(line: &str) -> bool {
    let upper = line.to_uppercase();
    upper.starts_with("#+CAPTION:")
        || upper.starts_with("#+NAME:")
        || upper.starts_with("#+ATTR_")
        || upper.starts_with("#+HEADER:")
        || upper.starts_with("#+HEADERS:")
        || upper.starts_with("#+PLOT:")
}

/// Returns `true` if the line is an element that can receive affiliated keywords.
fn is_attachable_element(line: &str) -> bool {
    let trimmed = line.trim_start();
    // Tables, blocks, links on their own line, images.
    trimmed.starts_with('|')
        || trimmed.starts_with("#+BEGIN")
        || trimmed.starts_with("#+begin")
        || trimmed.starts_with("#+CALL")
        || trimmed.starts_with("#+call")
        || trimmed.starts_with("[[")
        || trimmed.starts_with("#+INCLUDE")
        || trimmed.starts_with("#+include")
}

impl LintRule for OrphanedAffiliatedKeywords {
    fn id(&self) -> &'static str {
        "W021"
    }

    fn name(&self) -> &'static str {
        "orphaned-affiliated-keywords"
    }

    fn description(&self) -> &'static str {
        "Detect affiliated keywords not attached to any element"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let lines: Vec<&str> = ctx.source.content.split('\n').collect();
        let mut offset = 0;

        for (i, &line) in lines.iter().enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            if is_affiliated(trimmed) {
                // Check the next non-blank, non-affiliated line.
                let mut j = i + 1;
                while j < lines.len() {
                    let next = lines[j].strip_suffix('\r').unwrap_or(lines[j]).trim_start();
                    if next.trim().is_empty() {
                        // Blank line between affiliated keyword and element = orphaned.
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message:
                                "affiliated keyword is separated from its element by a blank line"
                                    .to_string(),
                            fix: None,
                        });
                        break;
                    } else if is_affiliated(next) {
                        // Another affiliated keyword — skip, check the next.
                        j += 1;
                        continue;
                    } else if is_attachable_element(next) {
                        // Properly attached — no diagnostic.
                        break;
                    } else {
                        // Not an attachable element.
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: "affiliated keyword is not followed by an attachable element"
                                .to_string(),
                            fix: None,
                        });
                        break;
                    }
                }

                // End of file after affiliated keyword.
                if j >= lines.len() {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "affiliated keyword at end of file with no element".to_string(),
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
        OrphanedAffiliatedKeywords.check(&ctx)
    }

    #[test]
    fn properly_attached_to_table() {
        assert!(check_it("#+CAPTION: My table\n| a | b |\n").is_empty());
    }

    #[test]
    fn properly_attached_to_block() {
        assert!(check_it("#+NAME: my-code\n#+BEGIN_SRC python\ncode\n#+END_SRC\n").is_empty());
    }

    #[test]
    fn stacked_affiliated_keywords() {
        let input = "#+CAPTION: Caption\n#+NAME: name\n#+ATTR_LATEX: :width 0.5\n| a | b |\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn orphaned_by_blank_line() {
        let diags = check_it("#+CAPTION: orphaned\n\n| a | b |\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("blank line"));
    }

    #[test]
    fn orphaned_by_text() {
        let diags = check_it("#+CAPTION: orphaned\nSome text.\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("not followed by"));
    }

    #[test]
    fn orphaned_at_end_of_file() {
        // Trailing newline creates an empty last line, so this reports "blank line".
        let diags = check_it("#+NAME: orphaned\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn orphaned_at_end_of_file_no_newline() {
        let diags = check_it("#+NAME: orphaned");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("end of file"));
    }
}
