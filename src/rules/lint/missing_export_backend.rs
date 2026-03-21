// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects `#+BEGIN_EXPORT` blocks without a backend name.
///
/// Export blocks require a backend argument (e.g. `html`, `latex`) to specify
/// the output format. A bare `#+BEGIN_EXPORT` without a backend will be ignored
/// during export.
///
/// **Spec:** [Blocks](https://orgmode.org/manual/Blocks.html),
/// [Export Blocks (syntax)](https://orgmode.org/worg/org-syntax.html#Blocks)
///
/// **org-lint:** `missing-backend-in-export-block`
///
/// # Example
///
/// ```org
/// ;; Bad — no backend
/// #+BEGIN_EXPORT
/// <div>content</div>
/// #+END_EXPORT
///
/// ;; Good
/// #+BEGIN_EXPORT html
/// <div>content</div>
/// #+END_EXPORT
/// ```
pub struct MissingExportBackend;

impl LintRule for MissingExportBackend {
    fn id(&self) -> &'static str {
        "W013"
    }

    fn name(&self) -> &'static str {
        "missing-export-backend"
    }

    fn description(&self) -> &'static str {
        "Detect #+BEGIN_EXPORT without a backend name"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();
            let upper = trimmed.to_uppercase();

            if upper == "#+BEGIN_EXPORT" || upper.starts_with("#+BEGIN_EXPORT ") {
                let after = if upper.len() > 15 {
                    trimmed[15..].trim()
                } else {
                    ""
                };
                if after.is_empty() {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "#+BEGIN_EXPORT is missing a backend name (e.g., html, latex)"
                            .to_string(),
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
        MissingExportBackend.check(&ctx)
    }

    #[test]
    fn with_backend() {
        assert!(check_it("#+BEGIN_EXPORT html\n<div></div>\n#+END_EXPORT\n").is_empty());
    }

    #[test]
    fn without_backend() {
        let diags = check_it("#+BEGIN_EXPORT\ncontent\n#+END_EXPORT\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing a backend"));
    }

    #[test]
    fn src_block_not_flagged() {
        assert!(check_it("#+BEGIN_SRC python\ncode\n#+END_SRC\n").is_empty());
    }
}
