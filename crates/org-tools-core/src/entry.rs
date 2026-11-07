// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Add new TODO entries and parse plain-list checklist items in org-mode files.
//!
//! Inserts a heading with optional planning line (`SCHEDULED:` / `DEADLINE:`),
//! either at the end of the file or as a child of a specified parent entry, and
//! provides serializable plain-list checkbox types (`[ ]`, `[-]`, `[X]`).
//!
//! Spec: [§5.6 Checkboxes](https://orgmode.org/manual/Checkboxes.html),
//! [Syntax: Plain Lists and Items](https://orgmode.org/worg/org-syntax.html#Items)

use serde::{Deserialize, Serialize};

use crate::document::{OrgDocument, OrgEntry};
use crate::source::SourceFile;

// region Public Types

/// Completion state of an org-mode plain-list checkbox (`[ ]`, `[-]`, or `[X]`).
///
/// Spec: [§5.6 Checkboxes](https://orgmode.org/manual/Checkboxes.html),
/// [Syntax: Plain Lists and Items](https://orgmode.org/worg/org-syntax.html#Items)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckboxState {
    /// Unchecked item (`[ ]`).
    #[serde(alias = "Unchecked")]
    Unchecked,
    /// Partially completed item (`[-]`).
    #[serde(alias = "Partial")]
    Partial,
    /// Checked item (`[X]` or `[x]`).
    #[serde(alias = "Checked")]
    Checked,
}

/// A single plain-list checklist item belonging to an org entry's body.
///
/// Spec: [§5.6 Checkboxes](https://orgmode.org/manual/Checkboxes.html),
/// [Syntax: Plain Lists and Items](https://orgmode.org/worg/org-syntax.html#Items)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    /// 1-based line number of the plain-list item in the source document.
    pub line: usize,
    /// Completion state of the checkbox.
    pub state: CheckboxState,
    /// Trimmed item text following the checkbox marker.
    pub text: String,
}

/// Aggregated completion counts across an entry's plain-list [`ChecklistItem`]s.
///
/// Spec: [§5.6 Checkboxes](https://orgmode.org/manual/Checkboxes.html)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistSummary {
    /// Total number of checklist items on the entry.
    pub total: usize,
    /// Number of checked (`[X]` / `[x]`) items.
    pub checked: usize,
    /// Number of partially completed (`[-]`) items.
    pub partial: usize,
    /// Number of unchecked (`[ ]`) items.
    pub unchecked: usize,
}

impl ChecklistSummary {
    /// Computes aggregate completion counts from a slice of [`ChecklistItem`]s.
    pub fn from_items(items: &[ChecklistItem]) -> Self {
        let mut summary = Self {
            total: items.len(),
            ..Self::default()
        };
        for item in items {
            match item.state {
                CheckboxState::Checked => summary.checked += 1,
                CheckboxState::Partial => summary.partial += 1,
                CheckboxState::Unchecked => summary.unchecked += 1,
            }
        }
        summary
    }
}

/// Options for creating a new entry.
pub struct NewEntryOpts {
    /// Heading title text.
    pub title: String,
    /// Heading level (number of stars).
    pub level: usize,
    /// TODO keyword (e.g., `"TODO"`).
    pub keyword: Option<String>,
    /// Priority letter (e.g., `'A'`).
    pub priority: Option<char>,
    /// Tags to add.
    pub tags: Vec<String>,
    /// `SCHEDULED` date in `YYYY-MM-DD` format.
    pub scheduled: Option<String>,
    /// `DEADLINE` date in `YYYY-MM-DD` format.
    pub deadline: Option<String>,
}

/// Result of adding a new entry.
pub struct AddEntryResult {
    /// The modified file content.
    pub content: String,
}

// endregion

// region Public API

/// Parses a plain-list line (`-`, `+`, indented `*`, or numbered `1.` / `1)`) with a checkbox.
///
/// Returns `Some(ChecklistItem)` when `line` is a plain-list item carrying `[ ]`,
/// `[-]`, `[X]`, or `[x]`, or `None` otherwise. Unindented `*` lines are treated
/// as headings rather than list items per the Org syntax specification.
///
/// Spec: [§5.6 Checkboxes](https://orgmode.org/manual/Checkboxes.html),
/// [Syntax: Plain Lists and Items](https://orgmode.org/worg/org-syntax.html#Items)
pub fn parse_checklist_item(line: &str, line_number: usize) -> Option<ChecklistItem> {
    let raw = line.strip_suffix('\r').unwrap_or(line);
    let after_bullet = strip_plain_list_bullet(raw)?;
    let after_counter = strip_optional_counter_set(after_bullet);
    let (state, text) = parse_checkbox_marker(after_counter)?;
    Some(ChecklistItem {
        line: line_number,
        state,
        text: text.to_string(),
    })
}

