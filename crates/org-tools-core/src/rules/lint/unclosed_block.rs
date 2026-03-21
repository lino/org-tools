// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects blocks with mismatched or missing `#+BEGIN_` / `#+END_` delimiters.
///
/// Spec: [Manual: Blocks](https://orgmode.org/manual/Blocks.html),
/// [Syntax: Greater Blocks](https://orgmode.org/worg/org-syntax.html#Greater_Blocks)
///
/// org-lint: `unclosed-block`
///
/// Reports an error for every `#+BEGIN_<type>` without a corresponding `#+END_<type>`,
/// and vice versa. Block type matching is case-insensitive.
pub struct UnclosedBlock;

impl LintRule for UnclosedBlock {
    fn id(&self) -> &'static str {
        "E001"
    }

    fn name(&self) -> &'static str {
        "unclosed-block"
    }

    fn description(&self) -> &'static str {
        "Detect blocks without matching #+END_"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut stack: Vec<(String, usize, usize)> = Vec::new(); // (block_type, line, offset)
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();
            let upper = trimmed.to_uppercase();

            if let Some(rest) = upper.strip_prefix("#+BEGIN_") {
                let block_type = rest.split_whitespace().next().unwrap_or("").to_string();
                if !block_type.is_empty() {
                    let (line_num, col) = ctx.source.line_col(offset);
                    stack.push((block_type, line_num, col));
                }
            } else if let Some(rest) = upper.strip_prefix("#+END_") {
                let block_type = rest.split_whitespace().next().unwrap_or("").to_string();
                if !block_type.is_empty() {
                    if let Some(pos) = stack.iter().rposition(|(bt, _, _)| bt == &block_type) {
                        stack.remove(pos);
                    } else {
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Error,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                "#+END_{} without matching #+BEGIN_{}",
                                block_type, block_type
                            ),
                            fix: None,
                        });
                    }
                }
            }

            offset += line.len() + 1;
        }

        // Remaining unclosed blocks.
        for (block_type, line_num, col) in stack {
            diagnostics.push(Diagnostic {
                file: ctx.source.path.clone(),
                line: line_num,
                column: col,
                severity: Severity::Error,
                rule_id: self.id(),
                rule: self.name(),
                message: format!(
                    "#+BEGIN_{} without matching #+END_{}",
                    block_type, block_type
                ),
                fix: None,
            });
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
        UnclosedBlock.check(&ctx)
    }

    #[test]
    fn matched_blocks() {
        let diags = check_it("#+BEGIN_SRC python\ncode\n#+END_SRC\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn unclosed_begin() {
        let diags = check_it("#+BEGIN_SRC python\ncode\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("without matching #+END_SRC"));
    }

    #[test]
    fn unmatched_end() {
        let diags = check_it("code\n#+END_SRC\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("without matching #+BEGIN_SRC"));
    }

    #[test]
    fn nested_blocks() {
        let input = "#+BEGIN_QUOTE\n#+BEGIN_SRC python\ncode\n#+END_SRC\n#+END_QUOTE\n";
        let diags = check_it(input);
        assert!(diags.is_empty());
    }

    #[test]
    fn multiple_unclosed() {
        let diags = check_it("#+BEGIN_SRC\n#+BEGIN_EXAMPLE\n");
        assert_eq!(diags.len(), 2);
    }

    #[test]
    fn case_insensitive() {
        let diags = check_it("#+begin_src python\ncode\n#+end_src\n");
        assert!(diags.is_empty());
    }
}
