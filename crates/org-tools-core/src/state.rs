// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Change TODO state on org-mode entries.
//!
//! Replaces the TODO keyword on a heading line and optionally manages the
//! `CLOSED:` planning timestamp when transitioning between todo and done states.

use crate::diagnostic::{Fix, Span};
use crate::document::{OrgDocument, OrgEntry};
use crate::formatter::apply_fixes;
use crate::rules::heading::{parse_heading_with_keywords, HeadingParts};
use crate::source::SourceFile;

/// Result of a state change operation.
#[derive(Debug, Clone)]
pub struct SetStateResult {
    /// The modified file content.
    pub content: String,
    /// Number of entries whose state was changed.
    pub changed: usize,
    /// Entry indices in [`OrgDocument::entries`] whose TODO keyword changed.
    pub changed_indices: Vec<usize>,
    /// Entry indices that transitioned from a non-done state to a done state.
    pub newly_done_indices: Vec<usize>,
}

/// Computes heading and `CLOSED:` fixes for a single entry transitioning to `new_state`.
fn build_entry_state_fixes(
    source: &SourceFile,
    doc: &OrgDocument,
    lines: &[&str],
    kw_refs: &[&str],
    idx: usize,
    new_state: &str,
    add_closed: bool,
) -> Option<(Vec<Fix>, bool)> {
    let entry = doc.entries.get(idx)?;
    let heading_idx = entry.heading_line - 1;
    let heading_line = *lines.get(heading_idx)?;
    let parts = parse_heading_with_keywords(heading_line, kw_refs)?;

    if parts.keyword == Some(new_state) {
        return None;
    }

    let is_new_done = doc.todo_keywords.is_done(new_state);
    let was_done = parts
        .keyword
        .map(|kw| doc.todo_keywords.is_done(kw))
        .unwrap_or(false);
    let newly_done = !was_done && is_new_done;

    let mut entry_fixes = Vec::with_capacity(2);
    let new_heading = build_heading_with_state(&parts, new_state);
    let line_start = line_offset(&source.content, heading_idx);
    let line_end = line_start + heading_line.len();
    entry_fixes.push(Fix::new(Span::new(line_start, line_end), new_heading));

    if add_closed {
        if newly_done {
            if let Some(fix) = add_closed_timestamp(entry, lines, &source.content) {
                entry_fixes.push(fix);
            }
        } else if !is_new_done && was_done {
            if let Some(fix) = remove_closed_timestamp(entry, lines, &source.content) {
                entry_fixes.push(fix);
            }
        }
    }

    Some((entry_fixes, newly_done))
}

/// Changes the TODO keyword on entries to `new_state`.
///
/// When `entry_indices` is `None`, all entries are considered.
/// When transitioning to a done state, a `CLOSED:` timestamp is added
/// unless `add_closed` is `false`. When transitioning from done to todo,
/// any existing `CLOSED:` line is removed.
pub fn set_state(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_indices: Option<&[usize]>,
    new_state: &str,
    add_closed: bool,
) -> Option<SetStateResult> {
    let lines: Vec<&str> = source.content.split('\n').collect();
    let kw_refs = doc.todo_keywords.all();
    let indices: Vec<usize> = match entry_indices {
        Some(idx) => idx.to_vec(),
        None => (0..doc.entries.len()).collect(),
    };

    let mut fixes: Vec<Fix> = Vec::new();
    let mut changed_indices = Vec::new();
    let mut newly_done_indices = Vec::new();

    for &idx in &indices {
        let Some((entry_fixes, newly_done)) =
            build_entry_state_fixes(source, doc, &lines, &kw_refs, idx, new_state, add_closed)
        else {
            continue;
        };
        fixes.extend(entry_fixes);
        changed_indices.push(idx);
        if newly_done {
            newly_done_indices.push(idx);
        }
    }

    if fixes.is_empty() {
        return None;
    }

    fixes.sort_by_key(|f| f.span.start);
    let content = apply_fixes(&source.content, &fixes);
    Some(SetStateResult {
        content,
        changed: changed_indices.len(),
        changed_indices,
        newly_done_indices,
    })
}

