// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detects `file:` links pointing to non-existent local files.
//!
//! Spec: [§4.4 Handling Links](https://orgmode.org/manual/Handling-Links.html)
//! org-lint: `link-to-local-file`

use std::path::Path;

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

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

impl LintRule for LinkToLocalFile {
    fn id(&self) -> &'static str {
        "W023"
    }

    fn name(&self) -> &'static str {
        "link-to-local-file"
    }

    fn description(&self) -> &'static str {
        "Detect file: links to non-existent local files"
    }

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

            // Find [[file:path]] or [[file:path][desc]] patterns.
            let mut search = raw;
            while let Some(pos) = search.find("[[file:") {
                let rest = &search[pos + 7..];
                // Find the end of the path (]] or ][).
                let path_end = rest
                    .find("]]")
                    .or_else(|| rest.find("]["))
                    .unwrap_or(rest.len());
                let mut file_path = &rest[..path_end];

                // Strip search string (::search).
                if let Some(search_pos) = file_path.find("::") {
                    file_path = &file_path[..search_pos];
                }

                if !file_path.is_empty() {
                    let resolved = base_dir.join(file_path);
                    if !resolved.exists() {
                        let (line_num, _) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: 1,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                "file link '{}' points to a non-existent file",
                                file_path
                            ),
                            fix: None,
                        });
                    }
                }

                search = &rest[path_end..];
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

    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        LinkToLocalFile.check(&ctx)
    }

    #[test]
    fn no_file_links() {
        assert!(check_it("[[https://example.com]]\n").is_empty());
    }

    #[test]
    fn missing_file() {
        let diags = check_it("[[file:nonexistent.txt]]\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("non-existent"));
    }

    #[test]
    fn missing_file_with_desc() {
        let diags = check_it("[[file:nonexistent.txt][click here]]\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn in_code_block_ignored() {
        let input = "#+BEGIN_SRC org\n[[file:nonexistent.txt]]\n#+END_SRC\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn file_with_search_string() {
        let diags = check_it("[[file:nonexistent.org::*Heading]]\n");
        assert_eq!(diags.len(), 1);
    }
}
