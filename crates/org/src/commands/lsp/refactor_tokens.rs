// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=1200 reason="Cohesive LSP rename preparation, execution, and semantic token delta encoding"

//! LSP rename preparation, execution, and semantic token extraction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use org_tools_core::document::OrgDocument;
use org_tools_core::export_md::normalize_workspace_path;
use org_tools_core::property::{format_heading_with_tags, resolve_tag_column};
use org_tools_core::rules::format::regions::protected_regions;
use org_tools_core::rules::heading::parse_heading_with_keywords;

use super::{lsp_range, path_to_file_uri, LspSession};

// region Rename Target & Execution

/// Extracts a line by zero-based index from source content, stripping any carriage return.
fn get_line_str(content: &str, line_0: usize) -> Option<&str> {
    content
        .split('\n')
        .nth(line_0)
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
}

/// Calculates the UTF-16 code unit length of a string slice.
fn utf16_len(s: &str) -> u32 {
    s.chars().map(|c| c.len_utf16() as u32).sum()
}

/// A parsed Org bracket link occurrence on a single line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BracketLinkSpan {
    /// Zero-based UTF-16 start of the `[[` marker.
    link_start_u16: u32,
    /// Zero-based UTF-16 end of the closing `]]` marker.
    link_end_u16: u32,
    /// Raw link target text (inside first brackets).
    target: String,
    /// Zero-based UTF-16 start of the target text.
    target_start_u16: u32,
    /// Zero-based UTF-16 end of the target text.
    target_end_u16: u32,
    /// Optional link description text (inside second brackets).
    desc: Option<String>,
    /// Zero-based UTF-16 start of the description text if present.
    desc_start_u16: Option<u32>,
    /// Zero-based UTF-16 end of the description text if present.
    desc_end_u16: Option<u32>,
}

/// Attempts to parse a single bracket link starting at `open_idx` on `line`.
fn parse_bracket_link_at(line: &str, open_idx: usize) -> Option<(BracketLinkSpan, usize)> {
    let rest = &line[open_idx + 2..];
    let target_close_rel = rest.find(']')?;
    let target = &rest[..target_close_rel];
    if target.contains('[') {
        return None;
    }
    let after_target = &rest[target_close_rel..];
    if let Some(rest2) = after_target.strip_prefix("]]") {
        let close_idx = line.len() - rest2.len();
        let span = BracketLinkSpan {
            link_start_u16: utf16_len(&line[..open_idx]),
            link_end_u16: utf16_len(&line[..close_idx]),
            target: target.to_string(),
            target_start_u16: utf16_len(&line[..open_idx + 2]),
            target_end_u16: utf16_len(&line[..open_idx + 2 + target_close_rel]),
            desc: None,
            desc_start_u16: None,
            desc_end_u16: None,
        };
        Some((span, close_idx))
    } else if let Some(desc_rest) = after_target.strip_prefix("][") {
        let desc_close_rel = desc_rest.find("]]")?;
        let desc = &desc_rest[..desc_close_rel];
        let close_idx = open_idx + 2 + target_close_rel + 2 + desc_close_rel + 2;
        let desc_start_idx = open_idx + 2 + target_close_rel + 2;
        let span = BracketLinkSpan {
            link_start_u16: utf16_len(&line[..open_idx]),
            link_end_u16: utf16_len(&line[..close_idx]),
            target: target.to_string(),
            target_start_u16: utf16_len(&line[..open_idx + 2]),
            target_end_u16: utf16_len(&line[..open_idx + 2 + target_close_rel]),
            desc: Some(desc.to_string()),
            desc_start_u16: Some(utf16_len(&line[..desc_start_idx])),
            desc_end_u16: Some(utf16_len(&line[..desc_start_idx + desc.len()])),
        };
        Some((span, close_idx))
    } else {
        None
    }
}

/// Parses all bracket links `[[target]]` or `[[target][desc]]` on a single line.
fn scan_bracket_links(line: &str) -> Vec<BracketLinkSpan> {
    let mut links = Vec::new();
    let mut pos = 0;
    while let Some(start_rel) = line[pos..].find("[[") {
        let open_idx = pos + start_rel;
        if let Some((span, next_pos)) = parse_bracket_link_at(line, open_idx) {
            links.push(span);
            pos = next_pos;
        } else {
            pos = open_idx + 2;
        }
    }
    links
}

