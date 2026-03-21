// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lightweight heading tree model for org-mode documents.
//!
//! [`OrgDocument`] is built from a [`SourceFile`] using a single-pass line scan.
//! It is not a full AST — just enough structure for querying, clocking, and
//! locator resolution.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::rules::heading::{
    parse_heading, parse_heading_with_keywords, priority_range_from_file, todo_keywords_from_file,
    PriorityRange, TodoKeywords,
};
use crate::rules::timestamp::{parse_timestamp, OrgTimestamp};
use crate::source::SourceFile;

/// A parsed org document with heading tree structure.
#[derive(Debug)]
pub struct OrgDocument {
    /// Path to the source file.
    pub file: PathBuf,
    /// All heading entries in document order.
    pub entries: Vec<OrgEntry>,
    /// File-level properties (from a property drawer before any heading).
    pub file_properties: HashMap<String, String>,
    /// File-level keywords (`#+KEY: value` before any heading).
    pub file_keywords: HashMap<String, String>,
    /// TODO keyword configuration parsed from `#+TODO:` / `#+SEQ_TODO:` / `#+TYP_TODO:`.
    pub todo_keywords: TodoKeywords,
    /// Priority range parsed from `#+PRIORITIES:`.
    pub priority_range: PriorityRange,
    /// File-level tags from `#+FILETAGS:` (inherited by all entries).
    pub filetags: Vec<String>,
    /// File-wide default properties from `#+PROPERTY:` keyword lines.
    pub default_properties: HashMap<String, String>,
}

/// A single heading entry in an org document.
#[derive(Debug)]
pub struct OrgEntry {
    /// Heading level (1 = top-level `*`).
    pub level: usize,
    /// TODO keyword if present (e.g., `"TODO"`, `"DONE"`).
    pub keyword: Option<String>,
    /// Priority cookie character if present (e.g., `'A'`).
    pub priority: Option<char>,
    /// Heading title text (keyword, priority, and tags stripped).
    pub title: String,
    /// Local tags on this heading (not inherited).
    pub tags: Vec<String>,
    /// Properties from the `:PROPERTIES:` drawer.
    pub properties: HashMap<String, String>,
    /// Planning timestamps (SCHEDULED, DEADLINE, CLOSED).
    pub planning: Planning,
    /// Clock entries from `:LOGBOOK:` or standalone `CLOCK:` lines.
    pub clocks: Vec<ClockEntry>,
    /// Line number of the heading (1-based).
    pub heading_line: usize,
    /// Byte offset of the heading line start.
    pub heading_offset: usize,
    /// Line number where this entry's content ends (exclusive, 1-based).
    pub content_end_line: usize,
    /// Index of parent entry in [`OrgDocument::entries`], or `None` for top-level.
    pub parent: Option<usize>,
    /// Indices of direct children in [`OrgDocument::entries`].
    pub children: Vec<usize>,
    /// The original heading line text (for `--format=org` output).
    pub raw_heading: String,
}

/// Planning timestamps attached to a heading.
#[derive(Debug, Default)]
pub struct Planning {
    /// `SCHEDULED:` timestamp.
    pub scheduled: Option<OrgTimestamp>,
    /// `DEADLINE:` timestamp.
    pub deadline: Option<OrgTimestamp>,
    /// `CLOSED:` timestamp.
    pub closed: Option<OrgTimestamp>,
}

/// A single CLOCK entry.
#[derive(Debug)]
pub struct ClockEntry {
    /// Clock start timestamp.
    pub start: OrgTimestamp,
    /// Clock end timestamp (`None` for a running clock).
    pub end: Option<OrgTimestamp>,
    /// Computed duration in minutes (from `=> HH:MM`).
    pub duration_minutes: Option<i64>,
    /// Line number of this `CLOCK:` line (1-based).
    pub line: usize,
}

