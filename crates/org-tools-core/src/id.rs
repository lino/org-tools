// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Add `:ID:` properties to org-mode entries that lack them.
//!
//! Supports three ID generation strategies: UUID v4 (default), template strings
//! with placeholders, and external commands that receive entry metadata as JSON.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::diagnostic::{Fix, Span};
use crate::document::{OrgDocument, OrgEntry};
use crate::formatter::apply_fixes;
use crate::source::SourceFile;

/// Metadata about an entry, passed to ID generators.
#[derive(Debug)]
pub struct EntryInfo<'a> {
    /// Path to the org file.
    pub file: &'a Path,
    /// 1-based line number of the heading.
    pub line: usize,
    /// Heading level (1 = top-level `*`).
    pub level: usize,
    /// Heading title text.
    pub title: &'a str,
    /// TODO keyword if present.
    pub keyword: Option<&'a str>,
    /// Tags on the heading.
    pub tags: &'a [String],
    /// Properties from the `:PROPERTIES:` drawer.
    pub properties: &'a HashMap<String, String>,
    /// 0-based index of this entry within the document.
    pub index: usize,
}

/// Strategy for generating ID values.
pub enum IdGenerator {
    /// Default: UUID v4.
    Uuid,
    /// Template string with `{placeholder}` expansion.
    Template(String),
    /// External command: receives JSON on stdin, outputs ID on stdout.
    Command(String),
}

impl IdGenerator {
    /// Generate an ID for the given entry.
    pub fn generate(&self, info: &EntryInfo) -> Result<String, String> {
        match self {
            Self::Uuid => Ok(uuid::Uuid::new_v4().to_string()),
            Self::Template(tpl) => Ok(expand_template(tpl, info)),
            Self::Command(cmd) => run_command(cmd, info),
        }
    }
}

/// Result of adding IDs to a file.
pub struct AddIdResult {
    /// The modified file content.
    pub content: String,
    /// Number of IDs that were inserted.
    pub ids_added: usize,
}

/// Add `:ID:` properties to entries that lack them.
///
/// When `entry_indices` is `None`, all entries in the document are processed.
/// When `Some`, only the specified entries (by index into `doc.entries`) are
/// processed.
///
/// Returns `None` if no entries needed an ID (all already have one or no
/// entries matched).
pub fn add_ids(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_indices: Option<&[usize]>,
    generator: &IdGenerator,
) -> Result<Option<AddIdResult>, String> {
    let lines: Vec<&str> = source.content.split('\n').collect();
    let mut fixes: Vec<Fix> = Vec::new();

    let indices: Vec<usize> = match entry_indices {
        Some(idx) => idx.to_vec(),
        None => (0..doc.entries.len()).collect(),
    };

    for &idx in &indices {
        let entry = &doc.entries[idx];

        // Skip entries that already have an ID.
        if entry.properties.contains_key("ID") {
            continue;
        }

        let info = EntryInfo {
            file: &doc.file,
            line: entry.heading_line,
            level: entry.level,
            title: &entry.title,
            keyword: entry.keyword.as_deref(),
            tags: &entry.tags,
            properties: &entry.properties,
            index: idx,
        };

        let id_value = generator.generate(&info)?;
        if id_value.is_empty() {
            continue;
        }

        if let Some(fix) = build_id_fix(entry, &lines, &source.content, &id_value) {
            fixes.push(fix);
        }
    }

    if fixes.is_empty() {
        return Ok(None);
    }

    let ids_added = fixes.len();
    // Fixes must be sorted by span.start for apply_fixes.
    fixes.sort_by_key(|f| f.span.start);

    let content = apply_fixes(&source.content, &fixes);
    Ok(Some(AddIdResult { content, ids_added }))
}

/// Collect all descendant entry indices (depth-first) for a given entry.
pub fn collect_subtree(doc: &OrgDocument, entry_idx: usize) -> Vec<usize> {
    let mut result = vec![entry_idx];
    let mut stack = doc.entries[entry_idx].children.clone();
    while let Some(child) = stack.pop() {
        result.push(child);
        // Push children in reverse so we visit them in order.
        for &grandchild in doc.entries[child].children.iter().rev() {
            stack.push(grandchild);
        }
    }
    result
}

