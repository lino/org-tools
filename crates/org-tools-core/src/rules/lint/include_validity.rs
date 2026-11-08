// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Validates `#+INCLUDE:` directives for missing files, unresolved targets, workspace traversal, and cycles (`E003`).
//!
//! Spec:
//! - Org Mode Manual: [§12.4 Include Files](https://orgmode.org/manual/Include-Files.html).
//! - Org Syntax: [Keywords](https://orgmode.org/worg/org-syntax.html#Keywords).

use crate::diagnostic::{Diagnostic, Severity};
use crate::include::validate_file_includes;
use crate::rules::{LintContext, LintRule};

/// Detects broken, escaping, or cyclic `#+INCLUDE:` directives (`E003`).
///
/// Inspects every non-protected `#+INCLUDE:` line in a document and verifies
/// that the referenced file stays inside the workspace boundary, exists on
/// disk, resolves any `::*Heading`, `::#custom-id`, or `::id:<uuid>` subtree
/// target, and does not form a recursive include cycle.
///
/// Citations:
/// - Org Mode Manual: [§12.4 Include Files](https://orgmode.org/manual/Include-Files.html)
/// - Org Syntax: [Keywords](https://orgmode.org/worg/org-syntax.html#Keywords)
pub struct IncludeValidity;

impl LintRule for IncludeValidity {
    /// Returns the structural error rule identifier `"E003"`.
    fn id(&self) -> &'static str {
        "E003"
    }

    /// Returns the kebab-case rule name `"include-validity"`.
    fn name(&self) -> &'static str {
        "include-validity"
    }

    /// Returns a human-readable one-line summary of the rule.
    fn description(&self) -> &'static str {
        "Validate #+INCLUDE: targets, workspace confinement, and cycle freedom"
    }

    /// Scans non-protected `#+INCLUDE:` directives in `ctx.source` and emits `E003` diagnostics.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        validate_file_includes(ctx.source, None)
            .into_iter()
            .map(|(line, column, err)| Diagnostic {
                file: ctx.source.path.clone(),
                line,
                column,
                severity: Severity::Error,
                rule_id: self.id(),
                rule: self.name(),
                message: err.to_string(),
                fix: None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::include::{expand_includes, parse_include_directive, IncludeError};
    use crate::source::SourceFile;
    use std::fs;
    use tempfile::TempDir;

    /// Verifies synthetic buffers without `#+INCLUDE:` lines emit zero diagnostics.
    #[test]
    fn synthetic_buffer_without_includes_is_clean() {
        let source = SourceFile::new("test.org", "* Heading\nPlain body text.\n".to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        assert!(IncludeValidity.check(&ctx).is_empty());
    }

    /// Verifies `parse_include_directive` parses search targets, `:minlevel`, `:lines`, and `:only-contents`.
    #[test]
    fn parse_include_directive_options_and_ranges() {
        let dir = parse_include_directive(
            "#+INCLUDE: \"child.org::*Section\" :minlevel 2 :lines \"5-10\" :only-contents t",
        )
        .unwrap()
        .unwrap();
        assert_eq!(dir.raw_path, "child.org");
        assert_eq!(dir.search_option.as_deref(), Some("*Section"));
        assert_eq!(dir.minlevel, Some(2));
        assert_eq!(dir.lines, Some((Some(5), Some(10))));
        assert!(dir.only_contents);

        let open_start = parse_include_directive("#+include: \"a.org\" :lines \"-10\"")
            .unwrap()
            .unwrap();
        assert_eq!(open_start.lines, Some((None, Some(10))));

        let open_end = parse_include_directive("#+INCLUDE: \"a.org\" src rust :lines \"5-\"")
            .unwrap()
            .unwrap();
        assert_eq!(
            open_end.block_name,
            Some(("src".to_string(), Some("rust".to_string())))
        );
        assert_eq!(open_end.lines, Some((Some(5), None)));
    }

    /// Verifies `IncludeValidity` flags missing files, unresolved targets, path traversal, and cycles.
    #[test]
    fn include_validity_flags_missing_unresolved_traversal_and_cycle() {
        let outer = TempDir::new().unwrap();
        fs::write(outer.path().join("outside.org"), "* Outside\n").unwrap();
        let ws = outer.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("ok.org"), "* Present\nBody line.\n").unwrap();
        fs::write(ws.join("self.org"), "#+INCLUDE: \"self.org\"\n").unwrap();

        let main_path = ws.join("main.org");
        let main_content = concat!(
            "#+INCLUDE: \"ok.org::*Present\" :minlevel 2\n",
            "#+INCLUDE: \"missing.org\"\n",
            "#+INCLUDE: \"ok.org::*Absent\"\n",
            "#+INCLUDE: \"../outside.org\"\n",
            "#+INCLUDE: \"self.org\"\n",
            "#+begin_example\n",
            "#+INCLUDE: \"ignored.org\"\n",
            "#+end_example\n",
        );
        fs::write(&main_path, main_content).unwrap();
        let source = SourceFile::from_path(&main_path).unwrap();
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        let diags = IncludeValidity.check(&ctx);
        assert_eq!(diags.len(), 4);
        assert!(diags
            .iter()
            .all(|d| d.rule_id == "E003" && d.rule == "include-validity"));
        assert_eq!(diags[0].line, 2);
        assert_eq!(diags[1].line, 3);
        assert_eq!(diags[2].line, 4);
        assert_eq!(diags[3].line, 5);

        let err =
            expand_includes("#+INCLUDE: \"../outside.org\"\n", &main_path, Some(&ws)).unwrap_err();
        assert!(matches!(err, IncludeError::OutsideWorkspace(..)));
    }
}
