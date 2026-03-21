// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::Path;

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects `#+SETUPFILE:` pointing to a non-existent local file.
///
/// The `#+SETUPFILE:` keyword includes configuration from another org file.
/// If the referenced file does not exist, the include silently fails. This
/// rule resolves the path relative to the source file's directory. Remote
/// URLs (`http://`, `https://`) are skipped since their existence cannot be
/// verified offline.
///
/// **Spec:** [Export Settings](https://orgmode.org/manual/Export-Settings.html)
///
/// **org-lint:** `non-existent-setupfile-parameter`
///
/// # Example
///
/// ```org
/// ;; Will warn if ./setup.org does not exist
/// #+SETUPFILE: ./setup.org
/// ```
pub struct NonExistentSetupfile;

impl LintRule for NonExistentSetupfile {
    fn id(&self) -> &'static str {
        "W022"
    }

    fn name(&self) -> &'static str {
        "non-existent-setupfile"
    }

    fn description(&self) -> &'static str {
        "Detect #+SETUPFILE: pointing to non-existent files"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;
        let base_dir = ctx.source.path.parent().unwrap_or(Path::new("."));

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            // Case-insensitive match for #+SETUPFILE:
            let has_setupfile = trimmed.len() >= 12
                && trimmed.as_bytes()[0] == b'#'
                && trimmed.as_bytes()[1] == b'+'
                && trimmed[2..12].eq_ignore_ascii_case("SETUPFILE:");

            if has_setupfile {
                let path_str = trimmed[12..].trim();
                // Remove quotes if present.
                let path_str = path_str
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(path_str);

                // Skip URLs.
                if !path_str.is_empty()
                    && !path_str.starts_with("http://")
                    && !path_str.starts_with("https://")
                {
                    let resolved = base_dir.join(path_str);
                    if !resolved.exists() {
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!("#+SETUPFILE: '{}' does not exist", path_str),
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
        NonExistentSetupfile.check(&ctx)
    }

    #[test]
    fn no_setupfile() {
        assert!(check_it("#+TITLE: test\n").is_empty());
    }

    #[test]
    fn missing_file() {
        let diags = check_it("#+SETUPFILE: ./nonexistent-setup.org\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("does not exist"));
    }

    #[test]
    fn url_not_flagged() {
        assert!(check_it("#+SETUPFILE: https://example.com/setup.org\n").is_empty());
    }

    #[test]
    fn quoted_path() {
        let diags = check_it("#+SETUPFILE: \"./nonexistent.org\"\n");
        assert_eq!(diags.len(), 1);
    }
}