/// Build a `Fix` that inserts `:ID: <value>` for an entry.
///
/// Handles two cases:
/// 1. Entry already has a `:PROPERTIES:` drawer → insert `:ID:` as the first property.
/// 2. Entry has no drawer → insert a full `:PROPERTIES:` / `:END:` block.
fn build_id_fix(entry: &OrgEntry, lines: &[&str], content: &str, id_value: &str) -> Option<Fix> {
    // heading_line is 1-based, convert to 0-based index.
    let heading_idx = entry.heading_line - 1;

    // Find the line after heading (+ optional planning line).
    let mut insert_line = heading_idx + 1;
    if insert_line < lines.len() && is_planning_line(lines[insert_line]) {
        insert_line += 1;
    }

    if insert_line >= lines.len() {
        // Entry is at the end of file with nothing after it. Insert at end of content.
        let insert_offset = content.len();
        let needs_newline = !content.ends_with('\n');
        let prefix = if needs_newline { "\n" } else { "" };
        let replacement = format!("{prefix}:PROPERTIES:\n:ID: {id_value}\n:END:\n");
        return Some(Fix::new(
            Span::new(insert_offset, insert_offset),
            replacement,
        ));
    }

    let next_line = lines[insert_line].trim();

    if next_line.eq_ignore_ascii_case(":PROPERTIES:") {
        // Drawer exists — insert :ID: as the first line inside it.
        // The insertion point is the start of the line after :PROPERTIES:.
        let after_props_line = insert_line + 1;
        let insert_offset = byte_offset_of_line(lines, after_props_line);
        let replacement = format!(":ID: {id_value}\n");
        Some(Fix::new(
            Span::new(insert_offset, insert_offset),
            replacement,
        ))
    } else {
        // No drawer — insert a full :PROPERTIES: block.
        let insert_offset = byte_offset_of_line(lines, insert_line);
        let replacement = format!(":PROPERTIES:\n:ID: {id_value}\n:END:\n");
        Some(Fix::new(
            Span::new(insert_offset, insert_offset),
            replacement,
        ))
    }
}

/// Compute the byte offset of the start of a 0-based line.
fn byte_offset_of_line(lines: &[&str], line_idx: usize) -> usize {
    // Each line's byte length + 1 for the \n separator.
    lines[..line_idx].iter().map(|l| l.len() + 1).sum()
}

fn is_planning_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

/// Expand a template string, replacing `{placeholder}` tokens with entry values.
fn expand_template(template: &str, info: &EntryInfo) -> String {
    let uuid = uuid::Uuid::new_v4().to_string();
    let uuid_short = &uuid[..8];
    let file_stem = info
        .file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    template
        .replace("{uuid}", &uuid)
        .replace("{uuid_short}", uuid_short)
        .replace("{file_stem}", file_stem)
        .replace("{title_slug}", &slugify(info.title))
        .replace("{level}", &info.level.to_string())
        .replace("{index}", &info.index.to_string())
        .replace("{ts}", &ts.to_string())
}

/// Convert a heading title to a URL-safe slug.
///
/// Lowercase, replace non-alphanumeric with hyphens, collapse runs, trim edges.
pub fn slugify(s: &str) -> String {
    let mut slug = String::with_capacity(s.len());
    let mut prev_hyphen = true; // suppress leading hyphen
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            prev_hyphen = false;
        } else if !prev_hyphen {
            slug.push('-');
            prev_hyphen = true;
        }
    }
    // Trim trailing hyphen.
    if slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// Run an external command with entry metadata as JSON on stdin.
