// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects malformed `#+CALL:` syntax.
//!
//! org-lint: `invalid-babel-call-block`
//!
//! Valid: `#+CALL: name(args)` or `#+CALL: name[header](args)[header]`.
//! Invalid: missing call name, missing parentheses.

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Validates `#+CALL:` lines for correct org-babel call syntax.
///
/// Reports a warning when the function name is missing or when parentheses
/// are absent. The expected form is `#+CALL: name(args)` with optional
/// header argument blocks in square brackets.
///
/// Spec: [§16.4 Evaluating Code Blocks](https://orgmode.org/manual/Evaluating-Code-Blocks.html)
/// org-lint: `invalid-babel-call-block`
pub struct InvalidBabelCall;

impl LintRule for InvalidBabelCall {
    fn id(&self) -> &'static str {
        "W024"
    }

    fn name(&self) -> &'static str {
        "invalid-babel-call"
    }

    fn description(&self) -> &'static str {
        "Detect malformed #+CALL: syntax"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            // Case-insensitive match for #+CALL:
            let has_call = trimmed.len() >= 7
                && trimmed.as_bytes()[0] == b'#'
                && trimmed.as_bytes()[1] == b'+'
                && trimmed[2..7].eq_ignore_ascii_case("CALL:");

            if has_call {
                let rest = trimmed[7..].trim();

                if rest.is_empty() {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "#+CALL: is missing the function name".to_string(),
                        fix: None,
                    });
                } else if !rest.contains('(') {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "#+CALL: is missing parentheses — expected name(args)".to_string(),
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
        InvalidBabelCall.check(&ctx)
    }

    #[test]
    fn valid_call() {
        assert!(check_it("#+CALL: my-function()\n").is_empty());
    }

    #[test]
    fn valid_call_with_args() {
        assert!(check_it("#+CALL: func(x=1, y=2)\n").is_empty());
    }

    #[test]
    fn valid_call_with_headers() {
        assert!(check_it("#+CALL: func[:results output]()\n").is_empty());
    }

    #[test]
    fn missing_name() {
        let diags = check_it("#+CALL:\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing the function name"));
    }

    #[test]
    fn missing_parens() {
        let diags = check_it("#+CALL: func\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing parentheses"));
    }

    #[test]
    fn no_call_lines() {
        assert!(check_it("#+TITLE: test\n").is_empty());
    }
}
