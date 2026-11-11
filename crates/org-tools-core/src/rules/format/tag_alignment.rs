// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Aligns heading tags to column 77 (`F007`).
//!
//! Spec: [Setting Tags](https://orgmode.org/manual/Setting-Tags.html),
//! [Headlines](https://orgmode.org/worg/org-syntax.html#Headlines_and_Sections)

use unicode_width::UnicodeWidthStr;

use crate::diagnostic::{Fix, Span};
use crate::document::OrgDocument;
use crate::rules::heading::{parse_heading_with_keywords, HeadingParts};
use crate::rules::{FormatContext, FormatRule};

/// Aligns heading tags to a target column (default 77).
///
/// Spec: [Setting Tags](https://orgmode.org/manual/Setting-Tags.html),
/// [Headlines](https://orgmode.org/worg/org-syntax.html#Headlines_and_Sections)
///
/// Tags (`:tag1:tag2:`) at the end of heading lines are padded so that the
/// tag string ends at column 77, matching the Emacs `org-tags-column`
/// default. If the heading title is too long for the tags to reach column
/// 77, the tags are placed one space after the title instead. Uses Unicode
/// display width for correct alignment with CJK and wide characters.
///
/// Rule ID: `F007`
pub struct TagAlignment;

/// Target column for tag alignment (matching Emacs default).
const TAG_COLUMN: usize = 77;

/// Formats a heading line's tags to end at [`TAG_COLUMN`].
fn align_heading_tags(parts: &HeadingParts<'_>, raw: &str, offset: usize) -> Option<Fix> {
    if parts.tags.is_empty() {
        return None;
    }
    let tag_str = format!(":{}:", parts.tags.join(":"));
    let tag_width = UnicodeWidthStr::width(tag_str.as_str());

    let mut prefix = format!("{} ", "*".repeat(parts.level));
    if let Some(kw) = parts.keyword {
        prefix.push_str(kw);
        prefix.push(' ');
    }
    if let Some(pri) = parts.priority {
        prefix.push_str(&format!("[#{pri}] "));
    }
    prefix.push_str(parts.title);

    let prefix_width = UnicodeWidthStr::width(prefix.as_str());
    let desired_width = if prefix_width + 1 + tag_width <= TAG_COLUMN {
        TAG_COLUMN - tag_width
    } else {
        prefix_width + 1
    };
    let spaces_needed = desired_width.saturating_sub(prefix_width).max(1);
    let new_line = format!("{prefix}{}{tag_str}", " ".repeat(spaces_needed));
    (new_line != raw).then(|| Fix::new(Span::new(offset, offset + raw.len()), new_line))
}

impl FormatRule for TagAlignment {
    /// Returns the format rule identifier `"F007"`.
    fn id(&self) -> &'static str {
        "F007"
    }

    /// Returns the kebab-case rule name `"tag-alignment"`.
    fn name(&self) -> &'static str {
        "tag-alignment"
    }

    /// Returns a human-readable description of the rule.
    fn description(&self) -> &'static str {
        "Align heading tags to column 77"
    }

    /// Computes tag alignment fixes across all heading lines in `ctx.source`.
    fn format(&self, ctx: &FormatContext) -> Vec<Fix> {
        let doc = OrgDocument::from_source_with_config(ctx.source, ctx.config);
        let kw_refs = doc.todo_keywords.all();
        let mut fixes = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            if let Some(parts) = parse_heading_with_keywords(raw, &kw_refs) {
                if let Some(fix) = align_heading_tags(&parts, raw, offset) {
                    fixes.push(fix);
                }
            }
            offset += line.len() + 1;
        }

        fixes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::formatter::apply_fixes;
    use crate::source::SourceFile;

    /// Applies `TagAlignment` to `input` and returns the formatted string.
    fn format_it(input: &str) -> String {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = FormatContext::new(&source, &config);
        let fixes = TagAlignment.format(&ctx);
        apply_fixes(input, &fixes)
    }

    /// Verifies tags on a short heading are aligned to column 77.
    #[test]
    fn aligns_to_column_77() {
        let input = "* Heading :tag:\n";
        let result = format_it(input);
        assert!(result.contains(":tag:\n"));
        let line = result.trim_end();
        assert!(line.ends_with(":tag:"));
        assert_eq!(UnicodeWidthStr::width(line), TAG_COLUMN);
    }

    /// Verifies headings without tags are unchanged.
    #[test]
    fn no_tags_no_change() {
        let input = "* Heading\n";
        assert_eq!(format_it(input), input);
    }

    /// Verifies long titles place tags one space after the title.
    #[test]
    fn long_title_one_space() {
        let long_title = "A".repeat(80);
        let input = format!("* {long_title} :tag:\n");
        let result = format_it(&input);
        assert!(result.contains(&format!("{long_title} :tag:")));
    }

    /// Verifies already-aligned headings produce zero fixes.
    #[test]
    fn already_aligned() {
        let spaces = TAG_COLUMN - 9 - 5;
        let input = format!("* Heading{}:tag:\n", " ".repeat(spaces));
        assert_eq!(format_it(&input), input);
    }

    /// Verifies multiple tags are preserved and aligned.
    #[test]
    fn multiple_tags() {
        let input = "* Heading :tag1:tag2:\n";
        let result = format_it(input);
        assert!(result.contains(":tag1:tag2:"));
    }
}
