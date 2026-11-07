// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Warns when an `[[id:...]]` link target does not match any file or heading `:ID:` (`W037`).
//!
//! Spec:
//! - Org Mode Manual: <https://orgmode.org/manual/Internal-Links.html>
//! - Org Syntax: <https://orgmode.org/worg/org-syntax.html#Links>
//! - org-roam v2 Manual: <https://www.orgroam.com/manual.html>

use crate::diagnostic::{Diagnostic, Severity};
use crate::document::OrgDocument;
use crate::roam::{doc_contains_id_ci, scan_id_links};
use crate::rules::{LintContext, LintRule};

/// Warns when `[[id:<target>]]` outside protected regions and verbatim spans cannot be resolved (`W037`).
///
/// Resolves `<target>` (with any `::search` suffix stripped) case-insensitively
/// against both level-0 file `:ID:` drawers and heading `:ID:` drawers across
/// the current document and [`LintContext::workspace_docs`], regardless of
/// `:ROAM_EXCLUDE:` settings.
///
/// Citations:
/// - Org Mode Manual: [§4.2 Internal Links](https://orgmode.org/manual/Internal-Links.html),
///   [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
/// - Org Syntax: [Links](https://orgmode.org/worg/org-syntax.html#Links),
///   [Code and Verbatim](https://orgmode.org/worg/org-syntax.html#Code_and_Verbatim)
/// - Ecosystem Spec: [org-roam v2](https://www.orgroam.com/manual.html)
pub struct BrokenIdLink;

impl LintRule for BrokenIdLink {
    /// Returns the canonical warning rule identifier `"W037"`.
    fn id(&self) -> &'static str {
        "W037"
    }

    /// Returns the kebab-case rule slug `"broken-id-link"`.
    fn name(&self) -> &'static str {
        "broken-id-link"
    }

    /// Returns a one-line summary of the broken `[[id:...]]` link check.
    fn description(&self) -> &'static str {
        "Warn when [[id:...]] link targets an unresolved :ID:"
    }

    /// Scans unprotected Org prose for `[[id:...]]` links whose target `:ID:` is absent in scope.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let current_doc = OrgDocument::from_source(ctx.source);
        let mut diagnostics = Vec::new();

        for link in scan_id_links(ctx.source) {
            if !id_exists_in_scope(&link.target_id, &current_doc, ctx.workspace_docs) {
                diagnostics.push(Diagnostic {
                    file: ctx.source.path.clone(),
                    line: link.line,
                    column: link.column,
                    severity: Severity::Warning,
                    rule_id: self.id(),
                    rule: self.name(),
                    message: format!("unresolved [[id:{}]] link target", link.target_id),
                    fix: None,
                });
            }
        }

        diagnostics
    }
}

/// Returns `true` when `target_id` matches any `:ID:` in `current_doc` or `workspace_docs`.
fn id_exists_in_scope(
    target_id: &str,
    current_doc: &OrgDocument,
    workspace_docs: Option<&[OrgDocument]>,
) -> bool {
    if doc_contains_id_ci(current_doc, target_id) {
        return true;
    }
    workspace_docs.is_some_and(|ws| ws.iter().any(|doc| doc_contains_id_ci(doc, target_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    /// Runs `BrokenIdLink` on a single source buffer without external workspace docs.
    fn check_single(content: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("links.org", content.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        BrokenIdLink.check(&ctx)
    }

    /// Verifies resolved intra-file, cross-file, `::search`, and `:ROAM_EXCLUDE: t` IDs produce zero warnings.
    #[test]
    fn resolved_intra_and_cross_file_links_including_roam_excluded_pass() {
        let target = SourceFile::new(
            "target.org",
            ":PROPERTIES:\n:ID: ext-roam\n:ROAM_EXCLUDE: t\n:END:\n#+TITLE: Excluded\n".to_string(),
        );
        let main = SourceFile::new(
            "main.org",
            concat!(
                "* Target\n:PROPERTIES:\n:ID: local-1\n:END:\n",
                "See [[id:LOCAL-1::*Sub][Local]] and [[id:ext-roam][External]].\n",
            )
            .to_string(),
        );
        let ws = vec![
            OrgDocument::from_source(&target),
            OrgDocument::from_source(&main),
        ];
        let cfg = Config::default();
        let ctx = LintContext::with_workspace(&main, &cfg, &ws);
        assert!(BrokenIdLink.check(&ctx).is_empty());
    }

    /// Verifies unresolved `[[id:...]]` links emit `W037` while skipping inline `=...=`/`~...~` and code blocks.
    #[test]
    fn broken_link_warns_while_skipping_verbatim_comments_drawers_and_blocks() {
        let content = concat!(
            ":PROPERTIES:\n:NOTE: [[id:in-drawer]]\n:END:\n",
            "# [[id:in-comment]]\n",
            "Literal =[[id:<uuid>]]= and ~[[id:code-id]]~ and *Standard =[[id:doc][Desc]]= Links*.\n",
            "#+begin_src org\n[[id:in-src]]\n#+end_src\n",
            "Broken reference [[id:missing-target][Missing]] here.\n",
        );
        let diags = check_single(content);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id, "W037");
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].line, 9);
        assert!(diags[0].message.contains("missing-target"));
    }
}