impl OrgDocument {
    /// Build a document from a [`SourceFile`] using line-based parsing.
    pub fn from_source(source: &SourceFile) -> Self {
        let lines: Vec<&str> = source.content.split('\n').collect();
        let mut entries: Vec<OrgEntry> = Vec::new();
        let mut file_properties: HashMap<String, String> = HashMap::new();
        let mut file_keywords: HashMap<String, String> = HashMap::new();
        // Stack of (level, index) for tracking parent-child.
        let mut level_stack: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        let mut first_heading_seen = false;
        let mut todo_kw = TodoKeywords::default();
        let mut pri_range = PriorityRange::default();
        let mut filetags: Vec<String> = Vec::new();
        let mut default_properties: HashMap<String, String> = HashMap::new();
        // Owned keyword strings for the lifetime of parsing; the &str refs
        // into `kw_refs` are used by `parse_heading_with_keywords`.
        let mut kw_strs: Vec<String> = todo_kw.all().iter().map(|s| s.to_string()).collect();
        let mut kw_refs: Vec<&str> = kw_strs.iter().map(|s| s.as_str()).collect();

        while i < lines.len() {
            let line = lines[i];

            // Before any heading: collect file-level keywords and properties.
            if !first_heading_seen {
                if parse_heading(line).is_some() {
                    first_heading_seen = true;
                    // Derive settings from file keywords collected so far.
                    todo_kw = todo_keywords_from_file(&file_keywords);
                    kw_strs = todo_kw.all().iter().map(|s| s.to_string()).collect();
                    kw_refs = kw_strs.iter().map(|s| s.as_str()).collect();
                    pri_range = priority_range_from_file(&file_keywords);
                    filetags = parse_filetags(file_keywords.get("FILETAGS"));
                    default_properties = parse_default_properties(&file_keywords);
                    // Fall through to heading processing below.
                } else {
                    if let Some((key, val)) = parse_keyword_line(line) {
                        file_keywords.insert(key, val);
                    }
                    // Check for file-level property drawer.
                    let trimmed = line.trim();
                    if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
                        let props = parse_property_drawer(&lines, i + 1);
                        file_properties = props.0;
                        i = props.1;
                        continue;
                    }
                    i += 1;
                    continue;
                }
            }

            if let Some(heading) = parse_heading_with_keywords(line, &kw_refs) {
                let entry_idx = entries.len();
                let heading_line = i + 1; // 1-based
                let heading_offset = if i < source.line_count() {
                    source.line_start(i)
                } else {
                    source.content.len()
                };

                // Determine parent.
                let parent = find_parent(&level_stack, heading.level);

                // Update parent's children list.
                if let Some(parent_idx) = parent {
                    entries[parent_idx].children.push(entry_idx);
                }

                // Pop stack entries at same or deeper level.
                while level_stack
                    .last()
                    .is_some_and(|&(lvl, _)| lvl >= heading.level)
                {
                    level_stack.pop();
                }
                level_stack.push((heading.level, entry_idx));

                let raw_heading = line.to_string();
                let mut entry = OrgEntry {
                    level: heading.level,
                    keyword: heading.keyword.map(|s| s.to_string()),
                    priority: heading.priority,
                    title: heading.title.to_string(),
                    tags: heading.tags.iter().map(|s| s.to_string()).collect(),
                    properties: HashMap::new(),
                    planning: Planning::default(),
                    clocks: Vec::new(),
                    heading_line,
                    heading_offset,
                    content_end_line: lines.len(), // Will be fixed later.
                    parent,
                    children: Vec::new(),
                    raw_heading,
                };

                i += 1;

                // Parse planning line (immediately after heading).
                if i < lines.len() {
                    let next = lines[i].trim();
                    if is_planning_line(next) {
                        entry.planning = parse_planning(next);
                        i += 1;
                    }
                }

                // Parse property drawer (immediately after heading or planning).
                if i < lines.len() && lines[i].trim().eq_ignore_ascii_case(":PROPERTIES:") {
                    let (props, end) = parse_property_drawer(&lines, i + 1);
                    entry.properties = props;
                    i = end;
                }

                // Scan entry body for LOGBOOK and standalone CLOCK lines.
                i = scan_entry_body(&lines, i, heading.level, &mut entry);

                entries.push(entry);
            } else {
                i += 1;
            }
        }

