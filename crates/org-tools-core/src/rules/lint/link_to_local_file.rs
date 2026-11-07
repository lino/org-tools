// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects `file:` links pointing to non-existent local files.
//!
//! Spec: [§4.4 Handling Links](https://orgmode.org/manual/Handling-Links.html)
//! org-lint: `link-to-local-file`

use std::path::Path;

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

/// Prefix opening an Org bracket link targeting a local file.
const FILE_LINK_PREFIX: &str = "[[file:";

/// Delimiter separating the link target from its optional description.
const LINK_DESC_DELIMITER: &str = "][";

/// Closing bracket pair terminating an Org bracket link.
const LINK_CLOSE_DELIMITER: &str = "]]";

/// Delimiter introducing an internal Org link search target (`::*Heading` or `::42`).
const LINK_SEARCH_DELIMITER: &str = "::";

/// Checks that `file:` links in bracket link syntax point to files that exist
/// on the local filesystem.
///
/// Resolves paths relative to the directory containing the source file. Strips
/// search strings (`::search`) before checking existence. Skips links inside
/// protected regions (code blocks, example blocks, etc.).
///
/// Spec: [§4.4 Handling Links](https://orgmode.org/manual/Handling-Links.html)
/// org-lint: `link-to-local-file`
pub struct LinkToLocalFile;

/// Extracts the next `(raw_target, remaining_line)` from `search`.
fn next_file_link(search: &str) -> Option<(&str, &str)> {
    let pos = search.find(FILE_LINK_PREFIX)?;
    let rest = &search[pos + FILE_LINK_PREFIX.len()..];
    match (
        rest.find(LINK_DESC_DELIMITER),
        rest.find(LINK_CLOSE_DELIMITER),
    ) {
        (Some(desc_pos), Some(close_pos)) if desc_pos < close_pos => {
            let after_desc = &rest[desc_pos + LINK_DESC_DELIMITER.len()..];
            let next = match after_desc.find(LINK_CLOSE_DELIMITER) {
                Some(end_pos) => &after_desc[end_pos + LINK_CLOSE_DELIMITER.len()..],
                None => "",
            };
            Some((&rest[..desc_pos], next))
        }
        (_, Some(close_pos)) => {
            let next = &rest[close_pos + LINK_CLOSE_DELIMITER.len()..];
            Some((&rest[..close_pos], next))
        }
        (Some(desc_pos), None) => Some((&rest[..desc_pos], "")),
        (None, None) => Some((rest, "")),
    }
}

/// Strips any `::search` suffix from a `file:` link target path.
fn strip_search_suffix(raw_target: &str) -> &str {
    match raw_target.find(LINK_SEARCH_DELIMITER) {
        Some(pos) => &raw_target[..pos],
        None => raw_target,
    }
}

impl LintRule for LinkToLocalFile {
    /// Returns the canonical lint rule identifier (`W023`).
    fn id(&self) -> &'static str {
        "W023"
    }

    /// Returns the human-readable rule slug (`link-to-local-file`).
    fn name(&self) -> &'static str {
        "link-to-local-file"
    }

    /// Returns a one-line summary of the lint check.
    fn description(&self) -> &'static str {
        "Detect file: links to non-existent local files"
    }

    /// Scans unprotected lines for `[[file:...]]` links whose targets do not exist on disk.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);
        let base_dir = ctx.source.path.parent().unwrap_or(Path::new("."));
        let mut offset = 0;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            let mut search = raw;
            while let Some((raw_target, remaining)) = next_file_link(search) {
                let file_path = strip_search_suffix(raw_target);
                if !file_path.is_empty() && !base_dir.join(file_path).exists() {
                    let (line_num, _) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: 1,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!("file link '{file_path}' points to a non-existent file"),
                        fix: None,
                    });
                }
                search = remaining;
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

    /// Evaluates [`LinkToLocalFile`] against an in-memory `test.org` snippet.
    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        LinkToLocalFile.check(&ctx)
    }

    /// Verifies external `https:` links are ignored by `link-to-local-file`.
    #[test]
    fn no_file_links() {
        assert!(check_it("[[https://example.com]]\n").is_empty());
    }

    /// Verifies a missing local file link without description emits `W023`.
    #[test]
    fn missing_file() {
        let diags = check_it("[[file:nonexistent.txt]]\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("non-existent"));
    }

    /// Verifies a missing local file link with description emits `W023` for the path only.
    #[test]
    fn missing_file_with_desc() {
        let diags = check_it("[[file:nonexistent.txt][click here]]\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("'nonexistent.txt'"));
    }

    /// Verifies `file:` links inside protected `#+BEGIN_SRC` blocks are skipped.
    #[test]
    fn in_code_block_ignored() {
        let input = "#+BEGIN_SRC org\n[[file:nonexistent.txt]]\n#+END_SRC\n";
        assert!(check_it(input).is_empty());
    }

    /// Verifies `::*Heading` search suffixes are stripped before checking file existence.
    #[test]
    fn file_with_search_string() {
        let diags = check_it("[[file:nonexistent.org::*Heading]]\n");
        assert_eq!(diags.len(), 1);
    }

    /// Verifies that an existing local file link with a description produces zero diagnostics.
    #[test]
    fn existing_file_with_desc() {
        let diags = check_it("See [[file:Cargo.toml][Cargo manifest]] for details.\n");
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    /// Verifies multiple local file links (plain and with descriptions) on the same line.
    #[test]
    fn multiple_existing_links_on_one_line() {
        let input = "Read [[file:Cargo.toml]] and [[file:src/lib.rs][library root]].\n";
        let diags = check_it(input);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }
}
