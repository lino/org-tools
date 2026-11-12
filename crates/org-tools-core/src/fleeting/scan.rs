// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scanner discovering file-level and heading-level Luhmann fleeting notes.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::document::OrgDocument;
use crate::files::{canonicalize_clean, collect_org_files};
use crate::roam::lookup_prop_ci;
use crate::source::SourceFile;

use super::types::{FleetingItem, FleetingKind, FleetingStatus, FleetingStatusFilter};

// region Body & Property Extraction Helpers

/// Extracts raw thoughts from the body under `* Fleeting Thoughts` or the file preamble body.
fn extract_file_fleeting_thoughts(doc: &OrgDocument, source: &SourceFile) -> String {
    let lines: Vec<&str> = source.content.lines().collect();
    if let Some((idx, entry)) = doc
        .entries
        .iter()
        .enumerate()
        .find(|(_, e)| e.title.trim().eq_ignore_ascii_case("Fleeting Thoughts"))
    {
        let start_line = entry.heading_line;
        let end_line = if idx + 1 < doc.entries.len() {
            doc.entries[idx + 1].heading_line.saturating_sub(1)
        } else {
            lines.len()
        };
        return slice_body_lines(&lines, start_line, end_line);
    }
    slice_preamble_body_lines(&lines)
}

/// Slices body lines between 1-based `start_line` and `end_line`, trimming outer blank lines.
fn slice_body_lines(lines: &[&str], start_line: usize, end_line: usize) -> String {
    let mut collected = Vec::new();
    for &line in lines.iter().take(end_line).skip(start_line) {
        if line.starts_with('*') {
            break;
        }
        collected.push(line);
    }
    collected.join("\n").trim().to_string()
}

/// Extracts body lines preceding the first heading, skipping drawers and `#+` lines.
fn slice_preamble_body_lines(lines: &[&str]) -> String {
    let mut in_drawer = false;
    let mut collected = Vec::new();
    for &line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('*') {
            break;
        }
        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
            in_drawer = true;
            continue;
        }
        if in_drawer {
            if trimmed.eq_ignore_ascii_case(":END:") {
                in_drawer = false;
            }
            continue;
        }
        if trimmed.starts_with("#+") {
            continue;
        }
        collected.push(line);
    }
    collected.join("\n").trim().to_string()
}

/// Extracts heading entry body lines, skipping property drawers and planning lines.
fn extract_heading_body(doc: &OrgDocument, source: &SourceFile, entry_idx: usize) -> String {
    let entry = &doc.entries[entry_idx];
    let lines: Vec<&str> = source.content.lines().collect();
    let start_line = entry.heading_line;
    let end_line = if entry_idx + 1 < doc.entries.len() {
        doc.entries[entry_idx + 1].heading_line.saturating_sub(1)
    } else {
        lines.len()
    };
    let mut in_drawer = false;
    let mut collected = Vec::new();
    for &line in lines.iter().take(end_line).skip(start_line) {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
            in_drawer = true;
            continue;
        }
        if in_drawer {
            if trimmed.eq_ignore_ascii_case(":END:") {
                in_drawer = false;
            }
            continue;
        }
        if trimmed.starts_with("SCHEDULED:") || trimmed.starts_with("DEADLINE:") {
            continue;
        }
        collected.push(line);
    }
    collected.join("\n").trim().to_string()
}

