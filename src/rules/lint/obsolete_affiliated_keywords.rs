// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects obsolete affiliated keywords and suggests their modern replacements.
///
/// Spec: [Syntax: Affiliated Keywords](https://orgmode.org/worg/org-syntax.html#Affiliated_Keywords)
///
/// org-lint: `obsolete-affiliated-keywords`
///
/// Obsolete keyword mappings:
/// - `DATA`, `LABEL`, `RESNAME`, `SOURCE`, `SRCNAME`, `TBLNAME` -> `NAME`
/// - `RESULT` -> `RESULTS`
/// - `HEADERS` -> `HEADER`
pub struct ObsoleteAffiliatedKeywords;

/// Returns the replacement for an obsolete keyword, if applicable.
fn obsolete_replacement(keyword: &str) -> Option<&'static str> {
    match keyword.to_uppercase().as_str() {
        "DATA" | "LABEL" | "RESNAME" | "SOURCE" | "SRCNAME" | "TBLNAME" => Some("NAME"),
        "RESULT" => Some("RESULTS"),
        "HEADERS" => Some("HEADER"),
        _ => None,
    }
}

impl LintRule for ObsoleteAffiliatedKeywords {
    fn id(&self) -> &'static str {
        "W006"
    }

    fn name(&self) -> &'static str {
        "obsolete-affiliated-keywords"
    }

    fn description(&self) -> &'static str {
        "Detect obsolete affiliated keywords"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            if let Some(rest) = trimmed.strip_prefix("#+") {
                let keyword_end = rest
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(rest.len());
                let keyword = &rest[..keyword_end];

                if let Some(replacement) = obsolete_replacement(keyword) {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!(
                            "#+{} is obsolete — use #+{} instead",
                            keyword, replacement
                        ),
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
        ObsoleteAffiliatedKeywords.check(&ctx)
    }

    #[test]
    fn no_obsolete() {
        assert!(check_it("#+NAME: foo\n#+RESULTS:\n").is_empty());
    }

    #[test]
    fn detects_srcname() {
        let diags = check_it("#+SRCNAME: foo\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("use #+NAME"));
    }

    #[test]
    fn detects_result() {
        let diags = check_it("#+RESULT: bar\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("use #+RESULTS"));
    }

    #[test]
    fn detects_headers() {
        let diags = check_it("#+HEADERS: baz\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("use #+HEADER"));
    }

    #[test]
    fn detects_data() {
        let diags = check_it("#+DATA: foo\n");
        assert_eq!(diags.len(), 1);
    }
}