/// Represents an identified Org-mode symbol targeted for renaming.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RenameTarget {
    /// Heading title defined in `def_path` at `line_0`.
    Heading {
        /// Defining document filesystem path.
        def_path: PathBuf,
        /// Zero-based heading line number in the defining document.
        line_0: u32,
        /// Selection range for prepareRename at caller cursor position.
        cursor_range: (u32, u32, u32, u32),
        /// Original heading title text.
        old_title: String,
        /// Associated `:ID:` uuid if defined on the heading.
        id: Option<String>,
    },
    /// Custom ID property defined in `def_path`.
    CustomId {
        /// Defining document filesystem path.
        def_path: PathBuf,
        /// Zero-based line number of `:CUSTOM_ID:` in defining document.
        def_line_0: u32,
        /// Selection range for prepareRename at caller cursor position.
        cursor_range: (u32, u32, u32, u32),
        /// UTF-16 character range `(start, end)` of the custom ID value on `def_line_0`.
        val_range: (u32, u32),
        /// Original custom ID value string.
        old_cid: String,
    },
    /// Node `:ID:` property defined in `def_path`.
    Id {
        /// Defining document filesystem path.
        def_path: PathBuf,
        /// Zero-based line number of `:ID:` in defining document.
        def_line_0: u32,
        /// Selection range for prepareRename at caller cursor position.
        cursor_range: (u32, u32, u32, u32),
        /// UTF-16 character range `(start, end)` of the ID value on `def_line_0`.
        val_range: (u32, u32),
        /// Original `:ID:` value string.
        old_id: String,
    },
}

/// Finds the defining heading matching `title` across workspace documents.
fn find_heading_definition(
    session: &LspSession,
    preferred_path: &Path,
    title: &str,
) -> Option<(PathBuf, u32, Option<String>)> {
    if let Ok((_src, doc)) = session.get_or_load_document(preferred_path) {
        if let Some(entry) = doc.entries.iter().find(|e| e.title == title) {
            let l0 = (entry.heading_line.saturating_sub(1)) as u32;
            let id = entry.properties.get("ID").cloned();
            return Some((preferred_path.to_path_buf(), l0, id));
        }
    }
    for (path, _, doc) in session.collect_workspace_documents() {
        if let Some(entry) = doc.entries.iter().find(|e| e.title == title) {
            let l0 = (entry.heading_line.saturating_sub(1)) as u32;
            let id = entry.properties.get("ID").cloned();
            return Some((path, l0, id));
        }
    }
    None
}

/// Finds the defining line of a `:CUSTOM_ID:` property across workspace documents.
fn find_custom_id_definition(
    session: &LspSession,
    preferred_path: &Path,
    cid: &str,
) -> Option<(PathBuf, u32, (u32, u32))> {
    let ws_docs = session.collect_workspace_documents();
    let search_order =
        std::iter::once(preferred_path.to_path_buf()).chain(ws_docs.into_iter().map(|(p, _, _)| p));

    for path in search_order {
        if let Ok((src, _)) = session.get_or_load_document(&path) {
            for (idx, line) in src.content.split('\n').enumerate() {
                let trimmed = line.trim_start();
                if trimmed.to_ascii_uppercase().starts_with(":CUSTOM_ID:") {
                    let val = trimmed[11..].trim();
                    if val == cid {
                        let l0 = idx as u32;
                        let val_idx = line.rfind(cid).unwrap_or(0);
                        let s = utf16_len(&line[..val_idx]);
                        let e = s + utf16_len(cid);
                        return Some((path, l0, (s, e)));
                    }
                }
            }
        }
    }
    None
}

/// Finds the defining line of an `:ID:` property across workspace documents.
fn find_id_definition(
    session: &LspSession,
    preferred_path: &Path,
    id: &str,
) -> Option<(PathBuf, u32, (u32, u32))> {
    let ws_docs = session.collect_workspace_documents();
    let search_order =
        std::iter::once(preferred_path.to_path_buf()).chain(ws_docs.into_iter().map(|(p, _, _)| p));

    for path in search_order {
        if let Ok((src, _)) = session.get_or_load_document(&path) {
            for (idx, line) in src.content.split('\n').enumerate() {
                let trimmed = line.trim_start();
                if trimmed.to_ascii_uppercase().starts_with(":ID:")
                    && !trimmed.to_ascii_uppercase().starts_with(":ID_")
                {
                    let val = trimmed[4..].trim();
                    if val == id {
                        let l0 = idx as u32;
                        let val_idx = line.rfind(id).unwrap_or(0);
                        let s = utf16_len(&line[..val_idx]);
                        let e = s + utf16_len(id);
                        return Some((path, l0, (s, e)));
                    }
                }
            }
        }
    }
    None
}

/// Resolves a heading rename target from an internal or external heading link.
fn find_heading_link_target(
    link: &BracketLinkSpan,
    session: &LspSession,
    path: &Path,
    line_0: u32,
) -> Option<RenameTarget> {
    if let Some(title) = link.target.strip_prefix('*') {
        let (def_path, def_l0, id) = find_heading_definition(session, path, title)?;
        Some(RenameTarget::Heading {
            def_path,
            line_0: def_l0,
            cursor_range: (
                line_0,
                link.target_start_u16 + 1,
                line_0,
                link.target_end_u16,
            ),
            old_title: title.to_string(),
            id,
        })
    } else if let Some((file_part, title)) = link.target.split_once("::*") {
        let target_path = path.parent().unwrap_or(path).join(file_part);
        let (def_path, def_l0, id) = find_heading_definition(session, &target_path, title)?;
        let prefix_len = utf16_len(&format!("{file_part}::*"));
        Some(RenameTarget::Heading {
            def_path,
            line_0: def_l0,
            cursor_range: (
                line_0,
                link.target_start_u16 + prefix_len,
                line_0,
                link.target_end_u16,
            ),
            old_title: title.to_string(),
            id,
        })
    } else {
        None
    }
}

