// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects duplicate footnote definition labels (`[fn:label]`).
///
/// Spec: [Manual: Creating Footnotes](https://orgmode.org/manual/Creating-Footnotes.html),
/// [Syntax: Footnote Definitions](https://orgmode.org/worg/org-syntax.html#Footnote_Definitions)
///
/// org-lint: `duplicate-footnote-definition`
///
/// Each footnote label must have exactly one definition. Duplicates are
/// ambiguous and only the last definition takes effect in Emacs.
pub struct DuplicateFootnoteDefinition;

impl LintRule for DuplicateFootnoteDefinition {
    fn id(&self) -> &'static str {
        "E005"
    }

    fn name(&self) -> &'static str {
        "duplicate-footnote-definition"
    }

    fn description(&self) -> &'static str {
        "Detect duplicate footnote definition labels"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            // Footnote definition: [fn:label] at start of line.
            if let Some(after_prefix) = trimmed.strip_prefix("[fn:") {
                if let Some(end) = after_prefix.find(']') {
                    let label = &after_prefix[..end];
                    // Skip inline footnotes (contain `:` in the label part).
                    if !label.is_empty() && !label.contains(':') {
                        let (line_num, _) = ctx.source.line_col(offset);
                        if let Some(&first_line) = seen.get(label) {
                            diagnostics.push(Diagnostic {
                                file: ctx.source.path.clone(),
                                line: line_num,
                                column: 1,
                                severity: Severity::Error,
                                rule_id: self.id(),
                                rule: self.name(),
                                message: format!(
                                    "duplicate footnote definition [fn:{}] (first at line {})",
                                    label, first_line
                                ),
                                fix: None,
                            });
                        } else {
                            seen.insert(label.to_string(), line_num);
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
        DuplicateFootnoteDefinition.check(&ctx)
    }

    #[test]
    fn no_duplicates() {
        assert!(check_it("[fn:1] First.\n[fn:2] Second.\n").is_empty());
    }

    #[test]
    fn detects_duplicate() {
        let diags = check_it("[fn:1] First.\n[fn:1] Duplicate.\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("duplicate footnote"));
    }

    #[test]
    fn no_footnotes() {
        assert!(check_it("text\n").is_empty());
    }
}
