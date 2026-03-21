// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Add new TODO entries to org-mode files.
//!
//! Inserts a heading with optional planning line (SCHEDULED/DEADLINE), either
//! at the end of the file or as a child of a specified parent entry.

use crate::document::OrgDocument;
use crate::source::SourceFile;

/// Options for creating a new entry.
pub struct NewEntryOpts {
    /// Heading title text.
    pub title: String,
    /// Heading level (number of stars).
    pub level: usize,
    /// TODO keyword (e.g., "TODO").
    pub keyword: Option<String>,
    /// Priority letter (e.g., 'A').
    pub priority: Option<char>,
    /// Tags to add.
    pub tags: Vec<String>,
    /// SCHEDULED date in YYYY-MM-DD format.
    pub scheduled: Option<String>,
    /// DEADLINE date in YYYY-MM-DD format.
    pub deadline: Option<String>,
}

/// Result of adding a new entry.
pub struct AddEntryResult {
    /// The modified file content.
    pub content: String,
}

/// Add a new entry to the file.
///
/// When `parent_idx` is `Some`, the entry is inserted as the last child of
/// the parent. When `None`, it is appended to the end of the file.
pub fn add_entry(
    source: &SourceFile,
    doc: &OrgDocument,
    parent_idx: Option<usize>,
    opts: &NewEntryOpts,
) -> AddEntryResult {
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

    let mut block = heading;

    // Planning line.
    let mut planning_parts = Vec::new();
    if let Some(date) = &opts.scheduled {
        planning_parts.push(format!("SCHEDULED: <{date}>"));
    }
    if let Some(date) = &opts.deadline {
        planning_parts.push(format!("DEADLINE: <{date}>"));
    }
    if !planning_parts.is_empty() {
        block.push('\n');
        block.push_str(&planning_parts.join(" "));
    }

    let mut content = source.content.clone();

    match parent_idx {
        Some(pidx) => {
            // Insert after the parent's last content line (before next sibling or EOF).
            let entry = &doc.entries[pidx];
            let insert_line = entry.content_end_line - 1; // content_end_line is 1-based exclusive
            let lines: Vec<&str> = content.split('\n').collect();
            let insert_line = insert_line.min(lines.len());
            let insert_offset: usize = lines[..insert_line].iter().map(|l| l.len() + 1).sum();
            let insert_offset = insert_offset.min(content.len());

            // Ensure blank line before new heading.
            let needs_blank = insert_offset > 0 && !content[..insert_offset].ends_with("\n\n");

            let mut insertion = String::new();
            if needs_blank {
                insertion.push('\n');
            }
            insertion.push_str(&block);
            insertion.push('\n');

            content.insert_str(insert_offset, &insertion);
        }
        None => {
            // Append to end of file.
            if !content.ends_with('\n') {
                content.push('\n');
            }
            if !content.ends_with("\n\n") {
                content.push('\n');
            }
            content.push_str(&block);
            content.push('\n');
        }
    }

    AddEntryResult { content }
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
}