/// Builds a heading line with a replacement TODO keyword and `F007`-aligned tags.
fn build_heading_with_state(parts: &HeadingParts<'_>, new_state: &str) -> String {
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
    let prefix = format!("{stars} {new_state}{pri}{title}");
    let prefix_trimmed = prefix.trim_end();
    if parts.tags.is_empty() {
        return prefix_trimmed.to_string();
    }

    let tag_str = format!(":{}:", parts.tags.join(":"));
    let tag_col = crate::property::resolve_tag_column(None);
    let prefix_width = unicode_width::UnicodeWidthStr::width(prefix_trimmed);
    let tag_width = unicode_width::UnicodeWidthStr::width(tag_str.as_str());
    let spaces = if prefix_width + 1 + tag_width <= tag_col {
        tag_col - tag_width - prefix_width
    } else {
        1
    };
    format!("{prefix_trimmed}{}{tag_str}", " ".repeat(spaces))
}

/// Adds a `CLOSED:` timestamp after the heading or prepends it to an existing planning line.
fn add_closed_timestamp(entry: &OrgEntry, lines: &[&str], content: &str) -> Option<Fix> {
    let heading_idx = entry.heading_line - 1;
    let next_idx = heading_idx + 1;
    let now = now_timestamp();

    if next_idx < lines.len() {
        let next = lines[next_idx].trim();
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
            return None;
        }
    }

    let insert_offset = line_offset(content, next_idx);
    Some(Fix::new(
        Span::new(insert_offset, insert_offset),
        format!("CLOSED: [{now}]\n"),
    ))
}

/// Removes a `CLOSED:` timestamp from an entry's planning line.
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
    let has_other = next.contains("SCHEDULED:") || next.contains("DEADLINE:");
    if !has_other {
        let line_end = (line_start + lines[next_idx].len() + 1).min(content.len());
        return Some(Fix::new(Span::new(line_start, line_end), String::new()));
    }

    let line_text = lines[next_idx];
    let pos = line_text.find("CLOSED:")?;
    let bracket_end = line_text[pos..].find(']')?;
    let remove_start = line_start + pos;
    let mut remove_end = line_start + pos + bracket_end + 1;
    if remove_end < line_start + line_text.len()
        && content.as_bytes().get(remove_end) == Some(&b' ')
    {
        remove_end += 1;
    }
    Some(Fix::new(Span::new(remove_start, remove_end), String::new()))
}

/// Extracts local `(year, month, day, hour, minute, dow)` from Unix epoch `secs` via `libc`.
fn local_timestamp_parts(secs: i64) -> Option<(u16, u8, u8, u32, u32, u8)> {
    #[cfg(unix)]
    unsafe {
        let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
        let time_t = secs as libc::time_t;
        if !libc::localtime_r(&time_t, tm.as_mut_ptr()).is_null() {
            let tm = tm.assume_init();
            return Some((
                (tm.tm_year + 1900) as u16,
                (tm.tm_mon + 1) as u8,
                tm.tm_mday as u8,
                tm.tm_hour as u32,
                tm.tm_min as u32,
                ((tm.tm_wday + 6) % 7) as u8,
            ));
        }
    }

    #[cfg(windows)]
    unsafe {
        let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
        let time_t = secs as libc::time_t;
        if libc::localtime_s(tm.as_mut_ptr(), &time_t) == 0 {
            let tm = tm.assume_init();
            return Some((
                (tm.tm_year + 1900) as u16,
                (tm.tm_mon + 1) as u8,
                tm.tm_mday as u8,
                tm.tm_hour as u32,
                tm.tm_min as u32,
                ((tm.tm_wday + 6) % 7) as u8,
            ));
        }
    }

    None
}

/// Returns a formatted Org inactive timestamp payload for the current local time (`YYYY-MM-DD Day HH:MM`).
pub fn now_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    if let Some((year, month, d, h, m, dow)) = local_timestamp_parts(secs) {
        const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let day_name = DAY_NAMES[dow as usize % 7];
        return format!("{year:04}-{month:02}-{d:02} {day_name} {h:02}:{m:02}");
    }

    let days = secs / 86_400;
    let time_of_day = secs % 86_400;
    let h = time_of_day / 3_600;
    let m = (time_of_day % 3_600) / 60;
    let (year, month, d) = civil_from_days(days);
    let day_name = weekday_abbrev(year, month, d);
    format!("{year:04}-{month:02}-{d:02} {day_name} {h:02}:{m:02}")
}

