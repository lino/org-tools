// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects duplicate `:ID:` property values within a file or across workspace files (`E009`).
//!
//! Spec:
//! - Org Mode Manual: <https://orgmode.org/manual/Property-Syntax.html>
//! - Org Syntax: <https://orgmode.org/worg/org-syntax.html#Property_Drawers>
//! - org-roam v2 Manual: <https://www.orgroam.com/manual.html>

use std::collections::HashMap;
use std::path::Path;

use crate::diagnostic::{Diagnostic, Severity};
use crate::document::OrgDocument;
use crate::export_md::normalize_workspace_path;
use crate::roam::doc_contains_id_ci;
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::heading::parse_heading;
use crate::rules::{LintContext, LintRule};

/// Detects duplicate `:ID:` values across level-0 file drawers, headings, and workspace files (`E009`).
///
/// Citations:
/// - Org Mode Manual: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html),
///   [§4.2 Internal Links](https://orgmode.org/manual/Internal-Links.html)
/// - Org Syntax: [Property Drawers](https://orgmode.org/worg/org-syntax.html#Property_Drawers)
/// - Ecosystem Spec: [org-roam v2](https://www.orgroam.com/manual.html)
pub struct DuplicateId;

/// An `:ID:` declaration located inside a `:PROPERTIES:` drawer of the linted file.
struct LocatedId {
    /// Trimmed `:ID:` property value.
    id: String,
    /// One-based line number of the `:ID:` line.
    line: usize,
    /// One-based column number of `:ID:`.
    column: usize,
}

impl LintRule for DuplicateId {
    /// Returns the canonical structural error rule identifier `"E009"`.
    fn id(&self) -> &'static str {
        "E009"
    }

    /// Returns the kebab-case rule slug `"duplicate-id"`.
    fn name(&self) -> &'static str {
        "duplicate-id"
    }

    /// Returns a one-line summary of the duplicate `:ID:` check.
    fn description(&self) -> &'static str {
        "Detect duplicate :ID: values within or across workspace files"
    }

    /// Scans level-0 file and heading `:PROPERTIES:` drawers for colliding `:ID:` values.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let located = collect_drawer_ids(ctx);
        let mut seen_in_file: HashMap<String, usize> = HashMap::new();
        let mut diagnostics = Vec::new();

        for item in located {
            let norm = item.id.to_ascii_lowercase();
            if let Some(&first_line) = seen_in_file.get(&norm) {
                diagnostics.push(build_error(
                    ctx,
                    &item,
                    format!(
                        "duplicate :ID: \"{}\" (first defined at line {first_line})",
                        item.id
                    ),
                ));
                continue;
            }
            seen_in_file.insert(norm, item.line);

            if let Some(other_file) =
                find_cross_file_collision(&ctx.source.path, &item.id, ctx.workspace_docs)
            {
                diagnostics.push(build_error(
                    ctx,
                    &item,
                    format!(
                        "duplicate :ID: \"{}\" (also defined in {})",
                        item.id,
                        other_file.display()
                    ),
                ));
            }
        }

        diagnostics
    }
}

/// Constructs an `E009` [`Diagnostic`] at `item`'s line and column.
fn build_error(ctx: &LintContext, item: &LocatedId, message: String) -> Diagnostic {
    Diagnostic {
        file: ctx.source.path.clone(),
        line: item.line,
        column: item.column,
        severity: Severity::Error,
        rule_id: "E009",
        rule: "duplicate-id",
        message,
        fix: None,
    }
}

/// Returns `true` when `left` and `right` refer to the same underlying document path.
fn is_same_document_path(left: &Path, right: &Path) -> bool {
    left == right || normalize_workspace_path(left) == normalize_workspace_path(right)
}

/// Finds another document in `workspace_docs` that also defines `id` case-insensitively.
fn find_cross_file_collision<'a>(
    current_path: &Path,
    id: &str,
    workspace_docs: Option<&'a [OrgDocument]>,
) -> Option<&'a Path> {
    let ws = workspace_docs?;
    ws.iter()
        .find(|doc| !is_same_document_path(current_path, &doc.file) && doc_contains_id_ci(doc, id))
        .map(|doc| doc.file.as_path())
}

