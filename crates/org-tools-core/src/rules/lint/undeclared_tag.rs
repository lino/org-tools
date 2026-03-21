// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Validates that tags used on headings are declared in `#+TAGS:`.
//!
//! Spec: [§6.2 Setting Tags](https://orgmode.org/manual/Setting-Tags.html)
//!
//! When one or more `#+TAGS:` lines are present, only declared tags (or tags
//! matching a declared regex pattern) are considered valid. Headings using
//! undeclared tags receive a warning. `#+FILETAGS:` tags are also validated.
//!
//! If no `#+TAGS:` lines are present, any tag is allowed (no diagnostics).

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::heading::{parse_heading, parse_tags_spec, TagSpec};
use crate::rules::{LintContext, LintRule};

/// Warns when a heading uses a tag not declared in `#+TAGS:`.
///
/// When `#+TAGS:` is configured, all tags used in the file should be from the
/// declared set. Undeclared tags are likely typos.
///
/// Spec: [§6.2 Setting Tags](https://orgmode.org/manual/Setting-Tags.html)
pub struct UndeclaredTag;

impl LintRule for UndeclaredTag {
    fn id(&self) -> &'static str {
        "W033"
    }

    fn name(&self) -> &'static str {
        "undeclared-tag"
    }

    fn description(&self) -> &'static str {
        "Tag not declared in #+TAGS:"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        // First pass: collect #+TAGS: lines and #+FILETAGS: from the preamble.
        let mut tags_values: Vec<&str> = Vec::new();
        let mut filetags_value: Option<&str> = None;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            // Stop collecting preamble at the first heading.
            if parse_heading(raw).is_some() {
                break;
            }

            let trimmed = raw.trim();
            if let Some(rest) = trimmed.strip_prefix("#+") {
                if let Some(colon) = rest.find(':') {
                    let key = rest[..colon].trim().to_uppercase();
                    let val = rest[colon + 1..].trim();
                    if key == "TAGS" {
                        tags_values.push(val);
                    } else if key == "FILETAGS" {
                        filetags_value = Some(val);
                    }
                }
            }
        }

        let tag_spec = parse_tags_spec(&tags_values);

        // If no #+TAGS: declared (allow_any), skip validation.
        if tag_spec.allow_any {
            return diagnostics;
        }

        // Validate #+FILETAGS: tags against the spec.
        if let Some(ft_val) = filetags_value {
            let ft_offset = find_keyword_offset(&ctx.source.content, "FILETAGS");
            let filetags: Vec<&str> = ft_val
                .trim_matches(':')
                .split(':')
                .filter(|s| !s.is_empty())
                .collect();
            for tag in filetags {
                if !tag_spec.matches_tag(tag) {
                    let (line_num, _) = ctx.source.line_col(ft_offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: 1,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!("file tag \"{tag}\" is not declared in #+TAGS:"),
                        fix: None,
                    });
                }
            }
        }

        // Second pass: check heading tags.
        let mut offset = 0;
        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if let Some(parts) = parse_heading(raw) {
                for tag in &parts.tags {
                    if !tag_spec.matches_tag(tag) {
                        let (line_num, _) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: 1,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!("tag \"{tag}\" is not declared in #+TAGS:"),
                            fix: None,
                        });
                    }
                }
            }

            offset += line.len() + 1;
        }

        diagnostics
    }
}

/// Find the byte offset of a `#+KEY:` keyword line in the content.
fn find_keyword_offset(content: &str, key: &str) -> usize {
    let needle_upper = format!("#+{}:", key.to_uppercase());
    let needle_lower = format!("#+{}:", key.to_lowercase());
    let mut offset = 0;
    for line in content.split('\n') {
        let trimmed = line.trim();
        let upper = trimmed.to_uppercase();
        if upper.starts_with(&needle_upper) || upper.starts_with(&needle_lower) {
            return offset;
        }
        offset += line.len() + 1;
    }
    0
}

/// Check if a tag spec matches — delegates to [`TagSpec::matches_tag`].
#[allow(dead_code)]
fn matches(tag_spec: &TagSpec, tag: &str) -> bool {
    tag_spec.matches_tag(tag)
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
        UndeclaredTag.check(&ctx)
    }

    #[test]
    fn no_tags_keyword_allows_any() {
        let diags = check_it("* Heading :anything:goes:\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn empty_tags_keyword_allows_any() {
        let diags = check_it("#+TAGS:\n* Heading :anything:\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn declared_tags_no_warning() {
        let diags = check_it("#+TAGS: @work @home laptop\n* Task :@work:laptop:\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn undeclared_tag_warning() {
        let diags = check_it("#+TAGS: @work @home\n* Task :@work:unknown:\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unknown"));
        assert!(diags[0].message.contains("not declared"));
    }

    #[test]
    fn at_prefix_tags() {
        let diags = check_it("#+TAGS: @work @home\n* Task :@work:\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn mutually_exclusive_group_tags_valid() {
        let diags = check_it("#+TAGS: { @work @home } laptop\n* Task :@work:laptop:\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn hierarchy_group_tags_valid() {
        let diags = check_it("#+TAGS: [ Project : SubA SubB ]\n* Task :Project:\n** Sub :SubA:\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn multiple_tags_lines_combined() {
        let diags = check_it("#+TAGS: @work\n#+TAGS: laptop\n* Task :@work:laptop:\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn filetags_validated() {
        let diags = check_it("#+TAGS: @work @home\n#+FILETAGS: :@work:unknown:\n* Heading\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unknown"));
        assert!(diags[0].message.contains("file tag"));
    }

    #[test]
    fn filetags_valid() {
        let diags = check_it("#+TAGS: @work @home\n#+FILETAGS: :@work:\n* Heading\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn fast_access_keys_stripped() {
        let diags = check_it("#+TAGS: @work(w) @home(h)\n* Task :@work:\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn case_sensitive_tags() {
        let diags = check_it("#+TAGS: Work\n* Task :work:\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("work"));
    }

    #[test]
    fn regex_member_matches() {
        let diags = check_it("#+TAGS: [ Project : {P@.+} ]\n* Task :P@frontend:\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn regex_member_no_match() {
        let diags = check_it("#+TAGS: [ Project : {P@.+} ]\n* Task :frontend:\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("frontend"));
    }

    #[test]
    fn multiple_undeclared_tags() {
        let diags = check_it("#+TAGS: @work\n* Task :bad1:bad2:\n");
        assert_eq!(diags.len(), 2);
    }

    #[test]
    fn no_headings_no_warnings() {
        let diags = check_it("#+TAGS: @work\nSome text\n");
        assert!(diags.is_empty());
    }
}