/// Add a new entry to the file.
///
/// When `parent_idx` resolves to an existing entry in `doc.entries`, the new entry
/// is inserted as the last child of that parent. When `None` (or when `parent_idx`
/// refers to a level-0 file node sentinel), it is appended to the end of the file.
pub fn add_entry(
    source: &SourceFile,
    doc: &OrgDocument,
    parent_idx: Option<usize>,
    opts: &NewEntryOpts,
) -> AddEntryResult {
    let block = format_entry_block(opts);
    let mut content = source.content.clone();

    match parent_idx.and_then(|pidx| doc.entries.get(pidx)) {
        Some(parent) => insert_after_parent(&mut content, parent, &block),
        None => append_to_document(&mut content, &block),
    }

    AddEntryResult { content }
}

// endregion

// region Internal Helpers

/// Strips a leading plain-list bullet (`-`, `+`, indented `*`, or `N.` / `N)`) and post-bullet whitespace.
fn strip_plain_list_bullet(raw: &str) -> Option<&str> {
    let trimmed = raw.trim_start_matches([' ', '\t']);
    let is_indented = trimmed.len() < raw.len();

    if let Some(rest) = trimmed.strip_prefix(['-', '+']) {
        return strip_required_horizontal_ws(rest);
    }
    if is_indented {
        if let Some(rest) = trimmed.strip_prefix('*') {
            return strip_required_horizontal_ws(rest);
        }
    }

    let ordered_len = if trimmed.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        trimmed.bytes().take_while(u8::is_ascii_digit).count()
    } else if trimmed
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
    {
        1
    } else {
        0
    };
    if ordered_len > 0 {
        if let Some(rest) = trimmed[ordered_len..].strip_prefix(['.', ')']) {
            return strip_required_horizontal_ws(rest);
        }
    }

    None
}

/// Requires at least one space or tab at the start of `s` and strips all leading spaces/tabs.
fn strip_required_horizontal_ws(s: &str) -> Option<&str> {
    if s.starts_with([' ', '\t']) {
        Some(s.trim_start_matches([' ', '\t']))
    } else {
        None
    }
}

/// Skips an optional ordered-list counter cookie (`[@20]`) preceding a checkbox.
fn strip_optional_counter_set(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix("[@") {
        if let Some(close_idx) = rest.find(']') {
            let after = &rest[close_idx + 1..];
            if after.is_empty() || after.starts_with([' ', '\t']) {
                return after.trim_start_matches([' ', '\t']);
            }
        }
    }
    s
}

/// Parses a `[ ]`, `[-]`, `[X]`, or `[x]` marker at the start of `s`.
fn parse_checkbox_marker(s: &str) -> Option<(CheckboxState, &str)> {
    let marker = s.get(..3)?;
    let state = match marker {
        "[ ]" => CheckboxState::Unchecked,
        "[-]" => CheckboxState::Partial,
        "[X]" | "[x]" => CheckboxState::Checked,
        _ => return None,
    };
    let after = &s[3..];
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    Some((state, after.trim()))
}

/// Formats a heading line and optional planning line from `opts`.
fn format_entry_block(opts: &NewEntryOpts) -> String {
    let mut heading = format!("{} ", "*".repeat(opts.level));

    if let Some(kw) = &opts.keyword {
        heading.push_str(kw);
        heading.push(' ');
    }
    if let Some(pri) = opts.priority {
        heading.push_str(&format!("[#{pri}] "));
    }
    heading.push_str(&opts.title);
    if !opts.tags.is_empty() {
        heading.push_str(&format!(" :{}:", opts.tags.join(":")));
    }

    let mut planning_parts = Vec::new();
    if let Some(date) = &opts.scheduled {
        planning_parts.push(format!("SCHEDULED: <{date}>"));
    }
    if let Some(date) = &opts.deadline {
        planning_parts.push(format!("DEADLINE: <{date}>"));
    }
    if !planning_parts.is_empty() {
        heading.push('\n');
        heading.push_str(&planning_parts.join(" "));
    }

    heading
}

/// Inserts `block` at the end of `parent`'s content section.
fn insert_after_parent(content: &mut String, parent: &OrgEntry, block: &str) {
    let insert_line = parent.content_end_line - 1;
    let lines: Vec<&str> = content.split('\n').collect();
    let clamped_line = insert_line.min(lines.len());
    let insert_offset: usize = lines[..clamped_line].iter().map(|l| l.len() + 1).sum();
    let clamped_offset = insert_offset.min(content.len());
    let needs_blank = clamped_offset > 0 && !content[..clamped_offset].ends_with("\n\n");

    let mut insertion = String::new();
    if needs_blank {
        insertion.push('\n');
    }
    insertion.push_str(block);
    insertion.push('\n');

    content.insert_str(clamped_offset, &insertion);
}