/// Extracts an optional non-empty property string by case-insensitive `key`.
fn get_prop(props: &HashMap<String, String>, key: &str) -> Option<String> {
    lookup_prop_ci(props, key)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// endregion

// region Node Inspection & Status Derivation

/// Checks if `doc` represents a file-level fleeting note.
fn is_file_level_fleeting(doc: &OrgDocument) -> bool {
    if let Some(t) = lookup_prop_ci(&doc.file_properties, "TYPE") {
        if t.trim().eq_ignore_ascii_case("fleeting") {
            return true;
        }
    }
    doc.filetags
        .iter()
        .any(|t| t.trim().eq_ignore_ascii_case("fleeting"))
}

/// Parses or infers status for a file-level fleeting note.
fn resolve_file_status(props: &HashMap<String, String>) -> FleetingStatus {
    if let Some(s) = lookup_prop_ci(props, "STATUS") {
        if let Some(parsed) = FleetingStatus::from_status_str(s) {
            return parsed;
        }
    }
    if lookup_prop_ci(props, "PERMANENT_NOTE").is_some() {
        FleetingStatus::Reformed
    } else if lookup_prop_ci(props, "TASK_NOTE").is_some() {
        FleetingStatus::ConvertedToTask
    } else {
        FleetingStatus::Unrefined
    }
}

/// Resolves status for a heading stub from its properties or TODO state.
fn resolve_heading_status(
    props: &HashMap<String, String>,
    keyword: Option<&str>,
) -> FleetingStatus {
    if let Some(s) = lookup_prop_ci(props, "STATUS") {
        if let Some(parsed) = FleetingStatus::from_status_str(s) {
            return parsed;
        }
    }
    if lookup_prop_ci(props, "PERMANENT_NOTE").is_some() {
        return FleetingStatus::Reformed;
    }
    if lookup_prop_ci(props, "TASK_NOTE").is_some() {
        return FleetingStatus::ConvertedToTask;
    }
    if let Some(kw) = keyword {
        if kw.eq_ignore_ascii_case("DONE") {
            return FleetingStatus::Reformed;
        }
        if kw.eq_ignore_ascii_case("CANCELLED") || kw.eq_ignore_ascii_case("CANCELED") {
            return FleetingStatus::Discarded;
        }
    }
    FleetingStatus::Unrefined
}

// endregion

// region File & Heading Fleeting Item Builders

/// Builds a [`FleetingItem`] from a file-level fleeting note document.
fn build_file_fleeting_item(
    path: &Path,
    doc: &OrgDocument,
    source: &SourceFile,
) -> Option<FleetingItem> {
    if !is_file_level_fleeting(doc) {
        return None;
    }
    let id = get_prop(&doc.file_properties, "ID");
    let status = resolve_file_status(&doc.file_properties);
    let path_str = path.display().to_string();
    let locator = id
        .as_ref()
        .map(|uuid| format!("id:{uuid}"))
        .unwrap_or_else(|| path_str.clone());
    let raw_thoughts = extract_file_fleeting_thoughts(doc, source);

    Some(FleetingItem {
        id,
        title: doc.file_title_or_stem(),
        status,
        kind: FleetingKind::File,
        file: path_str,
        line: 1,
        locator,
        captured_at: get_prop(&doc.file_properties, "CAPTURED_AT"),
        attached_medium: get_prop(&doc.file_properties, "ATTACHED_MEDIUM"),
        medium_location: get_prop(&doc.file_properties, "MEDIUM_LOCATION"),
        attached_zettel: get_prop(&doc.file_properties, "ATTACHED_ZETTEL"),
        source_file: get_prop(&doc.file_properties, "SOURCE_FILE"),
        permanent_note: get_prop(&doc.file_properties, "PERMANENT_NOTE"),
        task_note: get_prop(&doc.file_properties, "TASK_NOTE"),
        raw_thoughts,
    })
}

/// Checks whether `tags` contains fleeting or zettel_stub tags.
fn has_fleeting_heading_tag(tags: &[String]) -> bool {
    tags.iter().any(|t| {
        let clean = t.trim().trim_matches(':');
        clean.eq_ignore_ascii_case("fleeting") || clean.eq_ignore_ascii_case("zettel_stub")
    })
}

/// Scans heading-level fleeting stubs inside `doc`.
fn scan_heading_stubs(
    path: &Path,
    doc: &OrgDocument,
    source: &SourceFile,
    filter: FleetingStatusFilter,
    items: &mut Vec<FleetingItem>,
) {
    let path_str = path.display().to_string();
    for (idx, entry) in doc.entries.iter().enumerate() {
        if !has_fleeting_heading_tag(&entry.tags) {
            continue;
        }
        let status = resolve_heading_status(&entry.properties, entry.keyword.as_deref());
        if !filter.matches(status) {
            continue;
        }
        let id = get_prop(&entry.properties, "ID");
        let locator = id
            .as_ref()
            .map(|uuid| format!("id:{uuid}"))
            .unwrap_or_else(|| format!("{path_str}::*{}", entry.title));
        let raw_thoughts = extract_heading_body(doc, source, idx);
        items.push(FleetingItem {
            id,
            title: entry.title.clone(),
            status,
            kind: FleetingKind::Heading,
            file: path_str.clone(),
            line: entry.heading_line,
            locator,
            captured_at: get_prop(&entry.properties, "CAPTURED_AT"),
            attached_medium: get_prop(&entry.properties, "ATTACHED_MEDIUM"),
            medium_location: get_prop(&entry.properties, "MEDIUM_LOCATION"),
            attached_zettel: get_prop(&entry.properties, "ATTACHED_ZETTEL"),
            source_file: get_prop(&entry.properties, "SOURCE_FILE"),
            permanent_note: get_prop(&entry.properties, "PERMANENT_NOTE"),
            task_note: get_prop(&entry.properties, "TASK_NOTE"),
            raw_thoughts,
        });
    }
}

// endregion

// region Public Scanning API

/// Scans `roots` discovering all fleeting notes matching `filter`.
pub fn scan_fleeting_items(roots: &[PathBuf], filter: FleetingStatusFilter) -> Vec<FleetingItem> {
    let mut items = Vec::new();
    let mut seen_files = HashSet::new();
    for file in collect_org_files(roots) {
        let canon = canonicalize_clean(&file).unwrap_or_else(|_| file.clone());
        if !seen_files.insert(canon) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        let source = SourceFile::new(&file, content);
        let doc = OrgDocument::from_source(&source);
        if let Some(file_item) = build_file_fleeting_item(&file, &doc, &source) {
            if filter.matches(file_item.status) {
                items.push(file_item);
            }
        } else {
            scan_heading_stubs(&file, &doc, &source, filter, &mut items);
        }
    }
    items
}

// endregion
