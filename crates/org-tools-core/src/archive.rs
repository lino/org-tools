// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Archive done org-mode entries to a separate file or heading.
//!
//! Spec: [§10.1 Archiving](https://orgmode.org/manual/Archiving.html)
//!
//! Entries with done TODO keywords are moved to an archive location, preserving
//! their tree structure and adding metadata properties (`:ARCHIVE_TIME:`,
//! `:ARCHIVE_FILE:`, `:ARCHIVE_OLPATH:`).

use std::path::{Path, PathBuf};

use crate::document::OrgDocument;
use crate::source::SourceFile;

/// Parsed archive target from `#+ARCHIVE:` or `--target`.
#[derive(Debug, Clone)]
pub struct ArchiveTarget {
    /// Archive file path (None = same file).
    pub file: Option<PathBuf>,
    /// Target heading in the archive file (e.g., "* Archived").
    pub heading: Option<String>,
}

/// An entry to be archived with its extracted content.
#[derive(Debug)]
pub struct ArchiveEntry {
    /// Entry title for display.
    pub title: String,
    /// The full text of the entry subtree (heading + body + children).
    pub content: String,
    /// Original file path.
    pub source_file: String,
    /// Original outline path.
    pub outline_path: String,
    /// Line range in source (start_line, end_line) for removal.
    pub start_line: usize,
    pub end_line: usize,
}

/// Result of an archive operation.
pub struct ArchiveResult {
    /// Modified source content (entries removed).
    pub source_content: String,
    /// Content to append to the archive file.
    pub archive_content: String,
    /// Path to the archive file.
    pub archive_path: PathBuf,
    /// Number of entries archived.
    pub archived: usize,
}

/// Parse an archive target string (e.g., `filename.org::* Archived`).
pub fn parse_archive_target(target: &str, source_path: &Path) -> ArchiveTarget {
    if let Some(pos) = target.find("::") {
        let file_part = target[..pos].trim();
        let heading_part = target[pos + 2..].trim();
        ArchiveTarget {
            file: if file_part.is_empty() {
                None
            } else {
                let base = source_path.parent().unwrap_or(Path::new("."));
                Some(base.join(file_part))
            },
            heading: if heading_part.is_empty() {
                None
            } else {
                Some(heading_part.to_string())
            },
        }
    } else if target.trim().is_empty() {
        // Default: {filename}_archive.org
        let stem = source_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let base = source_path.parent().unwrap_or(Path::new("."));
        ArchiveTarget {
            file: Some(base.join(format!("{stem}_archive.org"))),
            heading: Some("* Archived".to_string()),
        }
    } else {
        // Just a filename.
        let base = source_path.parent().unwrap_or(Path::new("."));
        ArchiveTarget {
            file: Some(base.join(target.trim())),
            heading: None,
        }
    }
}

/// Find done entries and prepare them for archiving.
pub fn find_archivable_entries(
    source: &SourceFile,
    doc: &OrgDocument,
    filter_tags: &[String],
) -> Vec<ArchiveEntry> {
    let lines: Vec<&str> = source.content.split('\n').collect();
    let mut entries = Vec::new();
    let mut skip_children_of: Option<usize> = None;

    for (idx, entry) in doc.entries.iter().enumerate() {
        // Skip children of already-archived entries (they move with the parent).
        if let Some(parent) = skip_children_of {
            if is_descendant(doc, idx, parent) {
                continue;
            }
            skip_children_of = None;
        }

        // Only archive done entries.
        let is_done = entry
            .keyword
            .as_ref()
            .is_some_and(|kw| doc.todo_keywords.is_done(kw));
        if !is_done {
            continue;
        }

        // Tag filter.
        if !filter_tags.is_empty() {
            let inherited = doc.inherited_tags(idx);
            if !filter_tags
                .iter()
                .all(|ft| inherited.iter().any(|t| t.eq_ignore_ascii_case(ft)))
            {
                continue;
            }
        }

        // Extract the full subtree content.
        let start_line = entry.heading_line - 1; // 0-based
        let end_line = entry.content_end_line - 1; // 0-based, exclusive
        let outline_path = doc.outline_path(idx).join("/");

        // Add archive metadata as properties.
        let now = crate::state::now_timestamp();
        let mut archived_content = String::new();

        // Re-emit the heading at level 1 (flattened for archive).
        let heading_line = lines[start_line];
        archived_content.push_str(heading_line);
        archived_content.push('\n');

        // Check if entry has a property drawer.
        let has_drawer = start_line + 1 < end_line
            && lines
                .get(start_line + 1)
                .or_else(|| {
                    // Skip planning line.
                    let next = start_line + 1;
                    if next < lines.len() && is_planning(lines[next]) {
                        lines.get(next + 1)
                    } else {
                        None
                    }
                })
                .is_some_and(|l| l.trim().eq_ignore_ascii_case(":PROPERTIES:"));

        // If no planning line after heading, check for properties directly.
        let mut body_start = start_line + 1;

        // Copy planning line if present.
        if body_start < end_line && is_planning(lines[body_start]) {
            archived_content.push_str(lines[body_start]);
            archived_content.push('\n');
            body_start += 1;
        }

        if has_drawer
            || (body_start < end_line
                && lines[body_start]
                    .trim()
                    .eq_ignore_ascii_case(":PROPERTIES:"))
        {
            // Insert archive properties into existing drawer.
            archived_content.push_str(lines[body_start].trim_end()); // :PROPERTIES:
            archived_content.push('\n');
            archived_content.push_str(&format!(":ARCHIVE_TIME: {now}\n"));
            archived_content.push_str(&format!(":ARCHIVE_FILE: {}\n", doc.file.display()));
            archived_content.push_str(&format!(":ARCHIVE_OLPATH: {outline_path}\n"));
            body_start += 1;
            // Copy rest of drawer and body.
            for line in &lines[body_start..end_line] {
                archived_content.push_str(line);
                archived_content.push('\n');
            }
        } else {
            // Create new property drawer with archive metadata.
            archived_content.push_str(":PROPERTIES:\n");
            archived_content.push_str(&format!(":ARCHIVE_TIME: {now}\n"));
            archived_content.push_str(&format!(":ARCHIVE_FILE: {}\n", doc.file.display()));
            archived_content.push_str(&format!(":ARCHIVE_OLPATH: {outline_path}\n"));
            archived_content.push_str(":END:\n");
            // Copy body content.
            for line in &lines[body_start..end_line] {
                archived_content.push_str(line);
                archived_content.push('\n');
            }
        }

        entries.push(ArchiveEntry {
            title: entry.title.clone(),
            content: archived_content,
            source_file: doc.file.display().to_string(),
            outline_path,
            start_line: entry.heading_line,
            end_line: entry.content_end_line,
        });

        // Skip children of this entry.
        skip_children_of = Some(idx);
    }

    entries
}

