// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects duplicate `:CUSTOM_ID:` property values within a file.
///
/// Spec: [Manual: Property Syntax](https://orgmode.org/manual/Property-Syntax.html),
/// [Syntax: Property Drawers](https://orgmode.org/worg/org-syntax.html#Property_Drawers)
///
/// org-lint: `duplicate-custom-id`
///
/// Each `:CUSTOM_ID:` must be unique within a document because it serves as
/// an internal link target. Duplicates cause ambiguous link resolution.
pub struct DuplicateCustomId;

impl LintRule for DuplicateCustomId {
    fn id(&self) -> &'static str {
        "E002"
    }

    fn name(&self) -> &'static str {
        "duplicate-custom-id"
    }

    fn description(&self) -> &'static str {
        "Detect duplicate :CUSTOM_ID: values"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut seen: HashMap<String, (usize, usize)> = HashMap::new(); // id -> (line, col)
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            if let Some(id) = extract_custom_id(trimmed) {
                let (line_num, col) = ctx.source.line_col(offset);
                if let Some(&(first_line, _)) = seen.get(&id) {
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Error,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!(
                            "duplicate CUSTOM_ID \"{}\" (first defined at line {})",
                            id, first_line
                        ),
                        fix: None,
                    });
                } else {
                    seen.insert(id, (line_num, col));
                }
            }

            offset += line.len() + 1;
        }

        diagnostics
    }
}

/// Extracts the value from a `:CUSTOM_ID: value` property line.
fn extract_custom_id(line: &str) -> Option<String> {
    if !line.starts_with(":CUSTOM_ID:") {
        return None;
    }
    let value = line[":CUSTOM_ID:".len()..].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceFile;
    use crate::config::Config;

    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        DuplicateCustomId.check(&ctx)
    }

    #[test]
    fn no_duplicates() {
        let input = ":PROPERTIES:\n:CUSTOM_ID: a\n:END:\n:PROPERTIES:\n:CUSTOM_ID: b\n:END:\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn detects_duplicate() {
        let input =
            ":PROPERTIES:\n:CUSTOM_ID: same\n:END:\n:PROPERTIES:\n:CUSTOM_ID: same\n:END:\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("duplicate CUSTOM_ID"));
        assert!(diags[0].message.contains("same"));
    }

    #[test]
    fn no_custom_ids() {
        assert!(check_it("* Heading\ntext\n").is_empty());
    }
}