/// Resolves a custom ID rename target from an internal or external custom ID link.
fn find_custom_id_link_target(
    link: &BracketLinkSpan,
    session: &LspSession,
    path: &Path,
    line_0: u32,
) -> Option<RenameTarget> {
    if let Some(cid) = link.target.strip_prefix('#') {
        let (def_path, def_l0, val_range) = find_custom_id_definition(session, path, cid)?;
        Some(RenameTarget::CustomId {
            def_path,
            def_line_0: def_l0,
            cursor_range: (
                line_0,
                link.target_start_u16 + 1,
                line_0,
                link.target_end_u16,
            ),
            val_range,
            old_cid: cid.to_string(),
        })
    } else if let Some((file_part, cid)) = link.target.split_once("::#") {
        let target_path = path.parent().unwrap_or(path).join(file_part);
        let (def_path, def_l0, val_range) = find_custom_id_definition(session, &target_path, cid)?;
        let prefix_len = utf16_len(&format!("{file_part}::#"));
        Some(RenameTarget::CustomId {
            def_path,
            def_line_0: def_l0,
            cursor_range: (
                line_0,
                link.target_start_u16 + prefix_len,
                line_0,
                link.target_end_u16,
            ),
            val_range,
            old_cid: cid.to_string(),
        })
    } else {
        None
    }
}

/// Resolves an `:ID:` rename target from an `[[id:...]]` link.
fn find_id_link_target(
    link: &BracketLinkSpan,
    session: &LspSession,
    path: &Path,
    line_0: u32,
) -> Option<RenameTarget> {
    if !link.target.to_ascii_lowercase().starts_with("id:") {
        return None;
    }
    let raw_id = &link.target[3..];
    let id = raw_id.split_once("::").map_or(raw_id, |(b, _)| b).trim();
    let (def_path, def_l0, val_range) = find_id_definition(session, path, id)?;
    Some(RenameTarget::Id {
        def_path,
        def_line_0: def_l0,
        cursor_range: (
            line_0,
            link.target_start_u16 + 3,
            line_0,
            link.target_end_u16,
        ),
        val_range,
        old_id: id.to_string(),
    })
}

/// Identifies whether cursor is on a bracket link targeting a renameable symbol.
fn identify_link_rename_target(
    session: &LspSession,
    path: &Path,
    line_0: u32,
    char_0: u32,
    line_str: &str,
) -> Option<RenameTarget> {
    for link in scan_bracket_links(line_str) {
        if char_0 >= link.link_start_u16 && char_0 <= link.link_end_u16 {
            if let Some(target) = find_heading_link_target(&link, session, path, line_0) {
                return Some(target);
            }
            if let Some(target) = find_custom_id_link_target(&link, session, path, line_0) {
                return Some(target);
            }
            if let Some(target) = find_id_link_target(&link, session, path, line_0) {
                return Some(target);
            }
        }
    }
    None
}

/// Identifies whether cursor is on a defining heading line.
fn identify_heading_line_target(
    path: &Path,
    line_0: u32,
    line_str: &str,
    doc: &OrgDocument,
) -> Option<RenameTarget> {
    if !line_str.starts_with('*') {
        return None;
    }
    let parts = parse_heading_with_keywords(line_str, &doc.todo_keywords.all())?;
    let old_title = parts.title.to_string();
    let title_idx = line_str.find(parts.title).unwrap_or(0);
    let s = utf16_len(&line_str[..title_idx]);
    let e = s + utf16_len(parts.title);
    let id = doc
        .entries
        .iter()
        .find(|entry| entry.heading_line == (line_0 as usize) + 1)
        .and_then(|entry| entry.properties.get("ID").cloned());
    Some(RenameTarget::Heading {
        def_path: path.to_path_buf(),
        line_0,
        cursor_range: (line_0, s, line_0, e),
        old_title,
        id,
    })
}

