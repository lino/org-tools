// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects footnotes defined but never referenced, or referenced but never defined (`W004`).
//!
//! Spec: [Manual: Creating Footnotes](https://orgmode.org/manual/Creating-Footnotes.html),
//! [Syntax: Footnote Definitions](https://orgmode.org/worg/org-syntax.html#Footnote_Definitions)
//! org-lint: `orphaned-footnote-definitions` / `undefined-footnote-reference`

use std::collections::{HashMap, HashSet};

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Detects footnotes that are defined but never referenced, or referenced but never defined (`W004`).
///
/// Inline footnotes (`[fn:label:text]`) are self-contained and count as both
/// definition and reference. Content inside protected regions, drawers, comments,
/// and inline verbatim spans is ignored.
///
/// Spec: [Manual: Creating Footnotes](https://orgmode.org/manual/Creating-Footnotes.html)
/// Org Syntax: <https://orgmode.org/worg/org-syntax.html#Footnote_Definitions>
pub struct OrphanedFootnotes;

/// Returns `true` when `line` is an Org comment line starting with `#`.
fn is_comment_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('#')
}

/// Tracks `:PROPERTIES:` ... `:END:` drawer blocks and returns `true` while inside a drawer.
fn step_property_drawer(line: &str, in_drawer: &mut bool) -> bool {
    let trimmed = line.trim();
    if *in_drawer {
        if trimmed.starts_with('*') || trimmed.eq_ignore_ascii_case(":END:") {
            *in_drawer = false;
        }
        return true;
    }
    if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
        *in_drawer = true;
        return true;
    }
    false
}

/// Advances past an inline code or verbatim span (`~...~`, `=...=`, or backticks) starting at `pos`.
fn skip_inline_verbatim(line: &str, pos: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let marker = *bytes.get(pos)?;
    if marker != b'~' && marker != b'=' && marker != b'`' {
        return None;
    }
    let rest = line.get(pos + 1..)?;
    let end_rel = rest.find(marker as char)?;
    Some(pos + 1 + end_rel + 1)
}

/// Extracts a footnote definition label from a line like `[fn:label] definition text`.
fn extract_footnote_def(line: &str) -> Option<(String, usize)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("[fn:") {
        return None;
    }
    let col = line.len() - trimmed.len();
    let rest = &trimmed[4..];
    let end = rest.find(']')?;
    let label = &rest[..end];
    if label.is_empty() || label.contains(':') {
        return None;
    }
    Some((label.to_string(), col))
}

/// Extracts all footnote reference labels from `line` with byte offsets, skipping inline verbatim spans.
fn scan_footnote_refs(line: &str, skip_leading_def: bool) -> Vec<(String, bool, usize)> {
    let mut refs = Vec::new();
    let mut pos = 0;

    if skip_leading_def {
        let trimmed = line.trim_start();
        if let Some(end) = trimmed.find(']') {
            pos = line.len() - trimmed.len() + end + 1;
        }
    }

    while pos < line.len() {
        if let Some(next_pos) = skip_inline_verbatim(line, pos) {
            pos = next_pos;
            continue;
        }

        if let Some(after) = line.get(pos..).and_then(|s| s.strip_prefix("[fn:")) {
            if let Some(end) = after.find(']') {
                let content = &after[..end];
                let (label, is_inline) = if let Some(colon_pos) = content.find(':') {
                    (&content[..colon_pos], true)
                } else {
                    (content, false)
                };
                if !label.is_empty() {
                    refs.push((label.to_string(), is_inline, pos));
                }
                pos += 4 + end + 1;
                continue;
            }
        }

        pos += line[pos..].chars().next().map_or(1, char::len_utf8);
    }

    refs
}

impl LintRule for OrphanedFootnotes {
    /// Returns the canonical warning rule identifier (`W004`).
    fn id(&self) -> &'static str {
        "W004"
    }

    /// Returns the kebab-case rule slug (`orphaned-footnotes`).
    fn name(&self) -> &'static str {
        "orphaned-footnotes"
    }

    /// Returns a one-line summary of the orphaned footnote check.
    fn description(&self) -> &'static str {
        "Detect footnotes defined but not referenced, or vice versa"
    }

    /// Scans source lines for mismatched definitions and references outside protected regions.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut definitions: HashMap<String, (usize, usize)> = HashMap::new();
        let mut references: HashMap<String, (usize, usize)> = HashMap::new();
        let mut inline_defs: HashSet<String> = HashSet::new();
        let mut diagnostics = Vec::new();
        let mut offset = 0;
        let mut in_drawer = false;
        let regions = protected_regions(&ctx.source.content);

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_protected(i, &regions)
                || step_property_drawer(raw, &mut in_drawer)
                || is_comment_line(raw)
            {
                offset += line.len() + 1;
                continue;
            }

            let def_opt = extract_footnote_def(raw);
            let is_def = if let Some((ref label, col)) = def_opt {
                let (line_num, _) = ctx.source.line_col(offset);
                definitions.insert(label.clone(), (line_num, col + 1));
                true
            } else {
                false
            };

            for (label, is_inline, rel_pos) in scan_footnote_refs(raw, is_def) {
                if is_inline {
                    inline_defs.insert(label.clone());
                }
                references.entry(label).or_insert_with(|| {
                    let (line_num, col) = ctx.source.line_col(offset + rel_pos);
                    (line_num, col)
                });
            }

            offset += line.len() + 1;
        }

        // Definitions without references.
        for (label, (line, col)) in &definitions {
            if !references.contains_key(label) {
                diagnostics.push(Diagnostic {
                    file: ctx.source.path.clone(),
                    line: *line,
                    column: *col,
                    severity: Severity::Warning,
                    rule_id: self.id(),
                    rule: self.name(),
                    message: format!("footnote [fn:{label}] is defined but never referenced"),
                    fix: None,
                });
            }
        }

        // References without definitions (excluding self-contained inline footnotes).
        for (label, (line, col)) in &references {
            if !definitions.contains_key(label) && !inline_defs.contains(label) {
                diagnostics.push(Diagnostic {
                    file: ctx.source.path.clone(),
                    line: *line,
                    column: *col,
                    severity: Severity::Warning,
                    rule_id: self.id(),
                    rule: self.name(),
                    message: format!("footnote [fn:{label}] is referenced but never defined"),
                    fix: None,
                });
            }
        }

        diagnostics.sort_by_key(|d| (d.line, d.column));
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
