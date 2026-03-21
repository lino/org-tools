// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects old-style `#+INCLUDE:` markup syntax.
///
/// Older versions of org-mode allowed specifying the export backend directly
/// after the file path (e.g. `#+INCLUDE: "file" HTML`). The current syntax
/// requires the `export` keyword before the backend name
/// (e.g. `#+INCLUDE: "file" export html`).
///
/// **Spec:** [Include Files](https://orgmode.org/manual/Include-Files.html)
///
/// **org-lint:** `obsolete-include-markup`
///
/// # Example
///
/// ```org
/// ;; Bad — old syntax
/// #+INCLUDE: "file.org" HTML
///
/// ;; Good — current syntax
/// #+INCLUDE: "file.org" export html
/// ```
pub struct ObsoleteIncludeMarkup;

const DEPRECATED_BACKENDS: &[&str] = &[
    "ASCII", "BEAMER", "HTML", "LATEX", "MAN", "MARKDOWN", "MD", "ODT", "ORG", "TEXINFO",
];

impl LintRule for ObsoleteIncludeMarkup {
    fn id(&self) -> &'static str {
        "W017"
    }

    fn name(&self) -> &'static str {
        "obsolete-include-markup"
    }

    fn description(&self) -> &'static str {
        "Detect old-style #+INCLUDE: backend syntax"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            // Safe byte-level check: #+INCLUDE: is pure ASCII.
            let has_include = trimmed.len() >= 10
                && trimmed.as_bytes()[0] == b'#'
                && trimmed.as_bytes()[1] == b'+'
                && trimmed[2..10].eq_ignore_ascii_case("INCLUDE:");
            if has_include {
                let rest = trimmed[10..].trim();
                // The path is typically in quotes. Find the end of the path.
                if let Some(after_open_quote) = rest.strip_prefix('"') {
                    if let Some(close_quote) = after_open_quote.find('"') {
                        let after_path = after_open_quote[close_quote + 1..].trim();
                        // Check if it's a bare backend name (not "export backend").
                        let first_word = after_path
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_uppercase();
                        if DEPRECATED_BACKENDS.contains(&first_word.as_str())
                            && !after_path.to_uppercase().starts_with("EXPORT")
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
                                    "old #+INCLUDE: syntax — use 'export {}' instead of '{}'",
                                    first_word.to_lowercase(),
                                    first_word
                                ),
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
        ObsoleteIncludeMarkup.check(&ctx)
    }

    #[test]
    fn valid_include() {
        assert!(check_it("#+INCLUDE: \"file.org\"\n").is_empty());
    }

    #[test]
    fn valid_export_syntax() {
        assert!(check_it("#+INCLUDE: \"file.org\" export html\n").is_empty());
    }

    #[test]
    fn obsolete_html() {
        let diags = check_it("#+INCLUDE: \"file.org\" HTML\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("export html"));
    }

    #[test]
    fn valid_src_include() {
        assert!(check_it("#+INCLUDE: \"file.py\" src python\n").is_empty());
    }
}
