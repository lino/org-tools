// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects `file:` links pointing to non-existent local files (`W023`).
//!
//! Spec: [§4.4 Handling Links](https://orgmode.org/manual/Handling-Links.html)
//! Org Syntax: <https://orgmode.org/worg/org-syntax.html#Links>
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
/// on the local filesystem (`W023`).
///
/// Resolves paths relative to the directory containing the source file. Strips
/// search strings (`::search`) before checking existence. Skips links inside
/// protected regions (code blocks, example blocks), comment lines, drawers,
/// and inline verbatim spans.
///
/// Spec: [§4.4 Handling Links](https://orgmode.org/manual/Handling-Links.html)
/// Org Syntax: <https://orgmode.org/worg/org-syntax.html#Links>
pub struct LinkToLocalFile;

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

/// Strips any `::search` suffix from a `file:` link target path.
fn strip_search_suffix(raw_target: &str) -> &str {
    match raw_target.find(LINK_SEARCH_DELIMITER) {
        Some(pos) => &raw_target[..pos],
        None => raw_target,
    }
}

/// Extracts next local file link target from `line` at `pos`, returning `(target_path, next_pos, col_offset)`.
fn extract_file_link(line: &str, pos: usize) -> Option<(&str, usize, usize)> {
    let after_prefix = line.get(pos..)?.strip_prefix(FILE_LINK_PREFIX)?;
    let col_offset = pos;
    let close_idx = after_prefix.find(LINK_CLOSE_DELIMITER)?;
    let target = if let Some(desc_idx) = after_prefix.find(LINK_DESC_DELIMITER) {
        if desc_idx < close_idx {
            &after_prefix[..desc_idx]
        } else {
            &after_prefix[..close_idx]
        }
    } else {
        &after_prefix[..close_idx]
    };
    let next_pos = pos + FILE_LINK_PREFIX.len() + close_idx + LINK_CLOSE_DELIMITER.len();
    Some((target, next_pos, col_offset))
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
        let mut in_drawer = false;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_protected(i, &regions)
                || step_property_drawer(raw, &mut in_drawer)
                || is_comment_line(raw)
            {
                offset += line.len() + 1;
                continue;
            }

            let mut pos = 0;
            while pos < raw.len() {
                if let Some(next_pos) = skip_inline_verbatim(raw, pos) {
                    pos = next_pos;
                    continue;
                }

                if raw[pos..].starts_with(FILE_LINK_PREFIX) {
                    if let Some((raw_target, next_pos, col_offset)) = extract_file_link(raw, pos) {
                        let file_path = strip_search_suffix(raw_target).trim();
                        if !file_path.is_empty() && !base_dir.join(file_path).exists() {
                            let (line_num, col) = ctx.source.line_col(offset + col_offset);
                            diagnostics.push(Diagnostic {
                                file: ctx.source.path.clone(),
                                line: line_num,
                                column: col,
                                severity: Severity::Warning,
                                rule_id: self.id(),
                                rule: self.name(),
                                message: format!(
                                    "file link '{file_path}' points to a non-existent file"
                                ),
                                fix: None,
                            });
                        }
                        pos = next_pos;
                        continue;
                    }
                }
                pos += raw[pos..].chars().next().map_or(1, char::len_utf8);
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
