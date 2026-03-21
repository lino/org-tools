// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::{HashMap, HashSet};

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Detects footnotes that are defined but never referenced, or referenced but never defined.
///
/// Spec: [Manual: Creating Footnotes](https://orgmode.org/manual/Creating-Footnotes.html),
/// [Syntax: Footnote Definitions](https://orgmode.org/worg/org-syntax.html#Footnote_Definitions)
///
/// org-lint: `orphaned-footnote-definitions` / `undefined-footnote-reference`
///
/// Inline footnotes (`[fn:label:text]`) are self-contained and count as both
/// definition and reference. Content inside protected regions is ignored.
pub struct OrphanedFootnotes;

impl LintRule for OrphanedFootnotes {
    fn id(&self) -> &'static str {
        "W004"
    }

    fn name(&self) -> &'static str {
        "orphaned-footnotes"
    }

    fn description(&self) -> &'static str {
        "Detect footnotes defined but not referenced, or vice versa"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut definitions: HashMap<String, (usize, usize)> = HashMap::new();
        let mut references: HashSet<String> = HashSet::new();
        let mut inline_defs: HashSet<String> = HashSet::new(); // inline footnotes count as both
        let mut diagnostics = Vec::new();
        let mut offset = 0;
        let regions = protected_regions(&ctx.source.content);

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            // Skip lines inside protected regions (code blocks, etc.)
            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            // Footnote definition: `[fn:label] definition text`
            // Must be at the start of a line.
            let is_def = if let Some(label) = extract_footnote_def(raw) {
                let (line_num, col) = ctx.source.line_col(offset);
                definitions.insert(label, (line_num, col));
                true
            } else {
                false
            };

            // Footnote references: `[fn:label]` or `[fn:label:inline def]` anywhere in the line.
            // Skip the definition token at the start of definition lines.
            let ref_search = if is_def {
                // Skip past the `[fn:label]` at the start.
                let trimmed = raw.trim_start();
                if let Some(end) = trimmed.find(']') {
                    &raw[raw.len() - trimmed.len() + end + 1..]
                } else {
                    raw
                }
            } else {
                raw
            };
            for (label, is_inline) in extract_footnote_refs(ref_search) {
                references.insert(label.clone());
                if is_inline {
                    inline_defs.insert(label);
                }
            }

            offset += line.len() + 1;
        }

        // Definitions without references.
        for (label, (line, col)) in &definitions {
            if !references.contains(label) {
                diagnostics.push(Diagnostic {
                    file: ctx.source.path.clone(),
                    line: *line,
                    column: *col,
                    severity: Severity::Warning,
                    rule_id: self.id(),
                    rule: self.name(),
                    message: format!("footnote [fn:{}] is defined but never referenced", label),
                    fix: None,
                });
            }
        }

        // References without definitions (excluding inline footnotes which are self-contained).
        for label in &references {
            if !definitions.contains_key(label) && !inline_defs.contains(label) {
                diagnostics.push(Diagnostic {
                    file: ctx.source.path.clone(),
                    line: 0,
                    column: 0,
                    severity: Severity::Warning,
                    rule_id: self.id(),
                    rule: self.name(),
                    message: format!("footnote [fn:{}] is referenced but never defined", label),
                    fix: None,
                });
            }
        }

        diagnostics.sort_by_key(|d| (d.line, d.column));
        diagnostics
    }
}

/// Extract a footnote definition label from a line like `[fn:label] text`.
fn extract_footnote_def(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("[fn:") {
        return None;
    }
    let rest = &trimmed[4..];
    let end = rest.find(']')?;
    let label = &rest[..end];
    // A definition has no `:` in the label part (that would be an inline definition reference).
    if label.is_empty() || label.contains(':') {
        return None;
    }
    Some(label.to_string())
}

/// Extract all footnote reference labels from a line.
/// Returns (label, is_inline) where is_inline is true for `[fn:label:text]`.
fn extract_footnote_refs(line: &str) -> Vec<(String, bool)> {
    let mut refs = Vec::new();
    let mut search = line;

    while let Some(pos) = search.find("[fn:") {
        let rest = &search[pos + 4..];
        if let Some(end) = rest.find(']') {
            let content = &rest[..end];
            // Could be `label` or `label:inline text`.
            let (label, is_inline) = if let Some(colon_pos) = content.find(':') {
                (&content[..colon_pos], true)
            } else {
                (content, false)
            };
            if !label.is_empty() {
                refs.push((label.to_string(), is_inline));
            }
            search = &rest[end + 1..];
        } else {
            break;
        }
    }

    refs
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
        OrphanedFootnotes.check(&ctx)
    }

    #[test]
    fn matched_footnotes() {
        let input = "Text with [fn:1] reference.\n\n[fn:1] The definition.\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn unreferenced_definition() {
        let input = "[fn:unused] This footnote is never referenced.\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("defined but never referenced"));
    }

    #[test]
    fn undefined_reference() {
        let input = "Text with [fn:missing] reference.\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("referenced but never defined"));
    }

    #[test]
    fn inline_footnote_not_orphaned() {
        // Inline footnotes [fn:label:text] are self-contained — they provide
        // both the definition and reference. No diagnostic should be emitted.
        let input = "Text with [fn:x:inline definition] here.\n";
        let diags = check_it(input);
        assert!(diags.is_empty());
    }

    #[test]
    fn footnotes_in_code_block_ignored() {
        let input = "#+BEGIN_SRC org\n[fn:inside] Not a real footnote.\nText [fn:also] not real.\n#+END_SRC\n";
        let diags = check_it(input);
        assert!(diags.is_empty());
    }

    #[test]
    fn no_footnotes() {
        assert!(check_it("just text\n").is_empty());
    }

    #[test]
    fn multiple_refs_one_def() {
        let input = "Ref [fn:a] and again [fn:a].\n\n[fn:a] Definition.\n";
        assert!(check_it(input).is_empty());
    }
}