        // Fix content_end_line for each entry.
        for idx in 0..entries.len() {
            let end = if idx + 1 < entries.len() {
                entries[idx + 1].heading_line
            } else {
                lines.len() + 1
            };
            entries[idx].content_end_line = end;
        }

        Self {
            file: source.path.clone(),
            entries,
            file_properties,
            file_keywords,
            todo_keywords: todo_kw,
            priority_range: pri_range,
            filetags,
            default_properties,
        }
    }

    /// Find an entry by its `:ID:` property.
    pub fn find_by_id(&self, id: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.properties.get("ID").is_some_and(|v| v == id))
    }

    /// Find an entry by its `:CUSTOM_ID:` property.
    pub fn find_by_custom_id(&self, custom_id: &str) -> Option<usize> {
        self.entries.iter().position(|e| {
            e.properties
                .get("CUSTOM_ID")
                .is_some_and(|v| v == custom_id)
        })
    }

    /// Get the outline path for an entry (ancestor titles from root to entry).
    pub fn outline_path(&self, entry_idx: usize) -> Vec<&str> {
        let mut path = Vec::new();
        let mut idx = Some(entry_idx);
        while let Some(i) = idx {
            path.push(self.entries[i].title.as_str());
            idx = self.entries[i].parent;
        }
        path.reverse();
        path
    }

    /// Get all inherited tags for an entry (local + ancestor tags).
    pub fn inherited_tags(&self, entry_idx: usize) -> Vec<&str> {
        let mut tags = Vec::new();
        let mut idx = Some(entry_idx);
        while let Some(i) = idx {
            for tag in &self.entries[i].tags {
                if !tags.contains(&tag.as_str()) {
                    tags.push(tag.as_str());
                }
            }
            idx = self.entries[i].parent;
        }
        // Append file-level tags from #+FILETAGS:.
        for tag in &self.filetags {
            if !tags.contains(&tag.as_str()) {
                tags.push(tag.as_str());
            }
        }
        tags
    }

    /// Look up a property on an entry, falling back to file-wide `#+PROPERTY:` defaults.
    pub fn property(&self, entry_idx: usize, key: &str) -> Option<&str> {
        self.entries[entry_idx]
            .properties
            .get(key)
            .or_else(|| self.default_properties.get(key))
            .map(|s| s.as_str())
    }
}

/// Returns true if the line is a planning line (starts with SCHEDULED, DEADLINE, or CLOSED).
fn is_planning_line(trimmed: &str) -> bool {
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

/// Parses SCHEDULED, DEADLINE, and CLOSED timestamps from a planning line.
fn parse_planning(line: &str) -> Planning {
    let mut planning = Planning::default();

    if let Some(pos) = line.find("SCHEDULED:") {
        let rest = &line[pos + "SCHEDULED:".len()..];
        let rest = rest.trim_start();
        if let Some((ts, _)) = parse_timestamp(rest, 0) {
            planning.scheduled = Some(ts);
        }
    }
    if let Some(pos) = line.find("DEADLINE:") {
        let rest = &line[pos + "DEADLINE:".len()..];
        let rest = rest.trim_start();
        if let Some((ts, _)) = parse_timestamp(rest, 0) {
            planning.deadline = Some(ts);
        }
    }
    if let Some(pos) = line.find("CLOSED:") {
        let rest = &line[pos + "CLOSED:".len()..];
        let rest = rest.trim_start();
        if let Some((ts, _)) = parse_timestamp(rest, 0) {
            planning.closed = Some(ts);
        }
    }

    planning
}

/// Parses a `:PROPERTIES:` drawer, returning (properties, next_line_index).
/// `start` is the line after `:PROPERTIES:`.
fn parse_property_drawer(lines: &[&str], start: usize) -> (HashMap<String, String>, usize) {
    let mut props = HashMap::new();
    let mut i = start;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.eq_ignore_ascii_case(":END:") {
            return (props, i + 1);
        }
        if let Some((key, val)) = parse_property_line(trimmed) {
            props.insert(key, val);
        }
        i += 1;
    }

    (props, i)
}

