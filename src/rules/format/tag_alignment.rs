/// Aligns heading tags to a target column (default 77).
///
/// Spec: [§6.2 Setting Tags](https://orgmode.org/manual/Setting-Tags.html)
///
/// Tags are `:tag1:tag2:` at the end of heading lines. Emacs aligns them to
/// `org-tags-column` (default 77). If the title is longer, tags go one space
/// after the title.
use unicode_width::UnicodeWidthStr;

use crate::diagnostic::{Fix, Span};
use crate::rules::heading::parse_heading;
use crate::rules::{FormatContext, FormatRule};

pub struct TagAlignment;

/// Target column for tag alignment (matching Emacs default).
const TAG_COLUMN: usize = 77;

impl FormatRule for TagAlignment {
    fn id(&self) -> &'static str {
        "F007"
    }

    fn name(&self) -> &'static str {
        "tag-alignment"
    }

    fn description(&self) -> &'static str {
        "Align heading tags to column 77"
    }

    fn format(&self, ctx: &FormatContext) -> Vec<Fix> {
        let content = &ctx.source.content;
        let mut fixes = Vec::new();
        let mut offset = 0;

        for line in content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if let Some(parts) = parse_heading(raw) {
                if !parts.tags.is_empty() {
                    let tag_str = format!(":{}:", parts.tags.join(":"));
                    let tag_width = UnicodeWidthStr::width(tag_str.as_str());

                    // Reconstruct the heading without tags.
                    let mut prefix = format!("{} ", "*".repeat(parts.level));
                    if let Some(kw) = parts.keyword {
                        prefix.push_str(kw);
                        prefix.push(' ');
                    }
                    if let Some(pri) = parts.priority {
                        prefix.push_str(&format!("[#{}] ", pri));
                    }
                    prefix.push_str(parts.title);

                    let prefix_width = UnicodeWidthStr::width(prefix.as_str());

                    // Calculate needed spaces to reach tag column.
                    let desired_width = if prefix_width + 1 + tag_width <= TAG_COLUMN {
                        TAG_COLUMN - tag_width
                    } else {
                        // Title too long — put tags one space after.
                        prefix_width + 1
                    };

                    let spaces_needed = if desired_width > prefix_width {
                        desired_width - prefix_width
                    } else {
                        1
                    };

                    let new_line = format!("{}{}{}", prefix, " ".repeat(spaces_needed), tag_str);

                    if new_line != raw {
                        fixes.push(Fix::new(
                            Span::new(offset, offset + raw.len()),
                            new_line,
                        ));
                    }
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

    fn format_it(input: &str) -> String {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = FormatContext::new(&source, &config);
        let fixes = TagAlignment.format(&ctx);
        apply_fixes(input, &fixes)
    }

    #[test]
    fn aligns_to_column_77() {
        let input = "* Heading :tag:\n";
        let result = format_it(input);
        // Tag string ":tag:" is 5 chars. Should end at column 77.
        // So tag starts at column 72, meaning 72 - 10 ("* Heading ") = 62 spaces.
        assert!(result.contains(":tag:\n"));
        // The heading text + spaces + tag should make the tag end at col 77.
        let line = result.trim_end();
        assert!(line.ends_with(":tag:"));
        assert_eq!(UnicodeWidthStr::width(line), TAG_COLUMN);
    }

    #[test]
    fn no_tags_no_change() {
        let input = "* Heading\n";
        assert_eq!(format_it(input), input);
    }

    #[test]
    fn long_title_one_space() {
        // Title longer than 77 cols — tags get one space after title.
        let long_title = "A".repeat(80);
        let input = format!("* {} :tag:\n", long_title);
        let result = format_it(&input);
        assert!(result.contains(&format!("{} :tag:", long_title)));
    }

    #[test]
    fn already_aligned() {
        // Build a perfectly aligned heading. Line should be exactly TAG_COLUMN wide.
        // "* Heading" = 9 chars, ":tag:" = 5 chars, total = 77 → need 63 spaces.
        let prefix_width = 9; // "* Heading"
        let tag_width = 5; // ":tag:"
        let spaces = TAG_COLUMN - prefix_width - tag_width;
        let input = format!("* Heading{}:tag:\n", " ".repeat(spaces));
        assert_eq!(format_it(&input), input);
    }

    #[test]
    fn multiple_tags() {
        let input = "* Heading :tag1:tag2:\n";
        let result = format_it(input);
        assert!(result.contains(":tag1:tag2:"));
    }
}
