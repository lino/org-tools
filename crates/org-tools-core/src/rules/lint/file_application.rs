// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Detects deprecated `file+application:` link types.
///
/// The `file+sys:` and `file+emacs:` link prefixes are deprecated in modern
/// org-mode. They should be replaced with plain `file:` links. This rule skips
/// content inside protected regions (source blocks, example blocks, etc.).
///
/// **Spec:** [Handling Links](https://orgmode.org/manual/Handling-Links.html),
/// [External Links](https://orgmode.org/manual/External-Links.html)
///
/// **org-lint:** `file-application`
///
/// # Example
///
/// ```org
/// ;; Bad — deprecated prefix
/// [[file+sys:/path/to/file]]
///
/// ;; Good
/// [[file:/path/to/file]]
/// ```
pub struct FileApplication;

impl LintRule for FileApplication {
    fn id(&self) -> &'static str {
        "W016"
    }

    fn name(&self) -> &'static str {
        "file-application"
    }

    fn description(&self) -> &'static str {
        "Detect deprecated file+application: link types"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);
        let mut offset = 0;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            // Search for [[file+ pattern in links.
            let mut search = raw;
            while let Some(pos) = search.find("[[file+") {
                let (line_num, _) = ctx.source.line_col(offset);
                diagnostics.push(Diagnostic {
                    file: ctx.source.path.clone(),
                    line: line_num,
                    column: 1,
                    severity: Severity::Warning,
                    rule_id: self.id(),
                    rule: self.name(),
                    message: "file+application: link type is deprecated — use file: instead"
                        .to_string(),
                    fix: None,
                });
                search = &search[pos + 7..];
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
        FileApplication.check(&ctx)
    }

    #[test]
    fn normal_file_link() {
        assert!(check_it("[[file:path/to/file]]\n").is_empty());
    }

    #[test]
    fn deprecated_file_sys() {
        let diags = check_it("[[file+sys:path/to/file]]\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn deprecated_file_emacs() {
        let diags = check_it("[[file+emacs:path/to/file]]\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn no_links() {
        assert!(check_it("text\n").is_empty());
    }
}