/// Build the archive result: remove entries from source, build archive content.
pub fn build_archive(
    source: &SourceFile,
    doc: &OrgDocument,
    archive_entries: &[ArchiveEntry],
    target: &ArchiveTarget,
) -> Option<ArchiveResult> {
    if archive_entries.is_empty() {
        return None;
    }

    let lines: Vec<&str> = source.content.split('\n').collect();

    // Remove archived entries from source (from bottom to top to preserve line numbers).
    let mut source_lines: Vec<&str> = lines.clone();
    for entry in archive_entries.iter().rev() {
        let start = entry.start_line - 1; // 0-based
        let end = entry.end_line - 1; // 0-based, exclusive
        source_lines.drain(start..end);
    }
    let source_content = source_lines.join("\n");

    // Build archive content.
    let mut archive_content = String::new();
    if let Some(heading) = &target.heading {
        archive_content.push_str(heading);
        archive_content.push('\n');
    }
    for entry in archive_entries {
        archive_content.push('\n');
        archive_content.push_str(&entry.content);
    }

    let archive_path = target.file.clone().unwrap_or_else(|| doc.file.clone());

    Some(ArchiveResult {
        source_content,
        archive_content,
        archive_path,
        archived: archive_entries.len(),
    })
}

fn is_descendant(doc: &OrgDocument, idx: usize, ancestor: usize) -> bool {
    let mut current = doc.entries[idx].parent;
    while let Some(p) = current {
        if p == ancestor {
            return true;
        }
        current = doc.entries[p].parent;
    }
    false
}

fn is_planning(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

// Re-export now_timestamp from state module (avoid duplication).
pub use crate::state::now_timestamp;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc(content: &str) -> (SourceFile, OrgDocument) {
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);
        (source, doc)
    }

    #[test]
    fn parse_target_with_file_and_heading() {
        let target = parse_archive_target("archive.org::* Archived", Path::new("test.org"));
        assert_eq!(target.file.unwrap().file_name().unwrap(), "archive.org");
        assert_eq!(target.heading.unwrap(), "* Archived");
    }

    #[test]
    fn parse_target_same_file() {
        let target = parse_archive_target("::* Archive", Path::new("test.org"));
        assert!(target.file.is_none());
        assert_eq!(target.heading.unwrap(), "* Archive");
    }

    #[test]
    fn parse_target_default() {
        let target = parse_archive_target("", Path::new("/tmp/notes.org"));
        assert!(target.file.unwrap().ends_with("notes_archive.org"));
        assert_eq!(target.heading.unwrap(), "* Archived");
    }

    #[test]
    fn finds_done_entries() {
        let (source, doc) = make_doc("* TODO Open\n* DONE Finished\nSome body.\n* TODO Another\n");
        let entries = find_archivable_entries(&source, &doc, &[]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Finished");
        assert!(entries[0].content.contains("ARCHIVE_TIME"));
    }

    #[test]
    fn skips_open_entries() {
        let (source, doc) = make_doc("* TODO Open\n");
        let entries = find_archivable_entries(&source, &doc, &[]);
        assert!(entries.is_empty());
    }

    #[test]
    fn archives_with_children() {
        let (source, doc) =
            make_doc("* DONE Parent\n** DONE Child\n*** DONE Grandchild\n* TODO Other\n");
        let entries = find_archivable_entries(&source, &doc, &[]);
        // Parent is done, so the whole subtree is archived as one unit.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Parent");
    }

    #[test]
    fn build_archive_removes_from_source() {
        let (source, doc) = make_doc("* TODO Keep\n* DONE Remove\nBody.\n* TODO Also keep\n");
        let entries = find_archivable_entries(&source, &doc, &[]);
        let target = parse_archive_target("archive.org::* Archived", Path::new("test.org"));
        let result = build_archive(&source, &doc, &entries, &target).unwrap();

        assert!(result.source_content.contains("* TODO Keep"));
        assert!(result.source_content.contains("* TODO Also keep"));
        assert!(!result.source_content.contains("* DONE Remove"));
        assert!(result.archive_content.contains("* DONE Remove"));
        assert_eq!(result.archived, 1);
    }
}