/// Identifies whether cursor is on a `:CUSTOM_ID:` or `:ID:` property line.
fn identify_property_line_target(path: &Path, line_0: u32, line_str: &str) -> Option<RenameTarget> {
    let trimmed = line_str.trim_start();
    if trimmed.to_ascii_uppercase().starts_with(":CUSTOM_ID:") {
        let cid = trimmed[11..].trim();
        let val_idx = line_str.rfind(cid).unwrap_or(0);
        let s = utf16_len(&line_str[..val_idx]);
        let e = s + utf16_len(cid);
        return Some(RenameTarget::CustomId {
            def_path: path.to_path_buf(),
            def_line_0: line_0,
            cursor_range: (line_0, s, line_0, e),
            val_range: (s, e),
            old_cid: cid.to_string(),
        });
    }

    if trimmed.to_ascii_uppercase().starts_with(":ID:")
        && !trimmed.to_ascii_uppercase().starts_with(":ID_")
    {
        let id = trimmed[4..].trim();
        let val_idx = line_str.rfind(id).unwrap_or(0);
        let s = utf16_len(&line_str[..val_idx]);
        let e = s + utf16_len(id);
        return Some(RenameTarget::Id {
            def_path: path.to_path_buf(),
            def_line_0: line_0,
            cursor_range: (line_0, s, line_0, e),
            val_range: (s, e),
            old_id: id.to_string(),
        });
    }

    None
}

/// Resolves the symbol under cursor for prepareRename or rename.
fn identify_rename_target(
    session: &LspSession,
    path: &Path,
    line_0: u32,
    char_0: u32,
) -> Option<RenameTarget> {
    let (src, doc) = session.get_or_load_document(path).ok()?;
    let line_str = get_line_str(&src.content, line_0 as usize)?;

    if let Some(target) = identify_link_rename_target(session, path, line_0, char_0, line_str) {
        return Some(target);
    }
    identify_heading_line_target(path, line_0, line_str, &doc)
        .or_else(|| identify_property_line_target(path, line_0, line_str))
}

/// Rebuilds a heading line with `new_title` and aligned tags.
fn format_renamed_heading(orig_line: &str, doc: &OrgDocument, new_title: &str) -> Option<String> {
    let kw_all = doc.todo_keywords.all();
    let parts = parse_heading_with_keywords(orig_line, &kw_all)?;
    let mut prefix = format!("{} ", "*".repeat(parts.level));
    if let Some(kw) = parts.keyword {
        prefix.push_str(kw);
        prefix.push(' ');
    }
    if let Some(pri) = parts.priority {
        prefix.push_str(&format!("[#{pri}] "));
    }
    prefix.push_str(new_title);
    let tag_strings: Vec<String> = parts.tags.iter().map(|s| s.to_string()).collect();
    let tag_col = resolve_tag_column(None);
    Some(format_heading_with_tags(&prefix, &kw_all, &tag_strings, tag_col).unwrap_or(prefix))
}

/// Rewrites heading target links and matching ID link descriptions on a line.
fn update_heading_links(
    line: &str,
    l0: u32,
    old_title: &str,
    heading_id: Option<&str>,
    new_title: &str,
    file_edits: &mut Vec<Value>,
) {
    for link in scan_bracket_links(line) {
        if link.target == format!("*{old_title}") {
            file_edits.push(json!({
                "range": lsp_range(l0, link.target_start_u16, l0, link.target_end_u16),
                "newText": format!("*{new_title}")
            }));
        } else if link.target.ends_with(&format!("::*{old_title}")) {
            let base = &link.target[..link.target.len() - old_title.len()];
            file_edits.push(json!({
                "range": lsp_range(l0, link.target_start_u16, l0, link.target_end_u16),
                "newText": format!("{base}{new_title}")
            }));
        }
        if let Some(target_id) = heading_id {
            let is_target_id = link.target == format!("id:{target_id}")
                || link.target.starts_with(&format!("id:{target_id}::"));
            if is_target_id && link.desc.as_deref() == Some(old_title) {
                if let (Some(s), Some(e)) = (link.desc_start_u16, link.desc_end_u16) {
                    file_edits.push(json!({
                        "range": lsp_range(l0, s, l0, e),
                        "newText": new_title
                    }));
                }
            }
        }
    }
}

/// Rewrites `:CUSTOM_ID:` link targets on a line.
fn update_custom_id_links(
    line: &str,
    l0: u32,
    is_same_file: bool,
    old_cid: &str,
    new_cid: &str,
    file_edits: &mut Vec<Value>,
) {
    for link in scan_bracket_links(line) {
        if is_same_file && link.target == format!("#{old_cid}") {
            file_edits.push(json!({
                "range": lsp_range(l0, link.target_start_u16, l0, link.target_end_u16),
                "newText": format!("#{new_cid}")
            }));
        } else if link.target.ends_with(&format!("::#{old_cid}")) {
            let base = &link.target[..link.target.len() - old_cid.len()];
            file_edits.push(json!({
                "range": lsp_range(l0, link.target_start_u16, l0, link.target_end_u16),
                "newText": format!("{base}{new_cid}")
            }));
        }
    }
}

