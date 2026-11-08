// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Atomic subtree refiling across same-file and cross-file Org-mode targets.
//!
//! Spec: [§9.5 Refile and Copy](https://orgmode.org/manual/Refile-and-Copy.html),
//! [Syntax: Headlines and Sections](https://orgmode.org/worg/org-syntax.html#Headlines_and_Sections).

use std::path::Path;

use crate::document::{OrgDocument, OrgEntry};
use crate::locator::locator_for_entry;
use crate::property::{format_heading_with_tags, DEFAULT_TAG_COLUMN};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::heading::{is_heading, parse_heading_with_keywords};
use crate::source::SourceFile;

// region Public Types

/// Destination placement within a target Org document for a subtree refile operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefileDestination {
    /// Append the refiled subtree as a level-1 (`*`) root heading at the end of the file.
    FileRoot,
    /// Insert the refiled subtree as the last child at the end of the target entry's subtree.
    UnderEntry(usize),
}

/// Options controlling heading level adjustment and `F007` tag alignment during refiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefileOptions {
    /// Force the refiled subtree root to level 1 (`*`) at the end of the destination file.
    pub as_root: bool,
    /// Target display column for `F007` heading tag alignment (defaults to `77`).
    pub tag_column: usize,
}

impl Default for RefileOptions {
    /// Returns default refile options (`as_root: false`, `tag_column: 77`).
    fn default() -> Self {
        Self {
            as_root: false,
            tag_column: DEFAULT_TAG_COLUMN,
        }
    }
}

/// Computed outcome of a same-file or cross-file subtree refile operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefileOutcome {
    /// Updated content of the source file with the refiled subtree removed.
    pub source_content: String,
    /// Updated content of the destination file with the adjusted subtree inserted.
    pub dest_content: String,
    /// Whether the source and destination are the same document.
    pub same_file: bool,
    /// Heading title of the refiled subtree root.
    pub title: String,
    /// Original 1-based heading line of the refiled subtree root in the source document.
    pub old_line: usize,
    /// New 1-based heading line of the refiled subtree root in the destination document.
    pub new_line: usize,
    /// Original heading level (`*` count) of the refiled subtree root.
    pub old_level: usize,
    /// New heading level (`*` count) of the refiled subtree root in the destination document.
    pub new_level: usize,
    /// Canonical locator string of the refiled entry in the updated destination document.
    pub new_locator: String,
}

// endregion

// region Heading Level & Tag Alignment

/// Re-aligns tags on a single heading line to `tag_column` (`F007`), leaving tagless headings unchanged.
pub fn align_heading_tags_line(
    heading_line: &str,
    keywords: &[&str],
    tag_column: usize,
) -> Result<String, String> {
    let raw = heading_line.strip_suffix('\r').unwrap_or(heading_line);
    let Some(parts) = parse_heading_with_keywords(raw, keywords) else {
        return Ok(raw.to_string());
    };
    if parts.tags.is_empty() {
        return Ok(raw.to_string());
    }
    let owned_tags: Vec<String> = parts.tags.iter().map(|t| (*t).to_string()).collect();
    format_heading_with_tags(raw, keywords, &owned_tags, tag_column)
}

/// Adjusts a single unprotected heading line's star level by `delta` and aligns its tags to `tag_column`.
fn adjust_heading_line(
    line: &str,
    keywords: &[&str],
    delta: isize,
    tag_column: usize,
) -> Result<String, String> {
    let raw = line.strip_suffix('\r').unwrap_or(line);
    let Some(parts) = parse_heading_with_keywords(raw, keywords) else {
        return Ok(raw.to_string());
    };
    let new_level = ((parts.level as isize) + delta).max(1) as usize;
    let rest = &raw[parts.level..];
    let relevelled = if rest.is_empty() || rest.starts_with(' ') {
        format!("{}{rest}", "*".repeat(new_level))
    } else {
        format!("{} {rest}", "*".repeat(new_level))
    };
    align_heading_tags_line(&relevelled, keywords, tag_column)
}

/// Extracts and level-adjusts the source subtree lines while preserving protected blocks.
fn extract_adjusted_subtree(
    source_content: &str,
    source_lines: &[String],
    range: (usize, usize),
    keywords: &[&str],
    delta: isize,
    tag_column: usize,
) -> Result<Vec<String>, String> {
    let protected = protected_regions(source_content);
    let mut subtree = Vec::with_capacity(range.1.saturating_sub(range.0));
    for (offset, line) in source_lines[range.0..range.1].iter().enumerate() {
        let adjusted = if is_protected(range.0 + offset, &protected) {
            line.clone()
        } else {
            adjust_heading_line(line, keywords, delta, tag_column)?
        };
        subtree.push(adjusted);
    }
    while subtree.last().is_some_and(|l| l.trim().is_empty()) {
        subtree.pop();
    }
    Ok(subtree)
}

