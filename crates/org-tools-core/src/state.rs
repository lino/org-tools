// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Change TODO state on org-mode entries.
//!
//! Replaces the TODO keyword on a heading line and optionally manages the
//! `CLOSED:` planning timestamp when transitioning between todo and done states.

use crate::diagnostic::{Fix, Span};
use crate::document::{OrgDocument, OrgEntry};
use crate::formatter::apply_fixes;
use crate::rules::heading::parse_heading_with_keywords;
use crate::source::SourceFile;

/// Result of a state change operation.
pub struct SetStateResult {
    /// The modified file content.
    pub content: String,
    /// Number of entries whose state was changed.
    pub changed: usize,
}

/// Change the TODO keyword on entries to `new_state`.
///
/// When `entry_indices` is `None`, all entries are considered.
/// When transitioning to a done state, a `CLOSED:` timestamp is added
/// unless `add_closed` is false. When transitioning from done to todo,
/// any existing `CLOSED:` line is removed.
pub fn set_state(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_indices: Option<&[usize]>,
    new_state: &str,
    add_closed: bool,
) -> Option<SetStateResult> {
    let lines: Vec<&str> = source.content.split('\n').collect();
    let kw_list = doc.todo_keywords.all();
    let kw_refs: Vec<&str> = kw_list.to_vec();
    let mut fixes: Vec<Fix> = Vec::new();

    let indices: Vec<usize> = match entry_indices {
        Some(idx) => idx.to_vec(),
        None => (0..doc.entries.len()).collect(),
    };

    let is_new_done = doc.todo_keywords.is_done(new_state);

    for &idx in &indices {
        let entry = &doc.entries[idx];
        let heading_idx = entry.heading_line - 1;
        let heading_line = lines[heading_idx];

        // Parse the heading to find the current keyword.
        let parts = parse_heading_with_keywords(heading_line, &kw_refs);
        let parts = match parts {
            Some(p) => p,
            None => continue,
        };

        // Skip if already in the target state.
        if parts.keyword == Some(new_state) {
            continue;
        }

        // Build the new heading line.
        let new_heading = build_heading_with_state(&parts, new_state);
        let line_start = line_offset(&source.content, heading_idx);
        let line_end = line_start + heading_line.len();
        fixes.push(Fix::new(Span::new(line_start, line_end), new_heading));

        // Handle CLOSED: timestamp.
        if add_closed {
            let was_done = parts
                .keyword
                .map(|kw| doc.todo_keywords.is_done(kw))
                .unwrap_or(false);

            if is_new_done && !was_done {
                // Transitioning to done: add CLOSED timestamp.
                if let Some(fix) = add_closed_timestamp(entry, &lines, &source.content) {
                    fixes.push(fix);
                }
            } else if !is_new_done && was_done {
                // Transitioning to todo: remove CLOSED timestamp.
                if let Some(fix) = remove_closed_timestamp(entry, &lines, &source.content) {
                    fixes.push(fix);
                }
            }
        }
    }

    if fixes.is_empty() {
        return None;
    }

    let changed = fixes
        .iter()
        .filter(|f| f.span.start != f.span.end || !f.replacement.is_empty())
        .count();
    fixes.sort_by_key(|f| f.span.start);
    let content = apply_fixes(&source.content, &fixes);
    Some(SetStateResult {
        content,
        changed: changed.min(indices.len()),
    })
}

/// Build a heading line with a new keyword.
fn build_heading_with_state(
    parts: &crate::rules::heading::HeadingParts<'_>,
    new_state: &str,
) -> String {
    let stars = "*".repeat(parts.level);
    let pri = parts
        .priority
        .map(|p| format!(" [#{p}]"))
        .unwrap_or_default();
    let title = if parts.title.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.title)
    };
    let tags = if parts.tags.is_empty() {
        String::new()
    } else {
        format!(" :{}:", parts.tags.join(":"))
    };
    format!("{stars} {new_state}{pri}{title}{tags}")
}

/// Add a CLOSED: timestamp after the heading (or on the existing planning line).
fn add_closed_timestamp(entry: &OrgEntry, lines: &[&str], content: &str) -> Option<Fix> {
    let heading_idx = entry.heading_line - 1;
    let next_idx = heading_idx + 1;

    let now = now_timestamp();

    if next_idx < lines.len() {
        let next = lines[next_idx].trim();
        // If there's already a planning line, prepend CLOSED: to it.
        if next.starts_with("SCHEDULED:")
            || next.starts_with("DEADLINE:")
            || next.starts_with("CLOSED:")
        {
            if !next.contains("CLOSED:") {
                let start = line_offset(content, next_idx);
                let indent = lines[next_idx].len() - lines[next_idx].trim_start().len();
                let prefix = &lines[next_idx][..indent];
                return Some(Fix::new(
                    Span::new(start, start),
                    format!("{prefix}CLOSED: [{now}] "),
                ));
            }
            return None; // Already has CLOSED.
        }
    }

    // Insert a new planning line after heading.
    let insert_offset = line_offset(content, next_idx);
    Some(Fix::new(
        Span::new(insert_offset, insert_offset),
        format!("CLOSED: [{now}]\n"),
    ))
}

