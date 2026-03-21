// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Warns on unrecognized `#+BEGIN_TYPE` block types.
//!
//! Spec: [§2.6 Blocks](https://orgmode.org/manual/Blocks.html)
//!
//! Known types: SRC, EXAMPLE, QUOTE, VERSE, CENTER, COMMENT, EXPORT.
//! Custom block types are valid but uncommon; warn at Info severity.

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Flags `#+BEGIN_TYPE` lines whose block type is not in the standard set.
///
/// Compares against [`KNOWN_BLOCK_TYPES`] (case-insensitive). Reports at
/// [`Severity::Info`] since custom block types are valid in org-mode. Only
/// warns once per block type to avoid duplicate diagnostics from the
/// matching `#+END_TYPE` line.
///
/// Spec: [§2.6 Blocks](https://orgmode.org/manual/Blocks.html)
pub struct BlockTypeValidity;

const KNOWN_BLOCK_TYPES: &[&str] = &[
    "SRC", "EXAMPLE", "QUOTE", "VERSE", "CENTER", "COMMENT", "EXPORT",
    // Also valid: dynamic blocks use #+BEGIN: name (with colon, different syntax).
    // org-reveal adds NOTES.
    "NOTES",
];

impl LintRule for BlockTypeValidity {
    fn id(&self) -> &'static str {
        "I004"
    }

    fn name(&self) -> &'static str {
        "block-type-validity"
    }

    fn description(&self) -> &'static str {
        "Warn on unrecognized #+BEGIN_ block types"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;
        // Track which types we've already warned about to avoid duplicate warnings
        // for BEGIN and END of the same block.
        let mut warned_types: std::collections::HashSet<String> = std::collections::HashSet::new();

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();
            let upper = trimmed.to_uppercase();

            if let Some(rest) = upper.strip_prefix("#+BEGIN_") {
                let block_type = rest.split_whitespace().next().unwrap_or("");
                if !block_type.is_empty()
                    && !KNOWN_BLOCK_TYPES.contains(&block_type)
                    && !warned_types.contains(block_type)
                {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Info,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!("unrecognized block type #+BEGIN_{}", block_type),
                        fix: None,
                    });
                    warned_types.insert(block_type.to_string());
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
        BlockTypeValidity.check(&ctx)
    }

    #[test]
    fn known_types() {
        assert!(check_it("#+BEGIN_SRC python\ncode\n#+END_SRC\n").is_empty());
        assert!(check_it("#+BEGIN_QUOTE\ntext\n#+END_QUOTE\n").is_empty());
        assert!(check_it("#+BEGIN_EXAMPLE\ntext\n#+END_EXAMPLE\n").is_empty());
    }

    #[test]
    fn unknown_type() {
        let diags = check_it("#+BEGIN_FOOBAR\nstuff\n#+END_FOOBAR\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Info);
    }

    #[test]
    fn notes_block_ok() {
        assert!(check_it("#+BEGIN_NOTES\nspeaker notes\n#+END_NOTES\n").is_empty());
    }

    #[test]
    fn no_blocks() {
        assert!(check_it("text\n").is_empty());
    }
}