fn run_command(cmd: &str, info: &EntryInfo) -> Result<String, String> {
    let json = serde_json::json!({
        "file": info.file.display().to_string(),
        "line": info.line,
        "level": info.level,
        "title": info.title,
        "keyword": info.keyword,
        "tags": info.tags,
        "properties": info.properties,
        "index": info.index,
    });

    let mut child = Command::new("sh")
        .args(["-c", cmd])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run id-command: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(json.to_string().as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("id-command failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "id-command exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- slugify ---

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn slugify_special_chars() {
        assert_eq!(slugify("Foo: Bar & Baz!"), "foo-bar-baz");
    }

    #[test]
    fn slugify_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn slugify_leading_trailing_special() {
        assert_eq!(slugify("--Hello--"), "hello");
    }

    #[test]
    fn slugify_consecutive_specials() {
        assert_eq!(slugify("a   b   c"), "a-b-c");
    }

    // --- add_ids: no properties drawer ---

    #[test]
    fn add_id_no_drawer() {
        let content = "* Heading\nSome body text\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        let result = add_ids(&source, &doc, None, &IdGenerator::Uuid)
            .unwrap()
            .unwrap();

        assert_eq!(result.ids_added, 1);
        // The result should have :PROPERTIES: / :ID: / :END: inserted after heading.
        assert!(result.content.contains(":PROPERTIES:\n:ID: "));
        assert!(result.content.contains("\n:END:\n"));
        // Heading and body should still be there.
        assert!(result.content.starts_with("* Heading\n:PROPERTIES:"));
        assert!(result.content.contains("Some body text"));
    }

    // --- add_ids: existing drawer without ID ---

    #[test]
    fn add_id_existing_drawer() {
        let content = "* Heading\n:PROPERTIES:\n:CATEGORY: work\n:END:\nBody\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        let result = add_ids(&source, &doc, None, &IdGenerator::Uuid)
            .unwrap()
            .unwrap();

        assert_eq!(result.ids_added, 1);
        // :ID: should be inserted as the first property inside the drawer.
        let props_pos = result.content.find(":PROPERTIES:").unwrap();
        let id_pos = result.content.find(":ID: ").unwrap();
        let cat_pos = result.content.find(":CATEGORY:").unwrap();
        assert!(id_pos > props_pos, "ID should be after :PROPERTIES:");
        assert!(id_pos < cat_pos, "ID should be before existing properties");
    }

    // --- add_ids: already has ID ---

    #[test]
    fn skip_entry_with_existing_id() {
        let content = "* Heading\n:PROPERTIES:\n:ID: existing-id\n:END:\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        let result = add_ids(&source, &doc, None, &IdGenerator::Uuid).unwrap();
        assert!(
            result.is_none(),
            "Should return None when all entries have IDs"
        );
    }

    // --- add_ids: multiple entries, mixed ---

    #[test]
    fn mixed_entries() {
        let content = "* Has ID\n:PROPERTIES:\n:ID: abc\n:END:\n* No ID\nBody\n* Also no ID\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        let result = add_ids(&source, &doc, None, &IdGenerator::Uuid)
            .unwrap()
            .unwrap();

        assert_eq!(result.ids_added, 2);
        // Original ID should be preserved.
        assert!(result.content.contains(":ID: abc"));
    }

    // --- add_ids: with planning line ---

    #[test]
    fn add_id_after_planning() {
        let content = "* TODO Task\nSCHEDULED: <2026-03-21>\nBody\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        let result = add_ids(&source, &doc, None, &IdGenerator::Uuid)
            .unwrap()
            .unwrap();

        assert_eq!(result.ids_added, 1);
        // Drawer should be after the planning line.
        let sched_pos = result.content.find("SCHEDULED:").unwrap();
        let props_pos = result.content.find(":PROPERTIES:").unwrap();
        assert!(props_pos > sched_pos);
    }

    // --- add_ids: planning + existing drawer ---

    #[test]
    fn add_id_planning_and_drawer() {
        let content =
            "* TODO Task\nSCHEDULED: <2026-03-21>\n:PROPERTIES:\n:EFFORT: 1:00\n:END:\nBody\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        let result = add_ids(&source, &doc, None, &IdGenerator::Uuid)
            .unwrap()
            .unwrap();

        assert_eq!(result.ids_added, 1);
        // :ID: should be inside the existing drawer.
        let id_pos = result.content.find(":ID: ").unwrap();
        let effort_pos = result.content.find(":EFFORT:").unwrap();
        assert!(id_pos < effort_pos);
    }

    // --- add_ids: targeted entries ---

    #[test]
    fn add_id_targeted_single() {
        let content = "* A\n* B\n* C\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        // Only target entry 1 (B).
        let result = add_ids(&source, &doc, Some(&[1]), &IdGenerator::Uuid)
            .unwrap()
            .unwrap();

        assert_eq!(result.ids_added, 1);
        // A and C should not have drawers.
        let doc2 = {
            let s2 = SourceFile::new("test.org", result.content.clone());
            OrgDocument::from_source(&s2)
        };
        assert!(doc2.entries[0].properties.get("ID").is_none());
        assert!(doc2.entries[1].properties.get("ID").is_some());
        assert!(doc2.entries[2].properties.get("ID").is_none());
    }

    // --- collect_subtree ---

    #[test]
    fn subtree_collection() {
        let content = "* A\n** B\n*** C\n** D\n* E\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        // A is entry 0, its subtree should be [0, 1, 2, 3] (A, B, C, D).
        let mut subtree = collect_subtree(&doc, 0);
        subtree.sort();
        assert_eq!(subtree, vec![0, 1, 2, 3]);

        // B is entry 1, its subtree should be [1, 2] (B, C).
        let mut subtree = collect_subtree(&doc, 1);
        subtree.sort();
        assert_eq!(subtree, vec![1, 2]);

        // E is entry 4, standalone.
        assert_eq!(collect_subtree(&doc, 4), vec![4]);
    }

    // --- add_ids with recursive subtree ---

    #[test]
    fn add_id_subtree() {
        let content = "* A\n** B\n*** C\n* D\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        let subtree = collect_subtree(&doc, 0); // A and descendants.
        let result = add_ids(&source, &doc, Some(&subtree), &IdGenerator::Uuid)
            .unwrap()
            .unwrap();

        assert_eq!(result.ids_added, 3); // A, B, C — not D.
        let doc2 = {
            let s2 = SourceFile::new("test.org", result.content.clone());
            OrgDocument::from_source(&s2)
        };
        assert!(doc2.entries[0].properties.get("ID").is_some()); // A
        assert!(doc2.entries[1].properties.get("ID").is_some()); // B
        assert!(doc2.entries[2].properties.get("ID").is_some()); // C
        assert!(doc2.entries[3].properties.get("ID").is_none()); // D
    }

    // --- template expansion ---

    #[test]
    fn template_placeholders() {
        let gen = IdGenerator::Template("{file_stem}-{title_slug}-{level}".to_string());
        let info = EntryInfo {
            file: Path::new("projects.org"),
            line: 1,
            level: 2,
            title: "Quarterly Review",
            keyword: None,
            tags: &[],
            properties: &HashMap::new(),
            index: 0,
        };
        let id = gen.generate(&info).unwrap();
        assert_eq!(id, "projects-quarterly-review-2");
    }

    // --- empty file ---

    #[test]
    fn empty_file() {
        let source = SourceFile::new("test.org", String::new());
        let doc = OrgDocument::from_source(&source);
        let result = add_ids(&source, &doc, None, &IdGenerator::Uuid).unwrap();
        assert!(result.is_none());
    }

    // --- file with no headings ---

    #[test]
    fn file_without_headings() {
        let content = "Just some text\nNo headings here\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);
        let result = add_ids(&source, &doc, None, &IdGenerator::Uuid).unwrap();
        assert!(result.is_none());
    }

    // --- entry at end of file without trailing newline ---

    #[test]
    fn entry_at_eof_no_newline() {
        let content = "* Heading";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        let result = add_ids(&source, &doc, None, &IdGenerator::Uuid)
            .unwrap()
            .unwrap();

        assert_eq!(result.ids_added, 1);
        assert!(result.content.starts_with("* Heading\n:PROPERTIES:"));
    }

    // --- idempotency ---

    #[test]
    fn idempotent() {
        let content = "* A\n* B\n";
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);

        let result = add_ids(&source, &doc, None, &IdGenerator::Uuid)
            .unwrap()
            .unwrap();

        // Run again on the result.
        let source2 = SourceFile::new("test.org", result.content);
        let doc2 = OrgDocument::from_source(&source2);
        let result2 = add_ids(&source2, &doc2, None, &IdGenerator::Uuid).unwrap();
        assert!(result2.is_none(), "Second run should be a no-op");
    }
}
