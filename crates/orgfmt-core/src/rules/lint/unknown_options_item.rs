// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects unknown items in `#+OPTIONS:` keyword.
//!
//! Spec: [§17.2 Export Settings](https://orgmode.org/manual/Export-Settings.html)
//! org-lint: `unknown-options-item`
//!
//! Options are `key:value` pairs. Known keys are from the org export framework.

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Flags unrecognized `key:value` items in `#+OPTIONS:` lines.
///
/// Compares each key against the set of [`KNOWN_OPTIONS`] from the org-mode
/// export framework. Unknown keys are reported at [`Severity::Info`] since
/// backend-specific options may be valid in certain export contexts.
///
/// Spec: [§17.2 Export Settings](https://orgmode.org/manual/Export-Settings.html)
/// org-lint: `unknown-options-item`
pub struct UnknownOptionsItem;

/// Known `#+OPTIONS` keys from the org-mode export framework.
const KNOWN_OPTIONS: &[&str] = &[
    "'", "*", "-", ":", "<", "\\n", "^", "arch", "author", "broken-links",
    "c", "creator", "d", "date", "e", "email", "expand-links", "f", "h",
    "html-postamble", "html-preamble", "html-style", "html5-fancy",
    "inline", "num", "p", "pri", "prop", "reveal_", "stat", "tags",
    "tasks", "tex", "timestamp", "title", "toc", "todo", "|",
];

impl LintRule for UnknownOptionsItem {
    fn id(&self) -> &'static str {
        "I002"
    }

    fn name(&self) -> &'static str {
        "unknown-options-item"
    }

    fn description(&self) -> &'static str {
        "Detect unknown items in #+OPTIONS:"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            // Case-insensitive match for #+OPTIONS:
            let has_options = trimmed.len() >= 10
                && trimmed.as_bytes()[0] == b'#'
                && trimmed.as_bytes()[1] == b'+'
                && trimmed[2..10].eq_ignore_ascii_case("OPTIONS:");

            if has_options {
                let rest = trimmed[10..].trim();
                // Parse key:value pairs.
                for item in rest.split_whitespace() {
                    if let Some(colon_pos) = item.find(':') {
                        let key = &item[..colon_pos];
                        if !key.is_empty()
                            && !KNOWN_OPTIONS.contains(&key.to_lowercase().as_str())
                        {
                            let (line_num, _) = ctx.source.line_col(offset);
                            diagnostics.push(Diagnostic {
                                file: ctx.source.path.clone(),
                                line: line_num,
                                column: 1,
                                severity: Severity::Info,
                                rule_id: self.id(),
                                rule: self.name(),
                                message: format!(
                                    "unknown #+OPTIONS item '{}'",
                                    key
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
        UnknownOptionsItem.check(&ctx)
    }

    #[test]
    fn valid_options() {
        assert!(check_it("#+OPTIONS: toc:2 num:t\n").is_empty());
    }

    #[test]
    fn unknown_option() {
        let diags = check_it("#+OPTIONS: foobar:t\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("foobar"));
    }

    #[test]
    fn mixed_known_unknown() {
        let diags = check_it("#+OPTIONS: toc:2 badkey:t num:nil\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("badkey"));
    }

    #[test]
    fn no_options() {
        assert!(check_it("#+TITLE: test\n").is_empty());
    }
}
