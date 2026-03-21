// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

/// Detects duplicate `<<target>>` dedicated target definitions.
///
/// Spec: [Manual: Internal Links](https://orgmode.org/manual/Internal-Links.html),
/// [Syntax: Targets and Radio Targets](https://orgmode.org/worg/org-syntax.html#Targets_and_Radio_Targets)
///
/// org-lint: `duplicate-target`
///
/// Dedicated targets (`<<target>>`) serve as internal link anchors. Duplicates
/// cause ambiguous link resolution. Radio targets (`<<<target>>>`) are excluded.
pub struct DuplicateTarget;

impl LintRule for DuplicateTarget {
    fn id(&self) -> &'static str {
        "E004"
    }

    fn name(&self) -> &'static str {
        "duplicate-target"
    }

    fn description(&self) -> &'static str {
        "Detect duplicate <<target>> definitions"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            // Find all <<target>> patterns in this line.
            let mut search = raw;
            while let Some(start) = search.find("<<") {
                let rest = &search[start + 2..];
                if let Some(end) = rest.find(">>") {
                    let target = rest[..end].trim().to_string();
                    if !target.is_empty() && !target.contains('<') {
                        let (line_num, _) = ctx.source.line_col(offset);
                        if let Some(&first_line) = seen.get(&target) {
                            diagnostics.push(Diagnostic {
                                file: ctx.source.path.clone(),
                                line: line_num,
                                column: 1,
                                severity: Severity::Error,
                                rule_id: self.id(),
                                rule: self.name(),
                                message: format!(
                                    "duplicate target <<{}>> (first defined at line {})",
                                    target, first_line
                                ),
                                fix: None,
                            });
                        } else {
                            seen.insert(target, line_num);
                        }
                    }
                    search = &rest[end + 2..];
                } else {
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
        DuplicateTarget.check(&ctx)
    }

    #[test]
    fn no_duplicates() {
        assert!(check_it("<<target1>>\n<<target2>>\n").is_empty());
    }

    #[test]
    fn detects_duplicate() {
        let diags = check_it("<<foo>>\ntext\n<<foo>>\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("duplicate target"));
    }

    #[test]
    fn no_targets() {
        assert!(check_it("just text\n").is_empty());
    }

    #[test]
    fn radio_targets_not_confused() {
        // <<<radio>>> should not match as <<radio>>.
        assert!(check_it("<<<radio>>>\n<<<radio>>>\n").is_empty());
    }
}