// endregion

// region Line Splicing & Spacing Normalization

/// Splits document text into logical lines without a trailing empty element after the final `\n`.
fn split_logical_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }
    content
        .strip_suffix('\n')
        .unwrap_or(content)
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect()
}

/// Joins logical lines back into a trailing-newline-terminated Org document string.
fn join_logical_lines(mut lines: Vec<String>) -> String {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

/// Normalizes blank lines immediately above `heading_idx` per `F003` and returns the signed line delta.
fn normalize_spacing_before_heading(lines: &mut Vec<String>, heading_idx: usize) -> isize {
    if heading_idx == 0 || heading_idx >= lines.len() || !is_heading(&lines[heading_idx]) {
        return 0;
    }
    let mut cursor = heading_idx;
    while cursor > 0 && lines[cursor - 1].trim().is_empty() {
        cursor -= 1;
    }
    let blank_count = heading_idx - cursor;
    if cursor == 0 {
        lines.drain(0..heading_idx);
        return -(heading_idx as isize);
    }
    let desired = usize::from(!is_heading(&lines[cursor - 1]));
    if blank_count != desired {
        lines.splice(cursor..heading_idx, vec![String::new(); desired]);
    }
    (desired as isize) - (blank_count as isize)
}

/// Splices `subtree` into `dest_lines` at `insert_idx`, normalizes `F003` spacing, and returns the new 1-based root line.
fn splice_subtree_into_lines(
    dest_lines: &mut Vec<String>,
    insert_idx: usize,
    subtree: Vec<String>,
    extra_heading_idx: Option<usize>,
) -> usize {
    let k = subtree.len();
    dest_lines.splice(insert_idx..insert_idx, subtree);
    let mut boundary_indices = vec![insert_idx, insert_idx + k];
    if let Some(extra) = extra_heading_idx {
        boundary_indices.push(extra);
    }
    boundary_indices.sort_unstable();
    boundary_indices.dedup();

    let mut tracked_insert = insert_idx as isize;
    for &idx in boundary_indices.iter().rev() {
        let delta = normalize_spacing_before_heading(dest_lines, idx);
        if idx <= insert_idx {
            tracked_insert += delta;
        }
    }
    let leading_blanks = dest_lines
        .iter()
        .take_while(|l| l.trim().is_empty())
        .count() as isize;
    (tracked_insert - leading_blanks).max(0) as usize + 1
}

// endregion

// region Core Refile Operations

/// Validates `source_idx` and computes `(source_entry, new_root_level, dest_insert_line_idx)`.
fn resolve_refile_bounds<'a>(
    source_doc: &'a OrgDocument,
    source_idx: usize,
    dest_doc: &OrgDocument,
    dest_line_count: usize,
    dest: RefileDestination,
    opts: RefileOptions,
    same_file: bool,
) -> Result<(&'a OrgEntry, usize, usize), String> {
    let source_entry = source_doc
        .entries
        .get(source_idx)
        .ok_or_else(|| format!("source entry index {source_idx} out of bounds"))?;
    if opts.as_root {
        return Ok((source_entry, 1, dest_line_count));
    }
    match dest {
        RefileDestination::FileRoot => Ok((source_entry, 1, dest_line_count)),
        RefileDestination::UnderEntry(dest_idx) => {
            let dest_entry = dest_doc
                .entries
                .get(dest_idx)
                .ok_or_else(|| format!("destination entry index {dest_idx} out of bounds"))?;
            if same_file
                && dest_entry.heading_line >= source_entry.heading_line
                && dest_entry.subtree_end_line <= source_entry.subtree_end_line
            {
                return Err(format!(
                    "cannot refile '{}' into itself or its own descendant '{}'",
                    source_entry.title, dest_entry.title
                ));
            }
            let insert_idx = (dest_entry.subtree_end_line - 1).min(dest_line_count);
            Ok((source_entry, dest_entry.level + 1, insert_idx))
        }
    }
}

/// Builds a [`RefileOutcome`] and resolves the refiled entry's new canonical [`OrgLocator`].
fn build_refile_outcome(
    source_entry: &OrgEntry,
    dest_file: &Path,
    source_content: String,
    dest_content: String,
    same_file: bool,
    new_line: usize,
    new_level: usize,
) -> RefileOutcome {
    let updated_src = SourceFile::new(dest_file.to_path_buf(), dest_content.clone());
    let updated_doc = OrgDocument::from_source(&updated_src);
    let idx = updated_doc
        .entries
        .iter()
        .position(|e| e.heading_line == new_line)
        .unwrap_or(0);
    let new_locator = locator_for_entry(&updated_doc, idx).to_string();
    RefileOutcome {
        source_content,
        dest_content,
        same_file,
        title: source_entry.title.clone(),
        old_line: source_entry.heading_line,
        new_line,
        old_level: source_entry.level,
        new_level,
        new_locator,
    }
}

