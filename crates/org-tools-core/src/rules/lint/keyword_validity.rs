// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Warns on unrecognized `#+KEYWORD` names.
//!
//! Spec: [§17.2 Export Settings](https://orgmode.org/manual/Export-Settings.html),
//! [Syntax: Keywords](https://orgmode.org/worg/org-syntax.html#Keywords)
//!
//! Info severity -- custom keywords are valid in some contexts.
//! Includes known package prefixes (HUGO_, REVEAL_, PANDOC_, etc.) to avoid
//! false positives.

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Flags `#+KEYWORD:` lines whose keyword name is not in the standard set
/// or recognized by a known package prefix.
///
/// Matches against [`KNOWN_KEYWORDS`] (exact) and [`KNOWN_PREFIXES`]
/// (prefix). Reports at [`Severity::Info`] since custom keywords are
/// valid in some org-mode configurations. Skips content inside protected
/// regions and block delimiters.
///
/// Spec: [§17.2 Export Settings](https://orgmode.org/manual/Export-Settings.html),
/// [Syntax: Keywords](https://orgmode.org/worg/org-syntax.html#Keywords)
pub struct KeywordValidity;

/// Standard org-mode in-buffer and export keyword names recognized without warnings.
const KNOWN_KEYWORDS: &[&str] = &[
    "ARCHIVE",
    "ATTR_",
    "AUTHOR",
    "BIBLIOGRAPHY",
    "BIND",
    "CALL",
    "CAPTION",
    "CATEGORY",
    "CITE_EXPORT",
    "COLUMNS",
    "CONSTANTS",
    "CREATOR",
    "DATE",
    "DESCRIPTION",
    "EMAIL",
    "EXCLUDE_TAGS",
    "EXPORT_FILE_NAME",
    "FILETAGS",
    "HEADER",
    "HTML_HEAD",
    "HTML_HEAD_EXTRA",
    "INCLUDE",
    "KEYWORDS",
    "LANGUAGE",
    "LATEX_CLASS",
    "LATEX_CLASS_OPTIONS",
    "LATEX_COMPILER",
    "LATEX_HEADER",
    "LATEX_HEADER_EXTRA",
    "LINK",
    "MACRO",
    "NAME",
    "OPTIONS",
    "PLOT",
    "PRINT_BIBLIOGRAPHY",
    "PRIORITIES",
    "PROPERTY",
    "RESULTS",
    "SELECT_TAGS",
    "SEQ_TODO",
    "SETUPFILE",
    "STARTUP",
    "SUBTITLE",
    "TAGS",
    "TBLFM",
    "TITLE",
    "TODO",
    "TYP_TODO",
];

/// Known keyword prefixes from popular packages.
const KNOWN_PREFIXES: &[&str] = &[
    "HUGO_", "REVEAL_", "PANDOC_", "BEAMER_", "EXPORT_", "HTML_", "LATEX_", "ODT_", "TEXINFO_",
    "ATTR_",
];

impl LintRule for KeywordValidity {
    /// Returns the diagnostic rule identifier (`I003`).
    fn id(&self) -> &'static str {
        "I003"
    }

    /// Returns the kebab-case org-lint rule name.
    fn name(&self) -> &'static str {
        "keyword-validity"
    }

    /// Returns a human-readable summary of the lint rule.
    fn description(&self) -> &'static str {
        "Warn on unrecognized #+KEYWORD names"
    }

    /// Scans unprotected `#+KEYWORD:` lines in `ctx` for unrecognized keyword names.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);
        let mut offset = 0;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            if !is_protected(i, &regions) {
                if let Some(keyword) = extract_unrecognized_keyword(raw) {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Info,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!("unrecognized keyword #+{}", keyword),
                        fix: None,
                    });
                }
            }
            offset += line.len() + 1;
        }

        diagnostics
    }
}

/// Returns `true` when the text after `#+` is empty, whitespace-prefixed, or a `BEGIN`/`END` block line.
fn is_block_delimiter_or_blank(rest: &str) -> bool {
    let rest_upper = rest.to_uppercase();
    rest.is_empty()
        || rest.starts_with(' ')
        || rest_upper.starts_with("BEGIN")
        || rest_upper.starts_with("END")
}

/// Extracts an uppercase keyword name from `raw` when it is an unrecognized `#+KEYWORD:` line.
fn extract_unrecognized_keyword(raw: &str) -> Option<String> {
    let rest = raw.trim_start().strip_prefix("#+")?;
    if is_block_delimiter_or_blank(rest) {
        return None;
    }
    let colon_pos = rest.find(':')?;
    let keyword = rest[..colon_pos].to_uppercase();
    (!keyword.is_empty() && !is_known_keyword(&keyword)).then_some(keyword)
}

/// Returns `true` if the keyword matches a known name or a known package prefix.
fn is_known_keyword(keyword: &str) -> bool {
    // Check exact matches.
    if KNOWN_KEYWORDS.contains(&keyword) {
        return true;
    }
    // Check prefix matches (e.g., HUGO_*, ATTR_LATEX).
    for prefix in KNOWN_PREFIXES {
        if keyword.starts_with(prefix) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    /// Runs `KeywordValidity` against `input` using default configuration.
    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        KeywordValidity.check(&ctx)
    }

    /// Verifies that standard built-in keywords like `#+TITLE:` pass without diagnostics.
    #[test]
    fn known_keyword() {
        assert!(check_it("#+TITLE: test\n").is_empty());
    }

    /// Verifies that known package prefixes like `#+HUGO_SECTION:` pass without diagnostics.
    #[test]
    fn known_package_prefix() {
        assert!(check_it("#+HUGO_SECTION: posts\n").is_empty());
    }

    /// Verifies that an unknown keyword emits an `Info`-severity `I003` diagnostic.
    #[test]
    fn unknown_keyword() {
        let diags = check_it("#+FOOBAR: value\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Info);
    }

    /// Verifies that `#+BEGIN_SRC` and `#+END_SRC` block delimiters are not flagged.
    #[test]
    fn block_not_flagged() {
        assert!(check_it("#+BEGIN_SRC python\ncode\n#+END_SRC\n").is_empty());
    }

    /// Verifies that `#+ATTR_*` affiliated keywords pass without diagnostics.
    #[test]
    fn attr_prefix() {
        assert!(check_it("#+ATTR_LATEX: :width 0.5\n").is_empty());
    }

    /// Verifies that unknown `#+KEYWORD:` lines inside `#+BEGIN_SRC` blocks are ignored.
    #[test]
    fn in_code_block() {
        let input = "#+BEGIN_SRC org\n#+FOOBAR: value\n#+END_SRC\n";
        assert!(check_it(input).is_empty());
    }

    /// Verifies that `KNOWN_KEYWORDS` has no duplicate entries and still accepts `#+CREATOR:`.
    #[test]
    fn known_keywords_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for &kw in KNOWN_KEYWORDS {
            assert!(seen.insert(kw), "duplicate keyword in KNOWN_KEYWORDS: {kw}");
        }
        assert!(check_it("#+CREATOR: Emacs 29.4\n").is_empty());
    }
}
