// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects drawers nested inside other drawers, which is invalid per spec.
//!
//! Spec: [§2.8 Drawers](https://orgmode.org/manual/Drawers.html)
//! -- "Drawers cannot be nested."

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Reports an error when a drawer is opened inside another drawer.
///
/// The org-mode spec explicitly forbids nested drawers. This rule tracks
/// open/close state via `:NAME:` and `:END:` lines, skipping content inside
/// protected regions. Property lines inside `:PROPERTIES:` drawers (e.g.
/// `:ID: value`) are not treated as drawer opens.
///
/// Spec: [§2.8 Drawers](https://orgmode.org/manual/Drawers.html)
pub struct DrawerNesting;

/// Returns `true` if the line opens a drawer (`:NAME:` pattern, excluding `:END:`).
fn is_drawer_open(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.starts_with(':') || !trimmed.ends_with(':') || trimmed.len() < 3 {
        return false;
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    if inner.is_empty() || inner.eq_ignore_ascii_case("END") {
        return false;
    }
    inner
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl LintRule for DrawerNesting {
    fn id(&self) -> &'static str {
        "E007"
    }

    fn name(&self) -> &'static str {
        "drawer-nesting"
    }

    fn description(&self) -> &'static str {
        "Detect drawers nested inside other drawers"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);
        let mut in_drawer = false;
        let mut drawer_is_properties = false;
        let mut offset = 0;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            let trimmed = raw.trim();

            if trimmed.eq_ignore_ascii_case(":END:") {
                in_drawer = false;
                drawer_is_properties = false;
            } else if is_drawer_open(trimmed) {
                if in_drawer && !drawer_is_properties {
                    // Nested drawer detected (skip PROPERTIES internal lines).
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Error,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "drawers cannot be nested".to_string(),
                        fix: None,
                    });
                } else {
                    in_drawer = true;
                    drawer_is_properties = trimmed.eq_ignore_ascii_case(":PROPERTIES:");
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
        DrawerNesting.check(&ctx)
    }

    #[test]
    fn single_drawer() {
        assert!(check_it(":LOGBOOK:\nCLOCK: ...\n:END:\n").is_empty());
    }

    #[test]
    fn sequential_drawers() {
        let input = ":PROPERTIES:\n:ID: a\n:END:\n:LOGBOOK:\nCLOCK: ...\n:END:\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn nested_drawer() {
        let input = ":RESULTS:\n:INNER:\nstuff\n:END:\n:END:\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("cannot be nested"));
    }

    #[test]
    fn properties_inside_not_flagged() {
        // Property lines like :ID: inside PROPERTIES aren't drawer opens.
        let input = ":PROPERTIES:\n:ID: abc\n:END:\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn in_code_block_ignored() {
        let input = "#+BEGIN_SRC org\n:OUTER:\n:INNER:\n:END:\n:END:\n#+END_SRC\n";
        assert!(check_it(input).is_empty());
    }
}
