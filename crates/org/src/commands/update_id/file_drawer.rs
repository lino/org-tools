// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Helpers for level-0 file `:PROPERTIES:` drawer `:ID:` inspection and insertion.

use org_tools_core::document::OrgDocument;
use org_tools_core::property::format_property_lines;
use org_tools_core::source::SourceFile;

use super::record::AddIdRecord;

/// Returns `true` if `line` is an Org comment line (`#` followed by space, tab, or EOL).
fn is_org_comment_line(line: &str) -> bool {
    let trimmed = line.strip_suffix('\r').unwrap_or(line).trim_start();
    trimmed == "#" || trimmed.starts_with("# ") || trimmed.starts_with("#\t")
}

/// Finds the 0-based insertion line for a level-0 drawer after leading comments and before `#+TITLE:`.
fn find_level_0_insert_idx(lines: &[&str]) -> usize {
    let mut idx = 0;
    while idx < lines.len() && is_org_comment_line(lines[idx]) {
        idx += 1;
    }
    idx
}

/// Parses existing drawer properties between `drawer_start` and `drawer_end`.
fn extract_drawer_properties(
    lines: &[&str],
    drawer_start: usize,
    drawer_end: usize,
) -> Vec<(String, String)> {
    let mut props = Vec::new();
    for line in &lines[drawer_start + 1..drawer_end] {
        let trimmed = line.trim();
        if let Some(stripped) = trimmed.strip_prefix(':') {
            if let Some((k, v)) = stripped.split_once(':') {
                props.push((k.trim().to_ascii_uppercase(), v.trim().to_string()));
            }
        }
    }
    props
}

/// Inserts `:ID:` into an existing level-0 drawer that lacked one.
fn splice_into_existing_drawer(
    lines: &[&str],
    drawer_start: usize,
    drawer_end: usize,
    id_value: &str,
    id_seed: Option<&str>,
) -> String {
    let mut props = extract_drawer_properties(lines, drawer_start, drawer_end);
    let mut new_props = vec![("ID".to_string(), id_value.to_string())];
    if let Some(seed) = id_seed {
        new_props.push(("ID_SEED".to_string(), seed.to_string()));
    }
    for (k, v) in props.drain(..) {
        if k != "ID" && k != "ID_SEED" {
            new_props.push((k, v));
        }
    }

    let mut out_lines = Vec::new();
    out_lines.extend_from_slice(&lines[..drawer_start]);
    out_lines.push(":PROPERTIES:");
    let formatted = format_property_lines(&new_props, "");
    for f in &formatted {
        out_lines.push(f.as_str());
    }
    out_lines.push(":END:");
    out_lines.extend_from_slice(&lines[drawer_end + 1..]);
    out_lines.join("\n")
}

/// Inserts a newly created level-0 `:PROPERTIES:` drawer at `insert_idx`.
fn insert_fresh_file_drawer(
    lines: &[&str],
    insert_idx: usize,
    id_value: &str,
    id_seed: Option<&str>,
) -> String {
    let mut props = vec![("ID".to_string(), id_value.to_string())];
    if let Some(seed) = id_seed {
        props.push(("ID_SEED".to_string(), seed.to_string()));
    }

    let formatted = format_property_lines(&props, "");
    let mut out_lines = Vec::new();
    out_lines.extend_from_slice(&lines[..insert_idx]);
    out_lines.push(":PROPERTIES:");
    for f in &formatted {
        out_lines.push(f.as_str());
    }
    out_lines.push(":END:");
    out_lines.extend_from_slice(&lines[insert_idx..]);
    out_lines.join("\n")
}

/// Ensures a level-0 file `:PROPERTIES:` drawer carries an `:ID:` property.
pub(crate) fn apply_file_level_id(
    source: &SourceFile,
    doc: &OrgDocument,
    id_value: &str,
    id_seed: Option<&str>,
) -> (String, AddIdRecord) {
    let file_str = source.path.display().to_string();

    // Idempotent: file already has an :ID: property
    if let Some(existing) = doc.file_properties.get("ID") {
        let record = AddIdRecord {
            file: file_str.clone(),
            locator: file_str,
            level: 0,
            line: doc.file_properties_line.unwrap_or(1),
            id: existing.clone(),
            id_seed: doc.file_properties.get("ID_SEED").cloned(),
            created: false,
            cache_synced: false,
        };
        return (source.content.clone(), record);
    }

    let lines: Vec<&str> = source.content.split('\n').collect();

    let (mut new_content, line) = if let Some(fl) = doc.file_properties_line {
        let start = fl - 1;
        let mut end = start + 1;
        while end < lines.len() && !lines[end].trim().eq_ignore_ascii_case(":END:") {
            end += 1;
        }
        (
            splice_into_existing_drawer(&lines, start, end.min(lines.len() - 1), id_value, id_seed),
            fl,
        )
    } else {
        let idx = find_level_0_insert_idx(&lines);
        (
            insert_fresh_file_drawer(&lines, idx, id_value, id_seed),
            idx + 1,
        )
    };

    if source.content.ends_with('\n') && !new_content.ends_with('\n') {
        new_content.push('\n');
    }

    let record = AddIdRecord {
        file: file_str.clone(),
        locator: file_str,
        level: 0,
        line,
        id: id_value.to_string(),
        id_seed: id_seed.map(str::to_string),
        created: true,
        cache_synced: false,
    };
    (new_content, record)
}