/// Rewrites `[[id:...]]` link targets on a line.
fn update_id_links(line: &str, l0: u32, old_id: &str, new_id: &str, file_edits: &mut Vec<Value>) {
    for link in scan_bracket_links(line) {
        if link.target == format!("id:{old_id}") {
            file_edits.push(json!({
                "range": lsp_range(l0, link.target_start_u16, l0, link.target_end_u16),
                "newText": format!("id:{new_id}")
            }));
        } else if link.target.starts_with(&format!("id:{old_id}::")) {
            let suffix = &link.target[3 + old_id.len()..];
            file_edits.push(json!({
                "range": lsp_range(l0, link.target_start_u16, l0, link.target_end_u16),
                "newText": format!("id:{new_id}{suffix}")
            }));
        }
    }
}

/// Context for constructing workspace rename edits.
struct RenameContext<'a> {
    /// Active LSP session handle.
    session: &'a LspSession,
    /// Client-provided document URI where rename was requested.
    req_uri: &'a str,
    /// Filesystem path of document where rename was requested.
    req_path: &'a Path,
    /// Accumulator for workspace edit changes indexed by URI.
    changes: &'a mut HashMap<String, Vec<Value>>,
}

impl<'a> RenameContext<'a> {
    /// Resolves the LSP file URI for a path, preferring client-provided URIs from session.
    fn resolve_uri(&self, path: &Path) -> String {
        let norm = normalize_workspace_path(path);
        if norm == normalize_workspace_path(self.req_path) {
            return self.req_uri.to_string();
        }
        if let Some(open) = self.session.open_docs.get(&norm) {
            return open.uri.clone();
        }
        path_to_file_uri(path)
    }

    /// Applies workspace-wide renames for a heading title and aligned tags.
    fn rename_heading(
        &mut self,
        def_path: &Path,
        def_line_0: u32,
        old_title: &str,
        heading_id: Option<&str>,
        new_title: &str,
    ) {
        if let Ok((def_src, def_doc)) = self.session.get_or_load_document(def_path) {
            if let Some(orig_line) = get_line_str(&def_src.content, def_line_0 as usize) {
                if let Some(new_line) = format_renamed_heading(orig_line, &def_doc, new_title) {
                    let def_uri = self.resolve_uri(def_path);
                    self.changes.entry(def_uri).or_default().push(json!({
                        "range": lsp_range(def_line_0, 0, def_line_0, utf16_len(orig_line)),
                        "newText": new_line
                    }));
                }
            }
        }

        for (ws_path, ws_src, _) in self.session.collect_workspace_documents() {
            let ws_uri = self.resolve_uri(&ws_path);
            let mut file_edits = Vec::new();
            for (l_idx, line) in ws_src.content.split('\n').enumerate() {
                update_heading_links(
                    line,
                    l_idx as u32,
                    old_title,
                    heading_id,
                    new_title,
                    &mut file_edits,
                );
            }
            if !file_edits.is_empty() {
                self.changes.entry(ws_uri).or_default().extend(file_edits);
            }
        }
    }

    /// Applies workspace-wide renames for a `:CUSTOM_ID:` property and link references.
    fn rename_custom_id(
        &mut self,
        def_path: &Path,
        def_line_0: u32,
        val_range: (u32, u32),
        old_cid: &str,
        new_cid: &str,
    ) {
        let def_uri = self.resolve_uri(def_path);
        self.changes.entry(def_uri).or_default().push(json!({
            "range": lsp_range(def_line_0, val_range.0, def_line_0, val_range.1),
            "newText": new_cid
        }));

        for (ws_path, ws_src, _) in self.session.collect_workspace_documents() {
            let ws_uri = self.resolve_uri(&ws_path);
            let mut file_edits = Vec::new();
            let is_same_file =
                normalize_workspace_path(&ws_path) == normalize_workspace_path(def_path);

            for (l_idx, line) in ws_src.content.split('\n').enumerate() {
                update_custom_id_links(
                    line,
                    l_idx as u32,
                    is_same_file,
                    old_cid,
                    new_cid,
                    &mut file_edits,
                );
            }
            if !file_edits.is_empty() {
                self.changes.entry(ws_uri).or_default().extend(file_edits);
            }
        }
    }

    /// Applies workspace-wide renames for an `:ID:` property and `[[id:...]]` link targets.
    fn rename_id(
        &mut self,
        def_path: &Path,
        def_line_0: u32,
        val_range: (u32, u32),
        old_id: &str,
        new_id: &str,
    ) {
        let def_uri = self.resolve_uri(def_path);
        self.changes.entry(def_uri).or_default().push(json!({
            "range": lsp_range(def_line_0, val_range.0, def_line_0, val_range.1),
            "newText": new_id
        }));

        for (ws_path, ws_src, _) in self.session.collect_workspace_documents() {
            let ws_uri = self.resolve_uri(&ws_path);
            let mut file_edits = Vec::new();

            for (l_idx, line) in ws_src.content.split('\n').enumerate() {
                update_id_links(line, l_idx as u32, old_id, new_id, &mut file_edits);
            }
            if !file_edits.is_empty() {
                self.changes.entry(ws_uri).or_default().extend(file_edits);
            }
        }
    }
}