/// Appends `block` at the end of `content`, ensuring a separating blank line.
fn append_to_document(content: &mut String, block: &str) {
    if !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.ends_with("\n\n") {
        content.push('\n');
    }
    content.push_str(block);
    content.push('\n');
}

// endregion

// region Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a `(SourceFile, OrgDocument)` pair from inline org content.
    fn make_doc(content: &str) -> (SourceFile, OrgDocument) {
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);
        (source, doc)
    }

    /// Verifies adding a top-level TODO entry to an empty document.
    #[test]
    fn add_entry_to_empty_file() {
        let source = SourceFile::new("test.org", "".to_string());
        let doc = OrgDocument::from_source(&source);
        let result = add_entry(
            &source,
            &doc,
            None,
            &NewEntryOpts {
                title: "New task".to_string(),
                level: 1,
                keyword: Some("TODO".to_string()),
                priority: None,
                tags: vec![],
                scheduled: None,
                deadline: None,
            },
        );
        assert!(result.content.contains("* TODO New task"));
    }

    /// Verifies appending a new entry after an existing top-level heading.
    #[test]
    fn add_entry_at_end() {
        let (source, doc) = make_doc("* Existing\n");
        let result = add_entry(
            &source,
            &doc,
            None,
            &NewEntryOpts {
                title: "Second".to_string(),
                level: 1,
                keyword: Some("TODO".to_string()),
                priority: None,
                tags: vec![],
                scheduled: None,
                deadline: None,
            },
        );
        assert!(result.content.contains("* Existing\n"));
        assert!(result.content.contains("* TODO Second\n"));
    }

    /// Verifies formatting priority, tags, and `SCHEDULED:` planning metadata on a new entry.
    #[test]
    fn add_entry_with_planning() {
        let (source, doc) = make_doc("* Existing\n");
        let result = add_entry(
            &source,
            &doc,
            None,
            &NewEntryOpts {
                title: "Meeting".to_string(),
                level: 1,
                keyword: Some("TODO".to_string()),
                priority: Some('A'),
                tags: vec!["work".to_string()],
                scheduled: Some("2024-06-15".to_string()),
                deadline: None,
            },
        );
        assert!(result.content.contains("* TODO [#A] Meeting :work:"));
        assert!(result.content.contains("SCHEDULED: <2024-06-15>"));
    }

    /// Verifies inserting a child TODO entry beneath an existing parent heading.
    #[test]
    fn add_entry_as_child() {
        let (source, doc) = make_doc("* Parent\nSome text.\n");
        let result = add_entry(
            &source,
            &doc,
            Some(0),
            &NewEntryOpts {
                title: "Child".to_string(),
                level: 2,
                keyword: Some("TODO".to_string()),
                priority: None,
                tags: vec![],
                scheduled: None,
                deadline: None,
            },
        );
        assert!(result.content.contains("* Parent\n"));
        assert!(result.content.contains("** TODO Child\n"));
    }

    /// Verifies that `CheckboxState`, `ChecklistItem`, and `ChecklistSummary` round-trip through JSON.
    #[test]
    fn checklist_types_roundtrip_serde_json() {
        let item = ChecklistItem {
            line: 4,
            state: CheckboxState::Partial,
            text: "Review edge cases".to_string(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let decoded: ChecklistItem = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, item);

        let summary = ChecklistSummary {
            total: 3,
            checked: 1,
            partial: 1,
            unchecked: 1,
        };
        let summary_json = serde_json::to_string(&summary).unwrap();
        let decoded_summary: ChecklistSummary = serde_json::from_str(&summary_json).unwrap();
        assert_eq!(decoded_summary, summary);
    }

    /// Verifies that passing a file-node sentinel parent index appends safely without panicking.
    #[test]
    fn add_entry_with_file_node_sentinel_appends_safely() {
        let (source, doc) = make_doc(":PROPERTIES:\n:ID: roam-file\n:END:\n#+TITLE: Note\n");
        let result = add_entry(
            &source,
            &doc,
            Some(crate::locator::FILE_NODE_ENTRY_INDEX),
            &NewEntryOpts {
                title: "Top-level under file node".to_string(),
                level: 1,
                keyword: Some("TODO".to_string()),
                priority: None,
                tags: vec![],
                scheduled: None,
                deadline: None,
            },
        );
        assert!(result
            .content
            .contains("* TODO Top-level under file node\n"));
    }
}

// endregion
