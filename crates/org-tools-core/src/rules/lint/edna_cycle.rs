// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects circular `:BLOCKER:` dependency chains across single or multiple Org files (`E008`).
//!
//! Spec:
//! - Org Mode Manual: <https://orgmode.org/manual/Property-Syntax.html>
//! - Org Syntax: <https://orgmode.org/worg/org-syntax.html#Property_Drawers>
//! - org-edna Manual: <https://www.nongnu.org/org-edna-el/>

use crate::diagnostic::{Diagnostic, Severity};
use crate::document::{OrgDocument, OrgEntry};
use crate::edna::find_entry_blocker_cycle;
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Detects circular org-edna `:BLOCKER:` dependency cycles (`E008`).
///
/// Inspects every heading entry carrying a `:BLOCKER:` property outside
/// protected literal blocks and reports self-loops, 2-node cycles, and N-node
/// cross-file dependency cycles across [`LintContext::workspace_docs`].
///
/// Citations:
/// - Org Mode Manual: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
/// - Org Syntax: [Property Drawers](https://orgmode.org/worg/org-syntax.html#Property_Drawers)
/// - Ecosystem Spec: [org-edna](https://www.nongnu.org/org-edna-el/)
pub struct EdnaCycle;

impl LintRule for EdnaCycle {
    /// Returns the canonical structural error rule identifier `"E008"`.
    fn id(&self) -> &'static str {
        "E008"
    }

    /// Returns the kebab-case rule slug `"circular-edna-blocker"`.
    fn name(&self) -> &'static str {
        "circular-edna-blocker"
    }

    /// Returns a human-readable summary of the circular `:BLOCKER:` check.
    fn description(&self) -> &'static str {
        "Detect circular org-edna :BLOCKER: task dependencies"
    }

    /// Scans `:BLOCKER:` properties in `ctx.source` for dependency cycles.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let current_doc = OrgDocument::from_source(ctx.source);
        let doc_refs = assemble_workspace_refs(&current_doc, ctx.workspace_docs);
        let regions = protected_regions(&ctx.source.content);
        let lines: Vec<&str> = ctx.source.content.split('\n').collect();
        let mut diagnostics = Vec::new();

        for (entry_idx, entry) in current_doc.entries.iter().enumerate() {
            if current_doc.property(entry_idx, "BLOCKER").is_none() {
                continue;
            }
            if let Some(cycle_labels) = find_entry_blocker_cycle(&doc_refs, &current_doc, entry_idx)
            {
                let (line_num, col) = find_blocker_line_col(&lines, &regions, entry);
                diagnostics.push(Diagnostic {
                    file: ctx.source.path.clone(),
                    line: line_num,
                    column: col,
                    severity: Severity::Error,
                    rule_id: self.id(),
                    rule: self.name(),
                    message: format!(
                        "circular :BLOCKER: dependency cycle: {}",
                        cycle_labels.join(" -> ")
                    ),
                    fix: None,
                });
            }
        }

        diagnostics
    }
}

/// Builds the list of [`OrgDocument`] references substituting `current_doc` for its own path.
fn assemble_workspace_refs<'a>(
    current_doc: &'a OrgDocument,
    workspace_docs: Option<&'a [OrgDocument]>,
) -> Vec<&'a OrgDocument> {
    let Some(ws) = workspace_docs else {
        return vec![current_doc];
    };
    let mut refs = Vec::with_capacity(ws.len() + 1);
    let mut replaced = false;
    for doc in ws {
        if doc.file == current_doc.file {
            refs.push(current_doc);
            replaced = true;
        } else {
            refs.push(doc);
        }
    }
    if !replaced {
        refs.push(current_doc);
    }
    refs
}

/// Finds the 1-based `(line, column)` of `:BLOCKER:` inside `entry`'s drawer.
fn find_blocker_line_col(
    lines: &[&str],
    protected: &[(usize, usize)],
    entry: &OrgEntry,
) -> (usize, usize) {
    let start = entry.heading_line;
    let end = (entry.content_end_line - 1).min(lines.len());
    for (idx, &raw_line) in lines.iter().enumerate().take(end).skip(start) {
        if is_protected(idx, protected) {
            continue;
        }
        let trimmed = raw_line.trim_start();
        if trimmed.eq_ignore_ascii_case(":END:") {
            break;
        }
        if trimmed
            .get(..9)
            .is_some_and(|p| p.eq_ignore_ascii_case(":BLOCKER:"))
            || trimmed
                .get(..10)
                .is_some_and(|p| p.eq_ignore_ascii_case(":BLOCKER+:"))
        {
            let col = raw_line.len() - trimmed.len() + 1;
            return (idx + 1, col);
        }
    }
    (entry.heading_line, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    /// Runs `EdnaCycle` on a single source string.
    fn check_single(content: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("cycle.org", content.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        EdnaCycle.check(&ctx)
    }

    /// Verifies acyclic `:BLOCKER:` chains emit zero diagnostics.
    #[test]
    fn acyclic_chain_passes() {
        let content = concat!(
            "* TODO Step 1\n:PROPERTIES:\n:ID: s1\n:END:\n",
            "* TODO Step 2\n:PROPERTIES:\n:ID: s2\n:BLOCKER: ids(\"s1\")\n:END:\n",
        );
        assert!(check_single(content).is_empty());
    }

    /// Verifies self-referencing `:BLOCKER:` emits `E008`.
    #[test]
    fn detects_self_loop() {
        let content = "* TODO Self\n:PROPERTIES:\n:ID: self-1\n:BLOCKER: ids(\"self-1\")\n:END:\n";
        let diags = check_single(content);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule_id, "E008");
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(diags[0].line, 4);
    }

    /// Verifies cross-file cycles are detected when `workspace_docs` is provided.
    #[test]
    fn detects_cross_file_cycle() {
        let s1 = SourceFile::new(
            "a.org",
            "* TODO A\n:PROPERTIES:\n:ID: a\n:BLOCKER: ids(\"b\")\n:END:\n".to_string(),
        );
        let s2 = SourceFile::new(
            "b.org",
            "* TODO B\n:PROPERTIES:\n:ID: b\n:BLOCKER: ids(\"a\")\n:END:\n".to_string(),
        );
        let ws = vec![OrgDocument::from_source(&s1), OrgDocument::from_source(&s2)];
        let cfg = Config::default();
        let ctx = LintContext::with_workspace(&s1, &cfg, &ws);
        let diags = EdnaCycle.check(&ctx);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("a -> b -> a"));
    }

    /// Verifies `:BLOCKER:` cycles inside `#+begin_src` blocks are ignored.
    #[test]
    fn ignores_protected_src_blocks() {
        let content =
            "#+begin_src org\n* TODO Fake\n:PROPERTIES:\n:ID: f\n:BLOCKER: ids(\"f\")\n:END:\n#+end_src\n";
        assert!(check_single(content).is_empty());
    }

    /// Verifies case-insensitive `:blocker:` drawers are checked for cycles.
    #[test]
    fn detects_lowercase_blocker_cycle() {
        let content = "* TODO Lower\n:PROPERTIES:\n:ID: lc-1\n:blocker: ids(\"lc-1\")\n:END:\n";
        let diags = check_single(content);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].line, 4);
    }
}