/// Parses a single `:KEY: value` property line.
fn parse_property_line(trimmed: &str) -> Option<(String, String)> {
    if !trimmed.starts_with(':') {
        return None;
    }
    let rest = &trimmed[1..];
    let colon_pos = rest.find(':')?;
    let key = rest[..colon_pos].to_string();
    if key.is_empty() {
        return None;
    }
    let val = rest[colon_pos + 1..].trim().to_string();
    Some((key, val))
}

/// Parses a `#+KEY: value` keyword line.
fn parse_keyword_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with("#+") {
        return None;
    }
    let rest = &trimmed[2..];
    let colon_pos = rest.find(':')?;
    let key = rest[..colon_pos].trim().to_uppercase();
    let val = rest[colon_pos + 1..].trim().to_string();
    Some((key, val))
}

/// Parse `#+FILETAGS:` value into a list of tags.
///
/// Format: `:tag1:tag2:tag3:` (colon-delimited, matching org heading tag syntax).
fn parse_filetags(value: Option<&String>) -> Vec<String> {
    let Some(val) = value else {
        return Vec::new();
    };
    val.trim()
        .trim_matches(':')
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Parse `#+PROPERTY:` keyword lines into a default property map.
///
/// The `file_keywords` HashMap stores only the last `#+PROPERTY:` line (since it's
/// keyed by `"PROPERTY"`). In org-mode, multiple `#+PROPERTY:` lines are supported.
/// For now, we handle the single-value case: `#+PROPERTY: KEY VALUE`.
fn parse_default_properties(file_keywords: &HashMap<String, String>) -> HashMap<String, String> {
    let mut props = HashMap::new();
    if let Some(val) = file_keywords.get("PROPERTY") {
        let val = val.trim();
        if let Some(space) = val.find(|c: char| c.is_whitespace()) {
            let key = val[..space].to_uppercase();
            let value = val[space..].trim().to_string();
            if !key.is_empty() {
                props.insert(key, value);
            }
        }
    }
    props
}

/// Finds the parent index for a heading at the given level.
fn find_parent(level_stack: &[(usize, usize)], level: usize) -> Option<usize> {
    level_stack
        .iter()
        .rev()
        .find(|&&(lvl, _)| lvl < level)
        .map(|&(_, idx)| idx)
}

/// Scans the body of an entry for LOGBOOK drawers and standalone CLOCK lines.
/// Returns the line index after the body (next heading or EOF).
fn scan_entry_body(
    lines: &[&str],
    start: usize,
    _heading_level: usize,
    entry: &mut OrgEntry,
) -> usize {
    let mut i = start;

    while i < lines.len() {
        let line = lines[i];

        // Stop at next heading of same or higher level.
        if parse_heading(line).is_some() {
            // Don't consume the next heading line.
            return i;
        }

        let trimmed = line.trim();

        // LOGBOOK drawer.
        if trimmed.eq_ignore_ascii_case(":LOGBOOK:") {
            i += 1;
            while i < lines.len() {
                let inner = lines[i].trim();
                if inner.eq_ignore_ascii_case(":END:") {
                    i += 1;
                    break;
                }
                if let Some(clock) = parse_clock_line(inner, i + 1) {
                    entry.clocks.push(clock);
                }
                i += 1;
            }
            continue;
        }

        // Standalone CLOCK line (outside LOGBOOK).
        if trimmed.starts_with("CLOCK:") {
            if let Some(clock) = parse_clock_line(trimmed, i + 1) {
                entry.clocks.push(clock);
            }
        }

        i += 1;
    }

    i
}

/// Parses a `CLOCK:` line into a [`ClockEntry`].
fn parse_clock_line(trimmed: &str, line_number: usize) -> Option<ClockEntry> {
    let rest = trimmed.strip_prefix("CLOCK:")?.trim_start();

    let (start_ts, after_start) = parse_timestamp(rest, 0)?;

    // Check for end timestamp: `--[end]`
    let remaining = &rest[after_start..];
    if let Some(dash_pos) = remaining.find("--") {
        let after_dash = &remaining[dash_pos + 2..];
        if let Some((end_ts, after_end)) = parse_timestamp(after_dash, 0) {
            // Parse duration: `=> HH:MM`
            let duration_str = &after_dash[after_end..];
            let duration = parse_duration(duration_str);

            return Some(ClockEntry {
                start: start_ts,
                end: Some(end_ts),
                duration_minutes: duration,
                line: line_number,
            });
        }
    }

    // Running clock (no end timestamp).
    Some(ClockEntry {
        start: start_ts,
        end: None,
        duration_minutes: None,
        line: line_number,
    })
}

/// Parses a duration string like ` =>  1:30` into minutes.
fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    let rest = s.strip_prefix("=>")?;
    let rest = rest.trim();
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hours: i64 = parts[0].trim().parse().ok()?;
    let minutes: i64 = parts[1].trim().parse().ok()?;
    Some(hours * 60 + minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_source(content: &str) -> SourceFile {
        SourceFile::new("test.org", content.to_string())
    }

    #[test]
    fn empty_document() {
        let source = make_source("");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.entries.is_empty());
    }

    #[test]
    fn single_heading() {
        let source = make_source("* Hello\nSome text.\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries.len(), 1);
        assert_eq!(doc.entries[0].level, 1);
        assert_eq!(doc.entries[0].title, "Hello");
        assert_eq!(doc.entries[0].heading_line, 1);
    }

    #[test]
    fn heading_with_todo_and_tags() {
        let source = make_source("* TODO [#A] Meeting notes :work:urgent:\n");
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert_eq!(e.keyword.as_deref(), Some("TODO"));
        assert_eq!(e.priority, Some('A'));
        assert_eq!(e.title, "Meeting notes");
        assert_eq!(e.tags, vec!["work", "urgent"]);
    }

    #[test]
    fn parent_child_relationships() {
        let source = make_source("* Parent\n** Child 1\n** Child 2\n*** Grandchild\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries.len(), 4);

        assert!(doc.entries[0].parent.is_none());
        assert_eq!(doc.entries[0].children, vec![1, 2]);

        assert_eq!(doc.entries[1].parent, Some(0));
        assert!(doc.entries[1].children.is_empty());

        assert_eq!(doc.entries[2].parent, Some(0));
        assert_eq!(doc.entries[2].children, vec![3]);

        assert_eq!(doc.entries[3].parent, Some(2));
    }

    #[test]
    fn outline_path() {
        let source = make_source("* A\n** B\n*** C\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.outline_path(2), vec!["A", "B", "C"]);
        assert_eq!(doc.outline_path(0), vec!["A"]);
    }

    #[test]
    fn planning_timestamps() {
        let source = make_source(
            "* TODO Task\nSCHEDULED: <2024-01-15 Mon 09:00> DEADLINE: <2024-02-01 Thu>\n",
        );
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert!(e.planning.scheduled.is_some());
        assert!(e.planning.deadline.is_some());
        assert_eq!(e.planning.scheduled.as_ref().unwrap().day, 15);
        assert_eq!(e.planning.deadline.as_ref().unwrap().day, 1);
    }

    #[test]
    fn property_drawer() {
        let source =
            make_source("* Heading\n:PROPERTIES:\n:ID: abc-123\n:CUSTOM_ID: my-heading\n:END:\n");
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert_eq!(e.properties.get("ID").unwrap(), "abc-123");
        assert_eq!(e.properties.get("CUSTOM_ID").unwrap(), "my-heading");
    }

    #[test]
    fn find_by_id() {
        let source = make_source(
            "* A\n:PROPERTIES:\n:ID: uuid-1\n:END:\n* B\n:PROPERTIES:\n:ID: uuid-2\n:END:\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.find_by_id("uuid-2"), Some(1));
        assert_eq!(doc.find_by_id("uuid-1"), Some(0));
        assert_eq!(doc.find_by_id("nonexistent"), None);
    }

    #[test]
    fn find_by_custom_id() {
        let source = make_source("* Heading\n:PROPERTIES:\n:CUSTOM_ID: my-section\n:END:\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.find_by_custom_id("my-section"), Some(0));
    }

    #[test]
    fn clock_entries_in_logbook() {
        let source = make_source(
            "* Task\n:LOGBOOK:\nCLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:30] =>  1:30\nCLOCK: [2024-01-14 Sun 14:00]--[2024-01-14 Sun 14:45] =>  0:45\n:END:\n",
        );
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert_eq!(e.clocks.len(), 2);
        assert_eq!(e.clocks[0].duration_minutes, Some(90));
        assert!(e.clocks[0].end.is_some());
        assert_eq!(e.clocks[1].duration_minutes, Some(45));
    }

    #[test]
    fn running_clock() {
        let source = make_source("* Task\n:LOGBOOK:\nCLOCK: [2024-01-15 Mon 09:00]\n:END:\n");
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert_eq!(e.clocks.len(), 1);
        assert!(e.clocks[0].end.is_none());
        assert_eq!(e.clocks[0].duration_minutes, None);
    }

    #[test]
    fn standalone_clock_outside_logbook() {
        let source =
            make_source("* Task\nCLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:00] =>  1:00\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].clocks.len(), 1);
    }

    #[test]
    fn file_level_properties() {
        let source =
            make_source(":PROPERTIES:\n:ID: file-id\n:END:\n#+TITLE: My Document\n* Heading\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.file_properties.get("ID").unwrap(), "file-id");
        assert_eq!(doc.file_keywords.get("TITLE").unwrap(), "My Document");
        assert_eq!(doc.entries.len(), 1);
    }

    #[test]
    fn file_keywords() {
        let source = make_source("#+TITLE: Test\n#+AUTHOR: User\n* Heading\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.file_keywords.get("TITLE").unwrap(), "Test");
        assert_eq!(doc.file_keywords.get("AUTHOR").unwrap(), "User");
    }

    #[test]
    fn content_end_lines() {
        let source = make_source("* A\ntext\n* B\nmore text\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].heading_line, 1);
        assert_eq!(doc.entries[0].content_end_line, 3); // Up to line 3 (exclusive)
        assert_eq!(doc.entries[1].heading_line, 3);
    }

    #[test]
    fn inherited_tags() {
        let source = make_source("* Parent :parent_tag:\n** Child :child_tag:\n");
        let doc = OrgDocument::from_source(&source);
        let tags = doc.inherited_tags(1);
        assert!(tags.contains(&"child_tag"));
        assert!(tags.contains(&"parent_tag"));
    }

    #[test]
    fn planning_after_heading_then_properties() {
        let source = make_source(
            "* TODO Task\nSCHEDULED: <2024-01-15 Mon>\n:PROPERTIES:\n:ID: t1\n:END:\nBody text\n",
        );
        let doc = OrgDocument::from_source(&source);
        let e = &doc.entries[0];
        assert!(e.planning.scheduled.is_some());
        assert_eq!(e.properties.get("ID").unwrap(), "t1");
    }

    #[test]
    fn sibling_headings_at_different_levels() {
        let source = make_source("* A\n** B\n* C\n** D\n");
        let doc = OrgDocument::from_source(&source);
        // A's children: [B]
        assert_eq!(doc.entries[0].children, vec![1]);
        // C's children: [D]
        assert_eq!(doc.entries[2].children, vec![3]);
        // B and D are not siblings of each other
        assert_eq!(doc.entries[1].parent, Some(0));
        assert_eq!(doc.entries[3].parent, Some(2));
    }

    #[test]
    fn parse_duration_values() {
        assert_eq!(parse_duration("=>  1:30"), Some(90));
        assert_eq!(parse_duration("=> 0:45"), Some(45));
        assert_eq!(parse_duration("=> 10:00"), Some(600));
        assert_eq!(parse_duration("not a duration"), None);
    }

    #[test]
    fn property_line_parsing() {
        assert_eq!(
            parse_property_line(":ID: abc"),
            Some(("ID".to_string(), "abc".to_string()))
        );
        assert_eq!(
            parse_property_line(":CUSTOM_ID: my-id"),
            Some(("CUSTOM_ID".to_string(), "my-id".to_string()))
        );
        assert_eq!(parse_property_line("not a property"), None);
        assert_eq!(parse_property_line("::"), None); // Empty key.
    }

    #[test]
    fn keyword_line_parsing() {
        assert_eq!(
            parse_keyword_line("#+TITLE: My Doc"),
            Some(("TITLE".to_string(), "My Doc".to_string()))
        );
        assert_eq!(
            parse_keyword_line("#+author: Me"),
            Some(("AUTHOR".to_string(), "Me".to_string()))
        );
        assert_eq!(parse_keyword_line("not a keyword"), None);
    }

    // --- Custom TODO keywords ---

    #[test]
    fn custom_todo_keywords_recognized() {
        let source = make_source("#+TODO: OPEN | CLOSED WONTFIX\n* OPEN A bug\n* CLOSED Fixed\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.todo_keywords.todo, vec!["OPEN"]);
        assert_eq!(doc.todo_keywords.done, vec!["CLOSED", "WONTFIX"]);
        assert_eq!(doc.entries[0].keyword, Some("OPEN".to_string()));
        assert_eq!(doc.entries[1].keyword, Some("CLOSED".to_string()));
    }

    #[test]
    fn custom_todo_default_not_recognized() {
        // With custom keywords, "TODO" should NOT be recognized as a keyword.
        let source = make_source("#+TODO: OPEN | CLOSED\n* TODO This is a title\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].keyword, None);
        assert_eq!(doc.entries[0].title, "TODO This is a title");
    }

    #[test]
    fn no_todo_setting_uses_defaults() {
        let source = make_source("* TODO Task\n* DONE Finished\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].keyword, Some("TODO".to_string()));
        assert_eq!(doc.entries[1].keyword, Some("DONE".to_string()));
    }

    #[test]
    fn seq_todo_keyword() {
        let source = make_source("#+SEQ_TODO: DRAFT REVIEW | PUBLISHED\n* REVIEW Article\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.entries[0].keyword, Some("REVIEW".to_string()));
        assert!(doc.todo_keywords.is_done("PUBLISHED"));
        assert!(!doc.todo_keywords.is_done("REVIEW"));
    }

    // --- Custom priorities ---

    #[test]
    fn custom_priority_range() {
        let source = make_source("#+PRIORITIES: A E B\n* [#D] Task\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.priority_range.highest, 'A');
        assert_eq!(doc.priority_range.lowest, 'E');
        assert_eq!(doc.priority_range.default, 'B');
        assert!(doc.priority_range.is_valid('D'));
    }

    #[test]
    fn default_priority_range() {
        let source = make_source("* [#B] Task\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.priority_range, PriorityRange::default());
    }

    // --- FILETAGS ---

    #[test]
    fn filetags_inherited() {
        let source = make_source("#+FILETAGS: :project:work:\n* Heading :local:\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.filetags, vec!["project", "work"]);
        let tags = doc.inherited_tags(0);
        assert!(tags.contains(&"local"));
        assert!(tags.contains(&"project"));
        assert!(tags.contains(&"work"));
    }

    #[test]
    fn filetags_empty() {
        let source = make_source("* Heading\n");
        let doc = OrgDocument::from_source(&source);
        assert!(doc.filetags.is_empty());
    }

    #[test]
    fn filetags_without_surrounding_colons() {
        // Some users write #+FILETAGS: tag1:tag2 without leading/trailing colons.
        let source = make_source("#+FILETAGS: tag1:tag2\n* H\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.filetags, vec!["tag1", "tag2"]);
    }

    // --- Default properties ---

    #[test]
    fn default_property_fallback() {
        let source = make_source("#+PROPERTY: CATEGORY work\n* Task\n");
        let doc = OrgDocument::from_source(&source);
        // Entry has no CATEGORY property, but the file default exists.
        assert_eq!(doc.property(0, "CATEGORY"), Some("work"));
    }

    #[test]
    fn entry_property_overrides_default() {
        let source = make_source(
            "#+PROPERTY: CATEGORY work\n* Task\n:PROPERTIES:\n:CATEGORY: personal\n:END:\n",
        );
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.property(0, "CATEGORY"), Some("personal"));
    }

    #[test]
    fn no_default_property() {
        let source = make_source("* Task\n");
        let doc = OrgDocument::from_source(&source);
        assert_eq!(doc.property(0, "CATEGORY"), None);
    }
}
