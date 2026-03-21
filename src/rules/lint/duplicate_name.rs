// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects duplicate `#+NAME:` values across the document.
///
/// Spec: [Syntax: Affiliated Keywords](https://orgmode.org/worg/org-syntax.html#Affiliated_Keywords)
///
/// org-lint: `duplicate-name`
///
/// `#+NAME:` assigns a referenceable name to an element. Each name must be unique
/// within a file; duplicates cause ambiguous `#+CALL:` and `noweb` references.
pub struct DuplicateName;

impl LintRule for DuplicateName {
    fn id(&self) -> &'static str {
        "E003"
    }

    fn name(&self) -> &'static str {
        "duplicate-name"
    }

    fn description(&self) -> &'static str {
        "Detect duplicate #+NAME: values"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            // Case-insensitive check for "#+NAME:" prefix.
            let has_name = trimmed.len() >= 7
                && trimmed.as_bytes()[0] == b'#'
                && trimmed.as_bytes()[1] == b'+'
                && trimmed[2..6].eq_ignore_ascii_case("NAME")
                && trimmed.as_bytes()[6] == b':';
            if has_name {
                let value = trimmed[7..].trim().to_string();
                if !value.is_empty() {
                    let (line_num, _) = ctx.source.line_col(offset);
                    if let Some(&first_line) = seen.get(&value) {
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: 1,
                            severity: Severity::Error,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                "duplicate #+NAME: \"{}\" (first defined at line {})",
                                value, first_line
                            ),
                            fix: None,
                        });
                    } else {
                        seen.insert(value, line_num);
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
        DuplicateName.check(&ctx)
    }

    #[test]
    fn no_duplicates() {
        assert!(check_it("#+NAME: a\n#+NAME: b\n").is_empty());
    }

    #[test]
    fn detects_duplicate() {
        let diags = check_it("#+NAME: foo\ntext\n#+NAME: foo\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("duplicate #+NAME:"));
    }

    #[test]
    fn no_names() {
        assert!(check_it("just text\n").is_empty());
    }
}
