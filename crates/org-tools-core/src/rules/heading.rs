// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

/// Parsed components of an org heading line.
///
/// Spec: [§2.2 Headlines](https://orgmode.org/manual/Headlines.html),
/// [Syntax: Headlines](https://orgmode.org/worg/org-syntax.html#Headlines_and_Sections)
///
/// A heading line has the form:
/// `STARS KEYWORD PRIORITY TITLE TAGS`
/// where STARS is `*+` at column 0, KEYWORD is an optional TODO state,
/// PRIORITY is `[#X]`, TITLE is the text, and TAGS is `:tag1:tag2:` at EOL.
#[derive(Debug, PartialEq)]
pub struct HeadingParts<'a> {
    /// Number of leading stars (1 = top-level).
    pub level: usize,
    /// TODO keyword if present (e.g., `"TODO"`, `"DONE"`).
    pub keyword: Option<&'a str>,
    /// Priority cookie character if present (e.g., `'A'`).
    pub priority: Option<char>,
    /// Heading title text with keyword, priority, and tags stripped.
    pub title: &'a str,
    /// Tag strings without the surrounding colons (e.g., `["work", "urgent"]`).
    pub tags: Vec<&'a str>,
}

/// Default TODO keywords recognized when parsing headings.
const DEFAULT_TODO_KEYWORDS: &[&str] = &[
    "TODO",
    "DONE",
    "NEXT",
    "WAITING",
    "HOLD",
    "CANCELLED",
    "CANCELED",
    "STARTED",
    "DELEGATED",
    "REVIEW",
    "DRAFT",
    "PUBLISHED",
];

/// Returns the heading level (number of stars) if the line is a heading.
pub fn heading_level(line: &str) -> Option<usize> {
    if !line.starts_with('*') {
        return None;
    }
    let stars = line.len() - line.trim_start_matches('*').len();
    let after = &line[stars..];
    if after.is_empty() || after.starts_with(' ') {
        Some(stars)
    } else {
        None
    }
}

/// Returns true if the line is an org heading.
pub fn is_heading(line: &str) -> bool {
    heading_level(line).is_some()
}

/// Parses a heading line into its components.
pub fn parse_heading(line: &str) -> Option<HeadingParts<'_>> {
    let level = heading_level(line)?;
    let rest = line[level..].trim_start();

    // Extract tags from the end of the line.
    let (rest_no_tags, tags) = extract_tags(rest);
    let rest = rest_no_tags.trim_end();

    // Extract TODO keyword.
    let (rest, keyword) = extract_keyword(rest);

    // Extract priority.
    let (rest, priority) = extract_priority(rest);

    let title = rest.trim();

    Some(HeadingParts {
        level,
        keyword,
        priority,
        title,
        tags,
    })
}

/// Extracts tags from the end of a heading text. Returns (text_without_tags, tags).
fn extract_tags(text: &str) -> (&str, Vec<&str>) {
    let trimmed = text.trim_end();
    if !trimmed.ends_with(':') {
        return (text, Vec::new());
    }

    // Find the start of the tag string (last ` :` sequence).
    if let Some(tag_start) = trimmed.rfind(" :") {
        let tag_str = &trimmed[tag_start + 1..];
        // Validate: tags are `:word:word:` — each segment between colons is alphanumeric/underscore/@.
        let inner = tag_str.trim_start_matches(':').trim_end_matches(':');
        if inner.is_empty() {
            return (text, Vec::new());
        }
        let parts: Vec<&str> = inner.split(':').collect();
        let all_valid = parts.iter().all(|t| {
            !t.is_empty()
                && t.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '@')
        });
        if all_valid {
            return (&trimmed[..tag_start], parts);
        }
    }

    (text, Vec::new())
}

/// Extracts a TODO keyword from the start of heading text.
fn extract_keyword(text: &str) -> (&str, Option<&str>) {
    let first_word_end = text.find(' ').unwrap_or(text.len());
    let first_word = &text[..first_word_end];
    if DEFAULT_TODO_KEYWORDS.contains(&first_word) {
        let rest = if first_word_end < text.len() {
            text[first_word_end..].trim_start()
        } else {
            ""
        };
        (rest, Some(first_word))
    } else {
        (text, None)
    }
}

/// Extracts a priority cookie `[#X]` from the start of text.
fn extract_priority(text: &str) -> (&str, Option<char>) {
    if text.len() >= 4 && text.starts_with("[#") && text.as_bytes()[3] == b']' {
        let ch = text.as_bytes()[2] as char;
        if ch.is_ascii_alphabetic() {
            let rest = if text.len() > 4 {
                text[4..].trim_start()
            } else {
                ""
            };
            return (rest, Some(ch));
        }
    }
    (text, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_heading() {
        let parts = parse_heading("* Hello").unwrap();
        assert_eq!(parts.level, 1);
        assert_eq!(parts.keyword, None);
        assert_eq!(parts.priority, None);
        assert_eq!(parts.title, "Hello");
        assert!(parts.tags.is_empty());
    }

    #[test]
    fn heading_with_todo() {
        let parts = parse_heading("** TODO Write tests").unwrap();
        assert_eq!(parts.level, 2);
        assert_eq!(parts.keyword, Some("TODO"));
        assert_eq!(parts.title, "Write tests");
    }

    #[test]
    fn heading_with_priority() {
        let parts = parse_heading("* TODO [#A] Urgent task").unwrap();
        assert_eq!(parts.keyword, Some("TODO"));
        assert_eq!(parts.priority, Some('A'));
        assert_eq!(parts.title, "Urgent task");
    }

    #[test]
    fn heading_with_tags() {
        let parts = parse_heading("* Heading :tag1:tag2:").unwrap();
        assert_eq!(parts.title, "Heading");
        assert_eq!(parts.tags, vec!["tag1", "tag2"]);
    }

    #[test]
    fn heading_with_everything() {
        let parts = parse_heading("*** DONE [#B] Complete task :work:done:").unwrap();
        assert_eq!(parts.level, 3);
        assert_eq!(parts.keyword, Some("DONE"));
        assert_eq!(parts.priority, Some('B'));
        assert_eq!(parts.title, "Complete task");
        assert_eq!(parts.tags, vec!["work", "done"]);
    }

    #[test]
    fn not_a_heading() {
        assert!(parse_heading("not a heading").is_none());
        assert!(parse_heading("*bold text*").is_none());
    }

    #[test]
    fn heading_level() {
        assert_eq!(super::heading_level("* H"), Some(1));
        assert_eq!(super::heading_level("*** H"), Some(3));
        assert_eq!(super::heading_level("*not"), None);
    }

    #[test]
    fn empty_heading() {
        let parts = parse_heading("*").unwrap();
        assert_eq!(parts.level, 1);
        assert_eq!(parts.title, "");
    }

    #[test]
    fn heading_with_priority_only() {
        let parts = parse_heading("* [#C]").unwrap();
        assert_eq!(parts.priority, Some('C'));
        assert_eq!(parts.title, "");
    }
}