/// Handles `textDocument/prepareRename` request.
fn handle_prepare_rename(session: &mut LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let pos = params
        .get("position")
        .ok_or_else(|| (-32602, "Missing position".to_string()))?;
    let line_0 = pos["line"].as_u64().unwrap_or(0) as u32;
    let char_0 = pos["character"].as_u64().unwrap_or(0) as u32;

    let Some(target) = identify_rename_target(session, &path, line_0, char_0) else {
        return Ok(Value::Null);
    };

    match target {
        RenameTarget::Heading {
            cursor_range,
            old_title,
            ..
        } => Ok(json!({
            "range": lsp_range(cursor_range.0, cursor_range.1, cursor_range.2, cursor_range.3),
            "placeholder": old_title
        })),
        RenameTarget::CustomId {
            cursor_range,
            old_cid,
            ..
        } => Ok(json!({
            "range": lsp_range(cursor_range.0, cursor_range.1, cursor_range.2, cursor_range.3),
            "placeholder": old_cid
        })),
        RenameTarget::Id {
            cursor_range,
            old_id,
            ..
        } => Ok(json!({
            "range": lsp_range(cursor_range.0, cursor_range.1, cursor_range.2, cursor_range.3),
            "placeholder": old_id
        })),
    }
}

/// Dispatches workspace edit generation for an identified rename target.
fn dispatch_rename_target(ctx: &mut RenameContext, target: RenameTarget, new_name: &str) {
    match target {
        RenameTarget::Heading {
            def_path,
            line_0: def_l0,
            old_title,
            id,
            ..
        } => {
            ctx.rename_heading(&def_path, def_l0, &old_title, id.as_deref(), new_name);
        }
        RenameTarget::CustomId {
            def_path,
            def_line_0: def_l0,
            val_range,
            old_cid,
            ..
        } => {
            ctx.rename_custom_id(&def_path, def_l0, val_range, &old_cid, new_name);
        }
        RenameTarget::Id {
            def_path,
            def_line_0: def_l0,
            val_range,
            old_id,
            ..
        } => {
            ctx.rename_id(&def_path, def_l0, val_range, &old_id, new_name);
        }
    }
}

/// Handles `textDocument/rename` request.
fn handle_rename(session: &mut LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (uri, path) = session.resolve_uri_and_path(params)?;
    let pos = params
        .get("position")
        .ok_or_else(|| (-32602, "Missing position".to_string()))?;
    let line_0 = pos["line"].as_u64().unwrap_or(0) as u32;
    let char_0 = pos["character"].as_u64().unwrap_or(0) as u32;
    let new_name = params
        .get("newName")
        .and_then(|n| n.as_str())
        .ok_or_else(|| (-32602, "Missing newName".to_string()))?;

    let Some(target) = identify_rename_target(session, &path, line_0, char_0) else {
        return Ok(json!({ "changes": {} }));
    };

    let mut changes: HashMap<String, Vec<Value>> = HashMap::new();
    let mut ctx = RenameContext {
        session,
        req_uri: &uri,
        req_path: &path,
        changes: &mut changes,
    };

    dispatch_rename_target(&mut ctx, target, new_name);
    Ok(json!({ "changes": changes }))
}

// endregion

// region Semantic Tokens

/// A raw semantic token before delta encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawSemanticToken {
    /// Zero-based line number.
    line: u32,
    /// Zero-based character offset within `line`.
    start_char: u32,
    /// Length of token in UTF-16 code units.
    length: u32,
    /// Token type index in legend.
    token_type: u32,
    /// Token modifier bitmask.
    token_modifiers: u32,
}

/// Emits semantic tokens for heading TODO keyword and priority cookie if present.
fn push_heading_keyword_token(
    line_idx: u32,
    raw_line: &str,
    kw: Option<&str>,
    pri: Option<char>,
    doc: &OrgDocument,
    tokens: &mut Vec<RawSemanticToken>,
) {
    if let Some(kw) = kw {
        let kw_idx = raw_line.find(kw).unwrap_or(0);
        let is_done =
            doc.todo_keywords.is_done(kw) || matches!(kw, "DONE" | "CANCELLED" | "CANCELED");
        tokens.push(RawSemanticToken {
            line: line_idx,
            start_char: utf16_len(&raw_line[..kw_idx]),
            length: utf16_len(kw),
            token_type: 0,                                // keyword
            token_modifiers: if is_done { 4 } else { 0 }, // readonly if done
        });
    }

    if let Some(pri) = pri {
        let pri_str = format!("[#{pri}]");
        if let Some(pri_idx) = raw_line.find(&pri_str) {
            tokens.push(RawSemanticToken {
                line: line_idx,
                start_char: utf16_len(&raw_line[..pri_idx]),
                length: utf16_len(&pri_str),
                token_type: 2, // type
                token_modifiers: 0,
            });
        }
    }
}