/// Extracts `:ID:` value from a trimmed property line when the key is `"ID"` (case-insensitive).
fn parse_id_property_value(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix(':')?;
    let colon_pos = rest.find(':')?;
    let key = &rest[..colon_pos];
    if !key.eq_ignore_ascii_case("ID") {
        return None;
    }
    let val = rest[colon_pos + 1..].trim();
    (!val.is_empty()).then_some(val)
}

/// Collects all `:ID:` property declarations inside unprotected `:PROPERTIES:` drawers.
fn collect_drawer_ids(ctx: &LintContext) -> Vec<LocatedId> {
    let protected = protected_regions(&ctx.source.content);
    let mut in_drawer = false;
    let mut offset = 0;
    let mut out = Vec::new();

    for (line_idx, line) in ctx.source.content.split('\n').enumerate() {
        let raw = line.strip_suffix('\r').unwrap_or(line);
        if !is_protected(line_idx, &protected) {
            let trimmed = raw.trim();
            if in_drawer && (trimmed.eq_ignore_ascii_case(":END:") || parse_heading(raw).is_some())
            {
                in_drawer = false;
            } else if !in_drawer && trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
                in_drawer = true;
            } else if in_drawer {
                if let Some(val) = parse_id_property_value(trimmed) {
                    let lead = raw.len() - raw.trim_start().len();
                    let (line_num, col) = ctx.source.line_col(offset + lead);
                    out.push(LocatedId {
                        id: val.to_string(),
                        line: line_num,
                        column: col,
                    });
                }
            }
        }
        offset += line.len() + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    /// Runs `DuplicateId` against a single in-memory source buffer.
    fn check_single(content: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("single.org", content.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        DuplicateId.check(&ctx)
    }

    /// Verifies unique file and heading `:ID:` values emit zero diagnostics.
    #[test]
    fn unique_ids_pass() {
        let content = concat!(
            ":PROPERTIES:\n:ID: file-1\n:END:\n#+TITLE: Clean\n",
            "* One\n:PROPERTIES:\n:ID: head-1\n:END:\n",
            "* Two\n:PROPERTIES:\n:ID: head-2\n:END:\n",
        );
        assert!(check_single(content).is_empty());
    }

    /// Verifies case-insensitive intra-file duplicate `:ID:`s report `E009` on duplicate lines.
    #[test]
    fn intra_file_duplicate_case_insensitive_reports_error() {
        let content = concat!(
            ":PROPERTIES:\n:ID: Abc-123\n:END:\n",
            "* Heading\n:PROPERTIES:\n:id: abc-123\n:END:\n",
        );
        let diags = check_single(content);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id, "E009");
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(diags[0].line, 6);
    }

    /// Verifies cross-file `:ID:` collisions emit `E009` without self-colliding on the same file.
    #[test]
    fn cross_file_collision_reports_error_without_self_collision() {
        let s1 = SourceFile::new(
            "./a.org",
            ":PROPERTIES:\n:ID: shared-uuid\n:END:\n#+TITLE: A\n".to_string(),
        );
        let s2 = SourceFile::new(
            "b.org",
            "* Entry\n:PROPERTIES:\n:ID: SHARED-UUID\n:END:\n".to_string(),
        );
        let s1_norm = SourceFile::new("a.org", s1.content.clone());
        let ws_same_only = vec![OrgDocument::from_source(&s1_norm)];
        let cfg = Config::default();

        let self_ctx = LintContext::with_workspace(&s1, &cfg, &ws_same_only);
        assert!(DuplicateId.check(&self_ctx).is_empty());

        let ws_both = vec![
            OrgDocument::from_source(&s1_norm),
            OrgDocument::from_source(&s2),
        ];
        let cross_ctx = LintContext::with_workspace(&s1, &cfg, &ws_both);
        let diags = DuplicateId.check(&cross_ctx);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id, "E009");
        assert_eq!(diags[0].line, 2);
    }

    /// Verifies `:ID:` lines inside protected literal blocks are ignored.
    #[test]
    fn ignores_protected_blocks() {
        let content = concat!(
            "* Real\n:PROPERTIES:\n:ID: dup-1\n:END:\n",
            "#+begin_example\n:PROPERTIES:\n:ID: dup-1\n:END:\n#+end_example\n",
        );
        assert!(check_single(content).is_empty());
    }
}
