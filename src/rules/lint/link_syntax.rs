// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects malformed bracket link syntax.
//!
//! Spec: [§4.1 Link Format](https://orgmode.org/manual/Link-Format.html),
//! [Syntax: Regular Links](https://orgmode.org/worg/org-syntax.html#Regular_Links)
//!
//! Valid: `[[target]]`, `[[target][description]]`.
//! Invalid: unclosed brackets, empty targets.

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Checks bracket links for structural correctness.
///
/// Detects two problems: unclosed links (`[[` without a matching `]]`) and
/// links with an empty target (`[[][description]]`). Skips content inside
/// protected regions.
///
/// Spec: [§4.1 Link Format](https://orgmode.org/manual/Link-Format.html),
/// [Syntax: Regular Links](https://orgmode.org/worg/org-syntax.html#Regular_Links)
pub struct LinkSyntax;

impl LintRule for LinkSyntax {
    fn id(&self) -> &'static str {
        "W028"
    }

    fn name(&self) -> &'static str {
        "link-syntax"
    }

    fn description(&self) -> &'static str {
        "Detect malformed bracket link syntax"
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

            let mut search = raw;
            while let Some(pos) = search.find("[[") {
                let rest = &search[pos + 2..];

                if let Some(close) = rest.find("]]") {
                    let link_content = &rest[..close];
                    // Check for empty target.
                    let target = if let Some(desc_start) = link_content.find("][") {
                        &link_content[..desc_start]
                    } else {
                        link_content
                    };

                    if target.trim().is_empty() {
                        let (line_num, _) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: 1,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: "link has an empty target".to_string(),
                            fix: None,
                        });
                    }

                    search = &rest[close + 2..];
                } else {
                    // Unclosed link — no matching ]].
                    let (line_num, _) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: 1,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "unclosed link — [[ without matching ]]".to_string(),
                        fix: None,
                    });
                    break;
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
        LinkSyntax.check(&ctx)
    }

    #[test]
    fn valid_link() {
        assert!(check_it("[[https://example.com]]\n").is_empty());
    }

    #[test]
    fn valid_link_with_desc() {
        assert!(check_it("[[https://example.com][text]]\n").is_empty());
    }

    #[test]
    fn unclosed_link() {
        let diags = check_it("[[unclosed link\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unclosed"));
    }

    #[test]
    fn empty_target() {
        let diags = check_it("[[][description]]\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("empty target"));
    }

    #[test]
    fn in_code_block() {
        let input = "#+BEGIN_SRC org\n[[broken\n#+END_SRC\n";
        assert!(check_it(input).is_empty());
    }
}