/// Remove a CLOSED: timestamp from the planning line.
fn remove_closed_timestamp(entry: &OrgEntry, lines: &[&str], content: &str) -> Option<Fix> {
    let heading_idx = entry.heading_line - 1;
    let next_idx = heading_idx + 1;
    if next_idx >= lines.len() {
        return None;
    }

    let next = lines[next_idx].trim();
    if !next.contains("CLOSED:") {
        return None;
    }

    let line_start = line_offset(content, next_idx);

    // If CLOSED is the only planning keyword, remove the whole line.
    let has_other = next.contains("SCHEDULED:") || next.contains("DEADLINE:");
    if !has_other {
        let line_end = line_start + lines[next_idx].len() + 1; // +1 for newline
        return Some(Fix::new(Span::new(line_start, line_end), String::new()));
    }

    // Remove just the CLOSED: portion.
    let line_text = lines[next_idx];
    if let Some(pos) = line_text.find("CLOSED:") {
        // Find end of the CLOSED timestamp (next `]` after `[`).
        let after_closed = &line_text[pos..];
        if let Some(bracket_end) = after_closed.find(']') {
            let remove_start = line_start + pos;
            let remove_end = line_start + pos + bracket_end + 1;
            // Also remove trailing space.
            let remove_end = if remove_end < line_start + line_text.len()
                && content.as_bytes().get(remove_end) == Some(&b' ')
            {
                remove_end + 1
            } else {
                remove_end
            };
            return Some(Fix::new(Span::new(remove_start, remove_end), String::new()));
        }
    }

    None
}

/// Get a formatted timestamp for now: `YYYY-MM-DD Day HH:MM`.
pub fn now_timestamp() -> String {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;

    // Civil date from day count.
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = if month <= 2 { y + 1 } else { y } as u16;

    let dow = day_of_week(year, month, d);
    let day_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let day_name = day_names[dow as usize % 7];

    format!("{year:04}-{month:02}-{d:02} {day_name} {h:02}:{m:02}")
}

fn day_of_week(year: u16, month: u8, day: u8) -> u8 {
    let y = year as i64;
    let m = month as i64;
    let d = day as i64;
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let dow = (y + y / 4 - y / 100 + y / 400 + t[(m - 1) as usize] + d) % 7;
    ((dow + 6) % 7) as u8
}

/// Calculate byte offset of line `line_idx` (0-based) in content.
fn line_offset(content: &str, line_idx: usize) -> usize {
    content
        .split('\n')
        .take(line_idx)
        .map(|l| l.len() + 1)
        .sum()
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
    fn set_state_todo_to_done() {
        let (source, doc) = make_doc("* TODO Task\n");
        let result = set_state(&source, &doc, None, "DONE", false).unwrap();
        assert!(result.content.contains("* DONE Task"));
        assert_eq!(result.changed, 1);
    }

    #[test]
    fn set_state_done_to_todo() {
        let (source, doc) = make_doc("* DONE Task\n");
        let result = set_state(&source, &doc, None, "TODO", false).unwrap();
        assert!(result.content.contains("* TODO Task"));
    }

    #[test]
    fn set_state_preserves_priority_and_tags() {
        let (source, doc) = make_doc("* TODO [#A] Task :work:\n");
        let result = set_state(&source, &doc, None, "DONE", false).unwrap();
        assert!(result.content.contains("* DONE [#A] Task :work:"));
    }

    #[test]
    fn set_state_already_in_state() {
        let (source, doc) = make_doc("* TODO Task\n");
        let result = set_state(&source, &doc, None, "TODO", false);
        assert!(result.is_none());
    }

    #[test]
    fn set_state_specific_entry() {
        let (source, doc) = make_doc("* TODO First\n* TODO Second\n");
        let result = set_state(&source, &doc, Some(&[1]), "DONE", false).unwrap();
        assert!(result.content.contains("* TODO First"));
        assert!(result.content.contains("* DONE Second"));
    }

    #[test]
    fn set_state_with_closed_timestamp() {
        let (source, doc) = make_doc("* TODO Task\n");
        let result = set_state(&source, &doc, None, "DONE", true).unwrap();
        assert!(result.content.contains("* DONE Task"));
        assert!(result.content.contains("CLOSED:"));
    }
}