/// Converts days since the Unix epoch (`1970-01-01`) into a civil `(year, month, day)` triple.
pub fn civil_from_days(days: i64) -> (u16, u8, u8) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = if month <= 2 { y + 1 } else { y } as u16;
    (year, month, d)
}

/// Converts a civil `(year, month, day)` date into days since `1970-01-01`.
pub fn days_from_civil(year: u16, month: u8, day: u8) -> i64 {
    let y = year as i64 - i64::from(month <= 2);
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u64;
    let m = month as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Returns the three-letter English weekday abbreviation (`"Mon"`..`"Sun"`) for a civil date.
pub fn weekday_abbrev(year: u16, month: u8, day: u8) -> &'static str {
    const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let dow = day_of_week(year, month, day);
    DAY_NAMES[dow as usize % 7]
}

/// Computes the zero-based Monday-first day of week (`0 = Mon`, `6 = Sun`).
fn day_of_week(year: u16, month: u8, day: u8) -> u8 {
    let y = year as i64;
    let m = month as i64;
    let d = day as i64;
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let dow = (y + y / 4 - y / 100 + y / 400 + t[(m - 1) as usize] + d) % 7;
    ((dow + 6) % 7) as u8
}

/// Calculates the byte offset of zero-based `line_idx` in `content`.
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

    /// Builds a parsed `(SourceFile, OrgDocument)` pair from raw Org text.
    fn make_doc(content: &str) -> (SourceFile, OrgDocument) {
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);
        (source, doc)
    }

    /// Verifies transitioning `TODO` to `DONE` records `newly_done_indices`.
    #[test]
    fn set_state_todo_to_done() {
        let (source, doc) = make_doc("* TODO Task\n");
        let result = set_state(&source, &doc, None, "DONE", false).unwrap();
        assert!(result.content.contains("* DONE Task"));
        assert_eq!(result.changed, 1);
        assert_eq!(result.newly_done_indices, vec![0]);
    }

    /// Verifies transitioning `DONE` to `TODO` does not mark the entry newly done.
    #[test]
    fn set_state_done_to_todo() {
        let (source, doc) = make_doc("* DONE Task\n");
        let result = set_state(&source, &doc, None, "TODO", false).unwrap();
        assert!(result.content.contains("* TODO Task"));
        assert!(result.newly_done_indices.is_empty());
    }

    /// Verifies priority cookies and `F007`-aligned heading tags are preserved on state changes.
    #[test]
    fn set_state_preserves_priority_and_tags() {
        let (source, doc) = make_doc("* TODO [#A] Task :work:\n");
        let result = set_state(&source, &doc, None, "DONE", false).unwrap();
        let line = result.content.trim_end();
        assert!(line.starts_with("* DONE [#A] Task "));
        assert!(line.ends_with(" :work:"));
        assert_eq!(
            unicode_width::UnicodeWidthStr::width(line),
            crate::property::resolve_tag_column(None)
        );
    }

    /// Verifies setting the existing state returns `None`.
    #[test]
    fn set_state_already_in_state() {
        let (source, doc) = make_doc("* TODO Task\n");
        let result = set_state(&source, &doc, None, "TODO", false);
        assert!(result.is_none());
    }

    /// Verifies targeting a specific entry index leaves sibling headings untouched.
    #[test]
    fn set_state_specific_entry() {
        let (source, doc) = make_doc("* TODO First\n* TODO Second\n");
        let result = set_state(&source, &doc, Some(&[1]), "DONE", false).unwrap();
        assert!(result.content.contains("* TODO First"));
        assert!(result.content.contains("* DONE Second"));
        assert_eq!(result.newly_done_indices, vec![1]);
    }

    /// Verifies transitioning to `DONE` with `add_closed = true` inserts a `CLOSED:` line.
    #[test]
    fn set_state_with_closed_timestamp() {
        let (source, doc) = make_doc("* TODO Task\n");
        let result = set_state(&source, &doc, None, "DONE", true).unwrap();
        assert!(result.content.contains("* DONE Task"));
        assert!(result.content.contains("CLOSED:"));
    }
}
