// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Update statistic cookies (`[/]` and `[%]`) on org-mode headings.
//!
//! Statistic cookies show progress of child TODO entries:
//! - `[2/5]` — 2 of 5 children are done
//! - `[40%]` — 40% of children are done
//!
//! Spec: [§5.5 Breaking Down Tasks](https://orgmode.org/manual/Breaking-Down-Tasks.html),
//! [§5.6 Checkboxes](https://orgmode.org/manual/Checkboxes.html)

use crate::diagnostic::{Fix, Span};
use crate::document::OrgDocument;
use crate::formatter::apply_fixes;
use crate::source::SourceFile;

/// Result of updating statistic cookies.
pub struct UpdateCookieResult {
    /// The modified file content.
    pub content: String,
    /// Number of cookies that were updated.
    pub updated: usize,
}

/// Update statistic cookies on headings.
///
/// For each heading that has child entries with TODO keywords, compute the
/// completion ratio and update any existing `[n/m]` or `[n%]` cookie in the
/// heading title. When `insert_missing` is true, add a `[0/N]` cookie to
/// headings that have TODO children but no cookie.
pub fn update_cookies(
    source: &SourceFile,
    doc: &OrgDocument,
    insert_missing: bool,
) -> Option<UpdateCookieResult> {
    let lines: Vec<&str> = source.content.split('\n').collect();
    let mut fixes: Vec<Fix> = Vec::new();

    for entry in doc.entries.iter() {
        if entry.children.is_empty() {
            continue;
        }

        // Count children with TODO keywords.
        let mut total = 0usize;
        let mut done = 0usize;
        for &child_idx in &entry.children {
            let child = &doc.entries[child_idx];
            if child.keyword.is_some() {
                total += 1;
                if child
                    .keyword
                    .as_ref()
                    .is_some_and(|kw| doc.todo_keywords.is_done(kw))
                {
                    done += 1;
                }
            }
        }

        if total == 0 {
            continue;
        }

        let heading_idx = entry.heading_line - 1;
        let heading_line = lines[heading_idx];
        let line_start: usize = source
            .content
            .split('\n')
            .take(heading_idx)
            .map(|l| l.len() + 1)
            .sum();

        // Find existing cookie in the heading.
        if let Some((cookie_start, cookie_end, is_percent)) = find_cookie(heading_line) {
            let new_cookie = if is_percent {
                let pct = if total > 0 { (done * 100) / total } else { 0 };
                format!("[{pct}%]")
            } else {
                format!("[{done}/{total}]")
            };

            let abs_start = line_start + cookie_start;
            let abs_end = line_start + cookie_end;
            fixes.push(Fix::new(Span::new(abs_start, abs_end), new_cookie));
        } else if insert_missing {
            // Insert cookie before tags (or at end of title).
            let insert_pos = find_cookie_insert_position(heading_line);
            let abs_pos = line_start + insert_pos;
            fixes.push(Fix::new(
                Span::new(abs_pos, abs_pos),
                format!(" [{done}/{total}]"),
            ));
        }
    }

    if fixes.is_empty() {
        return None;
    }

    let updated = fixes.len();
    fixes.sort_by_key(|f| f.span.start);
    let content = apply_fixes(&source.content, &fixes);
    Some(UpdateCookieResult { content, updated })
}

/// Find a statistic cookie in a heading line.
/// Returns `(start_byte, end_byte, is_percent_format)`.
fn find_cookie(line: &str) -> Option<(usize, usize, bool)> {
    // Match [n/m] or [n%] patterns.
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            // Check for [digits/digits] or [digits%].
            let start = i;
            i += 1;
            // Consume digits.
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'/' {
                // Fraction cookie [n/m].
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b']' {
                    return Some((start, i + 1, false));
                }
            } else if i < bytes.len() && bytes[i] == b'%' {
                // Percent cookie [n%].
                i += 1;
                if i < bytes.len() && bytes[i] == b']' {
                    return Some((start, i + 1, true));
                }
            }
            // Not a cookie, continue searching.
            continue;
        }
        i += 1;
    }
    None
}

/// Find the position to insert a cookie (before tags, after title content).
fn find_cookie_insert_position(line: &str) -> usize {
    let trimmed = line.trim_end();
    // If line ends with tags `:tag1:tag2:`, insert before the tag string.
    if trimmed.ends_with(':') {
        if let Some(tag_start) = trimmed.rfind(" :") {
            return tag_start;
        }
    }
    // Otherwise, insert at the end of the content.
    trimmed.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc(content: &str) -> (SourceFile, OrgDocument) {
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);
        (source, doc)
    }

    #[test]
    fn updates_fraction_cookie() {
        let (source, doc) = make_doc("* Project [0/2]\n** TODO First\n** DONE Second\n");
        let result = update_cookies(&source, &doc, false).unwrap();
        assert!(result.content.contains("[1/2]"));
        assert_eq!(result.updated, 1);
    }

    #[test]
    fn updates_percent_cookie() {
        let (source, doc) = make_doc("* Project [0%]\n** TODO First\n** DONE Second\n");
        let result = update_cookies(&source, &doc, false).unwrap();
        assert!(result.content.contains("[50%]"));
    }

    #[test]
    fn inserts_missing_cookie() {
        let (source, doc) = make_doc("* Project\n** TODO First\n** DONE Second\n");
        let result = update_cookies(&source, &doc, true).unwrap();
        assert!(result.content.contains("[1/2]"));
    }

    #[test]
    fn no_insert_when_not_requested() {
        let (source, doc) = make_doc("* Project\n** TODO First\n** DONE Second\n");
        let result = update_cookies(&source, &doc, false);
        assert!(result.is_none());
    }

    #[test]
    fn no_todo_children_skipped() {
        let (source, doc) = make_doc("* Project [0/0]\n** Notes\n** More notes\n");
        // Children have no TODO keywords, so no update.
        let result = update_cookies(&source, &doc, false);
        assert!(result.is_none());
    }

    #[test]
    fn all_done() {
        let (source, doc) = make_doc("* Project [0/2]\n** DONE First\n** DONE Second\n");
        let result = update_cookies(&source, &doc, false).unwrap();
        assert!(result.content.contains("[2/2]"));
    }

    #[test]
    fn cookie_before_tags() {
        let (source, doc) = make_doc("* Project :work:\n** TODO Task\n");
        let result = update_cookies(&source, &doc, true).unwrap();
        // Cookie should be inserted before tags.
        assert!(result.content.contains("[0/1] :work:"));
    }

    #[test]
    fn find_cookie_fraction() {
        assert_eq!(find_cookie("* Project [2/5] more"), Some((10, 15, false)));
    }

    #[test]
    fn find_cookie_percent() {
        assert_eq!(find_cookie("* Project [40%] more"), Some((10, 15, true)));
    }

    #[test]
    fn find_cookie_none() {
        assert_eq!(find_cookie("* No cookie here"), None);
    }
}