/// Emits semantic tokens for heading tags if present.
fn push_heading_tag_tokens(
    line_idx: u32,
    raw_line: &str,
    tags: &[&str],
    tokens: &mut Vec<RawSemanticToken>,
) {
    if tags.is_empty() {
        return;
    }
    let tag_str = format!(":{}:", tags.join(":"));
    if let Some(tag_idx) = raw_line.rfind(&tag_str) {
        tokens.push(RawSemanticToken {
            line: line_idx,
            start_char: utf16_len(&raw_line[..tag_idx]),
            length: utf16_len(&tag_str),
            token_type: 9, // decorator
            token_modifiers: 0,
        });
    }
}

/// Scans heading line components into semantic tokens.
fn scan_heading_tokens(
    line_idx: u32,
    raw_line: &str,
    doc: &OrgDocument,
    tokens: &mut Vec<RawSemanticToken>,
) {
    let kw_all = doc.todo_keywords.all();
    if let Some(parts) = parse_heading_with_keywords(raw_line, &kw_all) {
        tokens.push(RawSemanticToken {
            line: line_idx,
            start_char: 0,
            length: parts.level as u32,
            token_type: 1,      // namespace
            token_modifiers: 1, // declaration
        });

        push_heading_keyword_token(
            line_idx,
            raw_line,
            parts.keyword,
            parts.priority,
            doc,
            tokens,
        );

        let title_idx = raw_line.find(parts.title).unwrap_or(0);
        tokens.push(RawSemanticToken {
            line: line_idx,
            start_char: utf16_len(&raw_line[..title_idx]),
            length: utf16_len(parts.title),
            token_type: 1,      // namespace
            token_modifiers: 1, // declaration
        });

        push_heading_tag_tokens(line_idx, raw_line, &parts.tags, tokens);
    }
}

/// Scans drawer boundary delimiters or drawer property keys into semantic tokens.
fn scan_drawer_boundary_or_property(
    line_idx: u32,
    trimmed: &str,
    indent_u16: u32,
    tokens: &mut Vec<RawSemanticToken>,
) -> bool {
    if matches!(
        trimmed.to_ascii_uppercase().as_str(),
        ":PROPERTIES:" | ":LOGBOOK:" | ":END:"
    ) {
        tokens.push(RawSemanticToken {
            line: line_idx,
            start_char: indent_u16,
            length: utf16_len(trimmed),
            token_type: 8, // macro
            token_modifiers: 0,
        });
        return true;
    }

    if trimmed.starts_with(':') && trimmed.len() > 1 {
        if let Some(end_colon) = trimmed[1..].find(':') {
            let key = &trimmed[..=end_colon + 1];
            tokens.push(RawSemanticToken {
                line: line_idx,
                start_char: indent_u16,
                length: utf16_len(key),
                token_type: 3, // property
                token_modifiers: 0,
            });
            return true;
        }
    }

    false
}

/// Scans comments, directives, and planning lines into semantic tokens.
fn scan_directive_or_planning(
    line_idx: u32,
    raw_line: &str,
    tokens: &mut Vec<RawSemanticToken>,
) -> bool {
    let trimmed = raw_line.trim_start();
    let indent_u16 = utf16_len(&raw_line[..raw_line.len() - trimmed.len()]);

    if trimmed.starts_with("# ") || trimmed == "#" {
        tokens.push(RawSemanticToken {
            line: line_idx,
            start_char: indent_u16,
            length: utf16_len(trimmed),
            token_type: 7, // comment
            token_modifiers: 0,
        });
        return true;
    }

    if trimmed.starts_with("#+") {
        let first_word = trimmed.split_whitespace().next().unwrap_or(trimmed);
        tokens.push(RawSemanticToken {
            line: line_idx,
            start_char: indent_u16,
            length: utf16_len(first_word),
            token_type: 8, // macro
            token_modifiers: 0,
        });
        return true;
    }

    scan_drawer_boundary_or_property(line_idx, trimmed, indent_u16, tokens)
}

/// Scans planning keywords, timestamps, statistics cookies, and links on a line.
fn scan_prose_or_planning_tokens(
    line_idx: u32,
    raw_line: &str,
    tokens: &mut Vec<RawSemanticToken>,
) {
    for kw in &["SCHEDULED:", "DEADLINE:", "CLOSED:"] {
        let mut search_pos = 0;
        while let Some(rel) = raw_line[search_pos..].find(kw) {
            let kw_idx = search_pos + rel;
            tokens.push(RawSemanticToken {
                line: line_idx,
                start_char: utf16_len(&raw_line[..kw_idx]),
                length: utf16_len(kw),
                token_type: 0, // keyword
                token_modifiers: 0,
            });
            search_pos = kw_idx + kw.len();
        }
    }

    for link in scan_bracket_links(raw_line) {
        tokens.push(RawSemanticToken {
            line: line_idx,
            start_char: link.link_start_u16,
            length: link.link_end_u16.saturating_sub(link.link_start_u16),
            token_type: 4, // string
            token_modifiers: 0,
        });
    }

    scan_timestamps_and_cookies(line_idx, raw_line, tokens);
}