/// Refiles `source_idx` to `dest` within the same [`OrgDocument`].
pub fn refile_same_file(
    source: &SourceFile,
    doc: &OrgDocument,
    source_idx: usize,
    dest: RefileDestination,
    opts: RefileOptions,
) -> Result<RefileOutcome, String> {
    let mut lines = split_logical_lines(&source.content);
    let (src_entry, new_level, dest_insert) =
        resolve_refile_bounds(doc, source_idx, doc, lines.len(), dest, opts, true)?;
    let (src_start, src_end) = (
        src_entry.heading_line - 1,
        (src_entry.subtree_end_line - 1).min(lines.len()),
    );
    let delta = (new_level as isize) - (src_entry.level as isize);
    let kw_refs = doc.todo_keywords.all();
    let subtree = extract_adjusted_subtree(
        &source.content,
        &lines,
        (src_start, src_end),
        &kw_refs,
        delta,
        opts.tag_column,
    )?;
    lines.drain(src_start..src_end);
    let insert_site = if src_end <= dest_insert {
        dest_insert - (src_end - src_start)
    } else {
        dest_insert
    };
    let rem_heading = if src_start <= insert_site {
        src_start
    } else {
        src_start + subtree.len()
    };
    let new_line = splice_subtree_into_lines(&mut lines, insert_site, subtree, Some(rem_heading));
    let dest_content = join_logical_lines(lines);
    Ok(build_refile_outcome(
        src_entry,
        &doc.file,
        dest_content.clone(),
        dest_content,
        true,
        new_line,
        new_level,
    ))
}

/// Refiles `source_idx` from `source_doc` into `dest_doc` at `dest`.
pub fn refile_cross_file(
    source: &SourceFile,
    source_doc: &OrgDocument,
    source_idx: usize,
    dest_source: &SourceFile,
    dest_doc: &OrgDocument,
    dest: RefileDestination,
    opts: RefileOptions,
) -> Result<RefileOutcome, String> {
    let mut src_lines = split_logical_lines(&source.content);
    let mut dest_lines = split_logical_lines(&dest_source.content);
    let (src_entry, new_level, dest_insert) = resolve_refile_bounds(
        source_doc,
        source_idx,
        dest_doc,
        dest_lines.len(),
        dest,
        opts,
        false,
    )?;
    let (src_start, src_end) = (
        src_entry.heading_line - 1,
        (src_entry.subtree_end_line - 1).min(src_lines.len()),
    );
    let delta = (new_level as isize) - (src_entry.level as isize);
    let mut kw_refs = source_doc.todo_keywords.all();
    for kw in dest_doc.todo_keywords.all() {
        if !kw_refs.contains(&kw) {
            kw_refs.push(kw);
        }
    }
    let subtree = extract_adjusted_subtree(
        &source.content,
        &src_lines,
        (src_start, src_end),
        &kw_refs,
        delta,
        opts.tag_column,
    )?;
    src_lines.drain(src_start..src_end);
    normalize_spacing_before_heading(&mut src_lines, src_start);
    let source_content = join_logical_lines(src_lines);
    let new_line = splice_subtree_into_lines(&mut dest_lines, dest_insert, subtree, None);
    let dest_content = join_logical_lines(dest_lines);
    Ok(build_refile_outcome(
        src_entry,
        &dest_doc.file,
        source_content,
        dest_content,
        false,
        new_line,
        new_level,
    ))
}

// endregion

// region Unit Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies `refile_same_file` rejects self and descendant targets and shifts levels cleanly.
    #[test]
    fn same_file_rejects_cycles_and_adjusts_levels() {
        let raw = "* A :tag:\n** A1\n* B\n";
        let sf = SourceFile::new("test.org", raw.to_string());
        let doc = OrgDocument::from_source(&sf);
        assert!(refile_same_file(
            &sf,
            &doc,
            0,
            RefileDestination::UnderEntry(0),
            RefileOptions::default()
        )
        .is_err());
        assert!(refile_same_file(
            &sf,
            &doc,
            0,
            RefileDestination::UnderEntry(1),
            RefileOptions::default()
        )
        .is_err());
        let out = refile_same_file(
            &sf,
            &doc,
            0,
            RefileDestination::UnderEntry(2),
            RefileOptions::default(),
        )
        .unwrap();
        assert_eq!((out.old_level, out.new_level), (1, 2));
        assert!(out.dest_content.contains("\n*** A1\n"));
    }
}

// endregion
