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
use crate::document::OrgDocument;
use crate::rules::heading::{parse_heading_with_keywords, parse_tags_spec, TagSpec};
use crate::rules::{LintContext, LintRule};

/// Warns when a heading uses a tag not declared in `#+TAGS:`.
///
/// When `#+TAGS:` is configured, all tags used in the file should be from the
/// declared set. Undeclared tags are likely typos.
///
/// Spec: [§6.2 Setting Tags](https://orgmode.org/manual/Setting-Tags.html)
pub struct UndeclaredTag;

/// Collects `#+TAGS:` values and optional `#+FILETAGS:` from the document preamble.
fn collect_preamble_tag_keywords<'a>(
    content: &'a str,
    kw_refs: &[&str],
) -> (Vec<&'a str>, Option<&'a str>) {
    let mut tags_values: Vec<&str> = Vec::new();
    let mut filetags_value: Option<&str> = None;

    for line in content.split('\n') {
        let raw = line.strip_suffix('\r').unwrap_or(line);
        if parse_heading_with_keywords(raw, kw_refs).is_some() {
            break;
        }
        if let Some(rest) = raw.trim().strip_prefix("#+") {
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
    (tags_values, filetags_value)
}

/// Validates `#+FILETAGS:` tags against `tag_spec` and appends warnings to `diagnostics`.
fn check_filetags(
    ctx: &LintContext,
    tag_spec: &TagSpec,
    ft_val: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let ft_offset = find_keyword_offset(&ctx.source.content, "FILETAGS");
    let filetags = ft_val
        .trim_matches(':')
        .split(':')
        .filter(|s| !s.is_empty());
    for tag in filetags {
        if !tag_spec.matches_tag(tag) {
            let (line_num, _) = ctx.source.line_col(ft_offset);
            diagnostics.push(Diagnostic {
                file: ctx.source.path.clone(),
                line: line_num,
                column: 1,
                severity: Severity::Warning,
                rule_id: "W033",
                rule: "undeclared-tag",
                message: format!("file tag \"{tag}\" is not declared in #+TAGS:"),
                fix: None,
            });
        }
    }
}

impl LintRule for UndeclaredTag {
    /// Returns the lint rule identifier `"W033"`.
    fn id(&self) -> &'static str {
        "W033"
    }

    /// Returns the kebab-case rule name `"undeclared-tag"`.
    fn name(&self) -> &'static str {
        "undeclared-tag"
    }

    /// Returns a one-line description of the rule.
    fn description(&self) -> &'static str {
        "Tag not declared in #+TAGS:"
    }

    /// Checks `#+FILETAGS:` and heading tags against declared `#+TAGS:` specifications.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let doc = OrgDocument::from_source_with_config(ctx.source, ctx.config);
        let kw_refs = doc.todo_keywords.all();
        let (tags_values, filetags_value) =
            collect_preamble_tag_keywords(&ctx.source.content, &kw_refs);
        let tag_spec = parse_tags_spec(&tags_values);
        let mut diagnostics = Vec::new();
        if tag_spec.allow_any {
            return diagnostics;
        }
        if let Some(ft_val) = filetags_value {
            check_filetags(ctx, &tag_spec, ft_val, &mut diagnostics);
        }

        let mut offset = 0;
        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            if let Some(parts) = parse_heading_with_keywords(raw, &kw_refs) {
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

/// Finds the byte offset of a `#+KEY:` keyword line in `content`.
fn find_keyword_offset(content: &str, key: &str) -> usize {
    let needle_upper = format!("#+{}:", key.to_uppercase());
    let needle_lower = format!("#+{}:", key.to_lowercase());
    let mut offset = 0;
    for line in content.split('\n') {
        let upper = line.trim().to_uppercase();
        if upper.starts_with(&needle_upper) || upper.starts_with(&needle_lower) {
            return offset;
        }
        offset += line.len() + 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    /// Runs `UndeclaredTag` on `input` with default configuration.
    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        UndeclaredTag.check(&ctx)
    }

    /// Verifies files without `#+TAGS:` allow any tag.
    #[test]
    fn no_tags_keyword_allows_any() {
        assert!(check_it("* Heading :anything:goes:\n").is_empty());
    }

    /// Verifies an empty `#+TAGS:` line allows any tag.
    #[test]
    fn empty_tags_keyword_allows_any() {
        assert!(check_it("#+TAGS:\n* Heading :anything:\n").is_empty());
    }

    /// Verifies declared tags emit no warning.
    #[test]
    fn declared_tags_no_warning() {
        assert!(check_it("#+TAGS: @work @home laptop\n* Task :@work:laptop:\n").is_empty());
    }

    /// Verifies undeclared heading tags emit a `W033` warning.
    #[test]
    fn undeclared_tag_warning() {
        let diags = check_it("#+TAGS: @work @home\n* Task :@work:unknown:\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unknown"));
    }

    /// Verifies `@`-prefixed tags are recognized.
    #[test]
    fn at_prefix_tags() {
        assert!(check_it("#+TAGS: @work @home\n* Task :@work:\n").is_empty());
    }

    /// Verifies mutually exclusive `{ ... }` tag groups are valid.
    #[test]
    fn mutually_exclusive_group_tags_valid() {
        assert!(check_it("#+TAGS: { @work @home } laptop\n* Task :@work:laptop:\n").is_empty());
    }

    /// Verifies hierarchy `[ Group : ... ]` tag groups are valid.
    #[test]
    fn hierarchy_group_tags_valid() {
        assert!(
            check_it("#+TAGS: [ Project : SubA SubB ]\n* Task :Project:\n** Sub :SubA:\n")
                .is_empty()
        );
    }

    /// Verifies multiple `#+TAGS:` lines combine additively.
    #[test]
    fn multiple_tags_lines_combined() {
        assert!(check_it("#+TAGS: @work\n#+TAGS: laptop\n* Task :@work:laptop:\n").is_empty());
    }

    /// Verifies undeclared `#+FILETAGS:` tags emit a warning.
    #[test]
    fn filetags_validated() {
        let diags = check_it("#+TAGS: @work @home\n#+FILETAGS: :@work:unknown:\n* Heading\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("file tag"));
    }

    /// Verifies declared `#+FILETAGS:` tags pass without warnings.
    #[test]
    fn filetags_valid() {
        assert!(check_it("#+TAGS: @work @home\n#+FILETAGS: :@work:\n* Heading\n").is_empty());
    }

    /// Verifies fast-access keys `(w)` are stripped when matching tags.
    #[test]
    fn fast_access_keys_stripped() {
        assert!(check_it("#+TAGS: @work(w) @home(h)\n* Task :@work:\n").is_empty());
    }

    /// Verifies tag matching is case-sensitive.
    #[test]
    fn case_sensitive_tags() {
        let diags = check_it("#+TAGS: Work\n* Task :work:\n");
        assert_eq!(diags.len(), 1);
    }

    /// Verifies regex tag members `{P@.+}` match conforming tags.
    #[test]
    fn regex_member_matches() {
        assert!(check_it("#+TAGS: [ Project : {P@.+} ]\n* Task :P@frontend:\n").is_empty());
    }

    /// Verifies regex tag members flag non-matching tags.
    #[test]
    fn regex_member_no_match() {
        let diags = check_it("#+TAGS: [ Project : {P@.+} ]\n* Task :frontend:\n");
        assert_eq!(diags.len(), 1);
    }

    /// Verifies multiple undeclared tags on a single heading each emit a warning.
    #[test]
    fn multiple_undeclared_tags() {
        let diags = check_it("#+TAGS: @work\n* Task :bad1:bad2:\n");
        assert_eq!(diags.len(), 2);
    }

    /// Verifies files without headings produce no warnings.
    #[test]
    fn no_headings_no_warnings() {
        assert!(check_it("#+TAGS: @work\nSome text\n").is_empty());
    }
}