/// Checks if `rest` starts with a timestamp `<...>` and returns its byte length.
fn try_scan_timestamp(rest: &str) -> Option<usize> {
    if !rest.starts_with('<') {
        return None;
    }
    let close = rest.find('>')?;
    let candidate = &rest[..=close];
    if candidate.len() >= 12 && candidate.chars().nth(5) == Some('-') {
        Some(close + 1)
    } else {
        None
    }
}

/// Checks if `rest` starts with a statistics cookie `[n/m]` or `[n%]` and returns its byte length.
fn try_scan_cookie(rest: &str) -> Option<usize> {
    if !rest.starts_with('[') || rest.starts_with("[[") {
        return None;
    }
    let close = rest.find(']')?;
    let candidate = &rest[..=close];
    if candidate.contains('/') || candidate.contains('%') {
        Some(close + 1)
    } else {
        None
    }
}

/// Scans timestamps `<...>`, `[...]` and statistics cookies `[n/m]`, `[n%]`.
fn scan_timestamps_and_cookies(line_idx: u32, raw_line: &str, tokens: &mut Vec<RawSemanticToken>) {
    let mut pos = 0;
    while pos < raw_line.len() {
        let rest = &raw_line[pos..];
        if let Some(len) = try_scan_timestamp(rest) {
            tokens.push(RawSemanticToken {
                line: line_idx,
                start_char: utf16_len(&raw_line[..pos]),
                length: utf16_len(&rest[..len]),
                token_type: 5, // number
                token_modifiers: 0,
            });
            pos += len;
            continue;
        }
        if let Some(len) = try_scan_cookie(rest) {
            tokens.push(RawSemanticToken {
                line: line_idx,
                start_char: utf16_len(&raw_line[..pos]),
                length: utf16_len(&rest[..len]),
                token_type: 5,      // number
                token_modifiers: 4, // readonly
            });
            pos += len;
            continue;
        }
        pos += rest.chars().next().map_or(1, char::len_utf8);
    }
}

/// Delta-encodes an ordered list of raw semantic tokens.
fn delta_encode_tokens(mut tokens: Vec<RawSemanticToken>) -> Vec<u32> {
    tokens.sort_by_key(|a| (a.line, a.start_char));
    let mut non_overlapping: Vec<RawSemanticToken> = Vec::with_capacity(tokens.len());
    for tok in tokens {
        if let Some(last) = non_overlapping.last() {
            if last.line == tok.line && tok.start_char < last.start_char + last.length {
                continue;
            }
        }
        non_overlapping.push(tok);
    }

    let mut data = Vec::with_capacity(non_overlapping.len() * 5);
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;

    for (i, tok) in non_overlapping.into_iter().enumerate() {
        let (delta_line, delta_start) = if i == 0 {
            (tok.line, tok.start_char)
        } else if tok.line == prev_line {
            (0, tok.start_char.saturating_sub(prev_start))
        } else {
            (tok.line.saturating_sub(prev_line), tok.start_char)
        };
        data.push(delta_line);
        data.push(delta_start);
        data.push(tok.length);
        data.push(tok.token_type);
        data.push(tok.token_modifiers);
        prev_line = tok.line;
        prev_start = tok.start_char;
    }
    data
}

/// Handles `textDocument/semanticTokens/full` request.
fn handle_semantic_tokens_full(
    session: &mut LspSession,
    params: &Value,
) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let (source, doc) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;

    let protected = protected_regions(&source.content);
    let mut raw_tokens = Vec::new();

    for (idx, line) in source.content.split('\n').enumerate() {
        let l0 = idx as u32;
        let is_interior = protected.iter().any(|&(s, e)| idx > s && idx < e);
        if is_interior {
            continue;
        }

        if line.starts_with('*') {
            scan_heading_tokens(l0, line, &doc, &mut raw_tokens);
            continue;
        }

        if !scan_directive_or_planning(l0, line, &mut raw_tokens) {
            scan_prose_or_planning_tokens(l0, line, &mut raw_tokens);
        }
    }

    let data = delta_encode_tokens(raw_tokens);
    Ok(json!({ "data": data }))
}

// endregion

/// Handles incoming LSP rename and semantic token requests.
pub(crate) fn handle_refactor_tokens_request(
    session: &mut LspSession,
    method: &str,
    params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "textDocument/prepareRename" => handle_prepare_rename(session, params),
        "textDocument/rename" => handle_rename(session, params),
        "textDocument/semanticTokens/full" => handle_semantic_tokens_full(session, params),
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}
