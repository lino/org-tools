// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Warns when `ids("...")` in `:BLOCKER:` or `:TRIGGER:` references an unknown `:ID:` (`W036`).
//!
//! Spec:
//! - Org Mode Manual: <https://orgmode.org/manual/Property-Syntax.html>
//! - Org Syntax: <https://orgmode.org/worg/org-syntax.html#Property_Drawers>
//! - org-edna Manual: <https://www.nongnu.org/org-edna-el/>

use crate::diagnostic::{Diagnostic, Severity};
use crate::document::OrgDocument;
use crate::edna::extract_dependency_ids;
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Warns when `ids(...)` in `:BLOCKER:` or `:TRIGGER:` references an unresolved `:ID:` (`W036`).
///
/// Resolves referenced IDs against both heading `:ID:` properties and level-0
/// `org-roam` v2 file `:ID:` drawers across [`LintContext::workspace_docs`] (or
/// the current [`OrgDocument`] when linting a single file). Only syntactically
/// parsed `ids(...)` calls outside protected literal blocks are checked.
///
/// Citations:
/// - Org Mode Manual: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
/// - Org Syntax: [Property Drawers](https://orgmode.org/worg/org-syntax.html#Property_Drawers)
/// - Ecosystem Spec: [org-edna](https://www.nongnu.org/org-edna-el/)
pub struct UnresolvedEdnaIds;

impl LintRule for UnresolvedEdnaIds {
    /// Returns the canonical warning rule identifier `"W036"`.
    fn id(&self) -> &'static str {
        "W036"
    }

    /// Returns the kebab-case rule slug `"unresolved-edna-ids"`.
    fn name(&self) -> &'static str {
        "unresolved-edna-ids"
    }

    /// Returns a one-line summary of the unresolved `ids(...)` check.
    fn description(&self) -> &'static str {
        "Warn when org-edna ids() references an unresolved :ID:"
    }

    /// Scans `:BLOCKER:` and `:TRIGGER:` lines for unresolved `ids(...)` targets.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let current_doc = OrgDocument::from_source(ctx.source);
        let regions = protected_regions(&ctx.source.content);
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for (line_idx, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            if !is_protected(line_idx, &regions) {
                if let Some((prop_name, prop_val)) = extract_edna_property(raw.trim()) {
                    for raw_id in extract_dependency_ids(prop_val) {
                        let clean_id = raw_id.strip_prefix("id:").unwrap_or(raw_id.as_str());
                        if !id_exists_in_scope(clean_id, &current_doc, ctx.workspace_docs) {
                            let (line_num, col) = ctx.source.line_col(offset);
                            diagnostics.push(Diagnostic {
                                file: ctx.source.path.clone(),
                                line: line_num,
                                column: col,
                                severity: Severity::Warning,
                                rule_id: self.id(),
                                rule: self.name(),
                                message: format!(
                                    ":{prop_name}: unresolved ids(\"{raw_id}\") target"
                                ),
                                fix: None,
                            });
                        }
                    }
                }
            }
            offset += line.len() + 1;
        }

        diagnostics
    }
}

/// Extracts `("BLOCKER" | "TRIGGER", value)` when `trimmed` is an org-edna property or continuation line.
fn extract_edna_property(trimmed: &str) -> Option<(&'static str, &str)> {
    let rest = trimmed.strip_prefix(':')?;
    let colon_pos = rest.find(':')?;
    let key = &rest[..colon_pos];
    let val = rest[colon_pos + 1..].trim();
    if val.is_empty() {
        return None;
    }
    if key.eq_ignore_ascii_case("BLOCKER") || key.eq_ignore_ascii_case("BLOCKER+") {
        Some(("BLOCKER", val))
    } else if key.eq_ignore_ascii_case("TRIGGER") || key.eq_ignore_ascii_case("TRIGGER+") {
        Some(("TRIGGER", val))
    } else {
        None
    }
}

/// Checks whether `id` matches any heading or level-0 file `:ID:` in `current_doc` or `workspace_docs`.
fn id_exists_in_scope(
    id: &str,
    current_doc: &OrgDocument,
    workspace_docs: Option<&[OrgDocument]>,
) -> bool {
    if current_doc.contains_id(id) {
        return true;
    }
    if let Some(ws) = workspace_docs {
        return ws.iter().any(|doc| doc.contains_id(id));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    /// Runs `UnresolvedEdnaIds` on a single source buffer.
    fn check_single(content: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("refs.org", content.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        UnresolvedEdnaIds.check(&ctx)
    }

    /// Verifies intra-file `:ID:` references produce zero warnings.
    #[test]
    fn valid_same_file_id_passes() {
        let content = concat!(
            "* TODO First\n:PROPERTIES:\n:ID: ok-1\n:END:\n",
            "* TODO Second\n:PROPERTIES:\n:BLOCKER: ids(\"ok-1\")\n:END:\n",
        );
        assert!(check_single(content).is_empty());
    }

    /// Verifies missing `ids(...)` targets emit `W036`.
    #[test]
    fn unresolved_id_emits_warning() {
        let content =
            "* TODO Lonely\n:PROPERTIES:\n:TRIGGER: ids(\"missing-uuid\") todo!(\"NEXT\")\n:END:\n";
        let diags = check_single(content);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id, "W036");
        assert_eq!(diags[0].severity, Severity::Warning);
        assert!(diags[0].message.contains("missing-uuid"));
    }

    /// Verifies cross-file heading and `org-roam` level-0 `:ID:`s resolve cleanly.
    #[test]
    fn workspace_and_roam_file_ids_resolve() {
        let roam = SourceFile::new(
            "roam.org",
            ":PROPERTIES:\n:ID: roam-1\n:END:\n#+TITLE: Roam\n".to_string(),
        );
        let main = SourceFile::new(
            "main.org",
            "* TODO Ref\n:PROPERTIES:\n:BLOCKER: ids(\"roam-1\")\n:END:\n".to_string(),
        );
        let ws = vec![
            OrgDocument::from_source(&roam),
            OrgDocument::from_source(&main),
        ];
        let cfg = Config::default();
        let ctx = LintContext::with_workspace(&main, &cfg, &ws);
        assert!(UnresolvedEdnaIds.check(&ctx).is_empty());
    }

    /// Verifies protected literal blocks and malformed `ids(` calls do not emit `W036`.
    #[test]
    fn skips_protected_blocks_and_malformed_syntax() {
        let content = concat!(
            "#+begin_src org\n:BLOCKER: ids(\"ghost\")\n#+end_src\n",
            "* TODO Bad Syntax\n:PROPERTIES:\n:BLOCKER: ids(\"unclosed\n:END:\n",
        );
        assert!(check_single(content).is_empty());
    }

    /// Verifies `:BLOCKER+:` and `:TRIGGER+:` continuation lines are linted for unresolved IDs.
    #[test]
    fn continuation_lines_and_case_insensitive_keys_checked() {
        let content = concat!(
            "* TODO Valid\n:PROPERTIES:\n:ID: valid-1\n:END:\n",
            "* TODO Cont\n:PROPERTIES:\n",
            ":BLOCKER:  ids(\"valid-1\")\n",
            ":BLOCKER+: ids(\"missing-b\")\n",
            ":trigger+: ids(\"missing-t\") todo!(\"NEXT\")\n",
            ":END:\n",
        );
        let diags = check_single(content);
        assert_eq!(diags.len(), 2);
        assert!(diags[0].message.contains("missing-b"));
        assert!(diags[1].message.contains("missing-t"));
    }
}
