// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=900 reason="Cohesive LSP navigation handlers for symbols, links, definitions, references, and folding ranges"

//! LSP navigation dispatch and handlers for symbols, links, definitions, references, and folding.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use org_tools_core::document::{OrgDocument, OrgEntry};
use org_tools_core::export_md::normalize_workspace_path;
use org_tools_core::include::parse_include_directive;
use org_tools_core::rules::format::regions::{is_protected, protected_regions};
use org_tools_core::source::SourceFile;

use super::{lsp_range, path_to_file_uri, LspSession};

/// Handles incoming LSP navigation requests.
pub(crate) fn handle_navigation_request(
    session: &mut LspSession,
    method: &str,
    params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "textDocument/documentSymbol" => handle_document_symbol(session, params),
        "workspace/symbol" => handle_workspace_symbol(session, params),
        "textDocument/documentLink" => handle_document_link(session, params),
        "textDocument/definition" => handle_definition(session, params),
        "textDocument/references" => handle_references(session, params),
        "textDocument/foldingRange" => handle_folding_range(session, params),
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}

// region Document & Workspace Symbols

/// Calculates the UTF-16 character length of a zero-based line index.
fn line_len_utf16(source: &SourceFile, line_0: u32) -> u32 {
    let idx = line_0 as usize;
    if idx >= source.line_count() {
        return 0;
    }
    let s = source.line_start(idx);
    let e = if idx + 1 < source.line_count() {
        source.line_start(idx + 1)
    } else {
        source.content.len()
    };
    let text = &source.content[s..e];
    let trimmed = text.trim_end_matches(['\r', '\n']);
    trimmed.chars().map(|c| c.len_utf16() as u32).sum()
}

/// Converts a byte offset within a line to a zero-based UTF-16 character index.
fn byte_to_utf16_char_in_line(line: &str, byte_offset: usize) -> u32 {
    let clamped = byte_offset.min(line.len());
    line[..clamped].chars().map(|c| c.len_utf16() as u32).sum()
}

/// Formats the detail description for an entry symbol from TODO, priority, and tags.
fn format_entry_detail(entry: &OrgEntry) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(ref kw) = entry.keyword {
        parts.push(kw.clone());
    }
    if let Some(p) = entry.priority {
        parts.push(format!("[#{p}]"));
    }
    if !entry.tags.is_empty() {
        parts.push(format!(":{}:", entry.tags.join(":")));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// Recursively builds a `DocumentSymbol` JSON object for an entry and its children.
fn build_entry_symbol(source: &SourceFile, doc: &OrgDocument, idx: usize) -> Value {
    let entry = &doc.entries[idx];
    let heading_l0 = entry.heading_line.saturating_sub(1) as u32;
    let heading_line_len = line_len_utf16(source, heading_l0);
    let max_l = source.line_count().saturating_sub(1);
    let end_l0 = entry
        .subtree_end_line
        .saturating_sub(1)
        .min(max_l)
        .max(heading_l0 as usize) as u32;
    let end_line_len = line_len_utf16(source, end_l0);

    let children: Vec<Value> = entry
        .children
        .iter()
        .map(|&c_idx| build_entry_symbol(source, doc, c_idx))
        .collect();

    let mut sym = json!({
        "name": entry.title,
        "kind": 15,
        "range": lsp_range(heading_l0, 0, end_l0, end_line_len),
        "selectionRange": lsp_range(heading_l0, 0, heading_l0, heading_line_len),
        "children": children,
    });
    if let Some(det) = format_entry_detail(entry) {
        sym["detail"] = Value::String(det);
    }
    sym
}

/// Handles `textDocument/documentSymbol` requests.
fn handle_document_symbol(session: &LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let (source, doc) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;

    let mut symbols = Vec::new();

    if let Some(file_id) = doc.file_id() {
        symbols.push(json!({
            "name": doc.file_title_or_stem(),
            "detail": format!(":ID: {file_id}"),
            "kind": 1,
            "range": lsp_range(0, 0, 0, 0),
            "selectionRange": lsp_range(0, 0, 0, 0),
        }));
    }

    for (idx, entry) in doc.entries.iter().enumerate() {
        if entry.parent.is_none() {
            symbols.push(build_entry_symbol(&source, &doc, idx));
        }
    }

    Ok(Value::Array(symbols))
}

/// Returns `true` when `haystack` contains `needle` ignoring ASCII case.
fn matches_query(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

/// Appends a file-level node symbol to `out` when it matches `query`.
fn check_file_node_symbol(path: &Path, doc: &OrgDocument, query: &str, out: &mut Vec<Value>) {
    let file_title = doc.file_title_or_stem();
    let file_id = doc.file_id().unwrap_or("");
    let file_alias = doc
        .file_properties
        .get("ROAM_ALIASES")
        .map(String::as_str)
        .unwrap_or("");

    if matches_query(&file_title, query)
        || matches_query(file_id, query)
        || matches_query(file_alias, query)
    {
        out.push(json!({
            "name": file_title,
            "kind": 1,
            "location": { "uri": path_to_file_uri(path), "range": lsp_range(0, 0, 0, 0) }
        }));
    }
}

/// Appends matching heading entry symbols from `doc` to `out`.
fn check_entries_symbols(
    path: &Path,
    source: &SourceFile,
    doc: &OrgDocument,
    query: &str,
    out: &mut Vec<Value>,
) {
    let file_uri = path_to_file_uri(path);
    for entry in &doc.entries {
        let entry_id = entry.properties.get("ID").map(String::as_str).unwrap_or("");
        let entry_cid = entry
            .properties
            .get("CUSTOM_ID")
            .map(String::as_str)
            .unwrap_or("");
        let entry_alias = entry
            .properties
            .get("ROAM_ALIASES")
            .map(String::as_str)
            .unwrap_or("");

        if matches_query(&entry.title, query)
            || matches_query(entry_id, query)
            || matches_query(entry_cid, query)
            || matches_query(entry_alias, query)
        {
            let l0 = entry.heading_line.saturating_sub(1) as u32;
            let len = line_len_utf16(source, l0);
            out.push(json!({
                "name": entry.title,
                "kind": 15,
                "location": { "uri": file_uri, "range": lsp_range(l0, 0, l0, len) }
            }));
        }
    }
}

/// Appends matching symbols from a single document to the workspace results.
fn collect_doc_symbols(
    path: &Path,
    source: &SourceFile,
    doc: &OrgDocument,
    query: &str,
    out: &mut Vec<Value>,
) {
    check_file_node_symbol(path, doc, query, out);
    check_entries_symbols(path, source, doc, query, out);
}

/// Handles `workspace/symbol` requests.
fn handle_workspace_symbol(session: &LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let query = params.get("query").and_then(|q| q.as_str()).unwrap_or("");
    let all_docs = session.collect_workspace_documents();
    let mut symbols = Vec::new();

    for (path, source, doc) in &all_docs {
        collect_doc_symbols(path, source, doc, query, &mut symbols);
    }

    Ok(Value::Array(symbols))
}

// endregion

// region Links & Definitions

/// A resolved link occurrence within a source document.
struct ResolvedDocLink {
    /// Zero-based line number containing the link.
    range_line_0: u32,
    /// Zero-based UTF-16 character index where the link begins.
    start_char: u32,
    /// Zero-based UTF-16 character index where the link ends.
    end_char: u32,
    /// Absolute or normalized path to the target file.
    target_path: PathBuf,
    /// Zero-based target line number.
    target_line_0: u32,
    /// UTF-16 character length of the target line.
    target_line_len: u32,
}

/// Resolves an anchor string (`#custom-id`, `*heading`, or line number) within a document.
fn find_anchor_line(doc: &OrgDocument, anchor: &str) -> u32 {
    if let Some(cid) = anchor.strip_prefix('#') {
        if let Some(idx) = doc.find_by_custom_id(cid) {
            return doc.entries[idx].heading_line.saturating_sub(1) as u32;
        }
    } else if let Some(title) = anchor.strip_prefix('*') {
        if let Some(entry) = doc.entries.iter().find(|e| e.title == title) {
            return entry.heading_line.saturating_sub(1) as u32;
        }
    } else if let Ok(line_num) = anchor.parse::<usize>() {
        return line_num.saturating_sub(1) as u32;
    }
    0
}

/// Resolves an `id:<uuid>` link target across all workspace documents.
fn resolve_id_target(session: &LspSession, uuid: &str) -> Option<(PathBuf, u32, u32)> {
    for (path, src, doc) in session.collect_workspace_documents() {
        if doc.file_id() == Some(uuid) {
            let l0 = doc
                .file_properties_line
                .map(|l| l.saturating_sub(1) as u32)
                .unwrap_or(0);
            return Some((path, l0, line_len_utf16(&src, l0)));
        }
        if let Some(idx) = doc.find_by_id(uuid) {
            let l0 = doc.entries[idx].heading_line.saturating_sub(1) as u32;
            return Some((path, l0, line_len_utf16(&src, l0)));
        }
    }
    None
}

/// Resolves a file-scoped link target (`file:<path>::<anchor>`).
fn resolve_file_target(
    session: &LspSession,
    current_file: &Path,
    file_spec: &str,
) -> Option<(PathBuf, u32, u32)> {
    let (rel_path, anchor) = match file_spec.split_once("::") {
        Some((p, a)) => (p.trim(), Some(a.trim())),
        None => (file_spec.trim(), None),
    };
    let parent = current_file.parent().unwrap_or(&session.default_workspace);
    let target_path = normalize_workspace_path(&parent.join(rel_path));
    if let Ok((src, doc)) = session.get_or_load_document(&target_path) {
        let l0 = anchor.map(|anc| find_anchor_line(&doc, anc)).unwrap_or(0);
        let len = line_len_utf16(&src, l0);
        return Some((target_path, l0, len));
    }
    None
}

/// Resolves a raw link target string to a concrete file and line coordinates.
fn resolve_link_target(
    session: &LspSession,
    current_file: &Path,
    raw_target: &str,
) -> Option<(PathBuf, u32, u32)> {
    if let Some(uuid) = raw_target.strip_prefix("id:") {
        return resolve_id_target(session, uuid.trim());
    }
    if let Some(file_spec) = raw_target.strip_prefix("file:") {
        return resolve_file_target(session, current_file, file_spec);
    }
    if let Some(title) = raw_target.strip_prefix("roam:") {
        return resolve_id_target(session, title.trim()).or_else(|| {
            for (path, src, doc) in session.collect_workspace_documents() {
                if doc.file_title_or_stem().eq_ignore_ascii_case(title) {
                    return Some((path, 0, line_len_utf16(&src, 0)));
                }
            }
            None
        });
    }
    if raw_target.starts_with('#') {
        if let Ok((src, doc)) = session.get_or_load_document(current_file) {
            let l0 = find_anchor_line(&doc, raw_target);
            return Some((current_file.to_path_buf(), l0, line_len_utf16(&src, l0)));
        }
    }
    if raw_target.starts_with('*') {
        if let Ok((src, doc)) = session.get_or_load_document(current_file) {
            let l0 = find_anchor_line(&doc, raw_target);
            return Some((current_file.to_path_buf(), l0, line_len_utf16(&src, l0)));
        }
    }
    resolve_file_target(session, current_file, raw_target)
}

/// Parses all `[[target]]` or `[[target][desc]]` spans within a single line.
fn scan_line_bracket_links(line: &str) -> Vec<(usize, usize, String)> {
    let mut links = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(rel) = line[i + 2..].find("]]") {
                let end = i + 2 + rel + 2;
                let inner = &line[i + 2..i + 2 + rel];
                let target = match inner.split_once("][") {
                    Some((t, _)) => t.trim().to_string(),
                    None => inner.trim().to_string(),
                };
                links.push((i, end, target));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    links
}

/// Resolves an `#+INCLUDE:` line into a [`ResolvedDocLink`] when valid.
fn process_include_line(
    session: &LspSession,
    parent: &Path,
    source: &SourceFile,
    line_str: &str,
    l0: u32,
) -> Option<ResolvedDocLink> {
    let directive = parse_include_directive(line_str)?.ok()?;
    let target_path = normalize_workspace_path(&parent.join(&directive.raw_path));
    let (target_src, target_doc) = session.get_or_load_document(&target_path).ok()?;
    let target_l0 = directive
        .search_option
        .as_deref()
        .map(|opt| find_anchor_line(&target_doc, opt))
        .unwrap_or(0);

    Some(ResolvedDocLink {
        range_line_0: l0,
        start_char: 0,
        end_char: line_len_utf16(source, l0),
        target_path,
        target_line_0: target_l0,
        target_line_len: line_len_utf16(&target_src, target_l0),
    })
}

/// Resolves bracket link occurrences within a line into [`ResolvedDocLink`] items.
fn process_bracket_links(
    session: &LspSession,
    current_file: &Path,
    line_str: &str,
    l0: u32,
) -> Vec<ResolvedDocLink> {
    let mut out = Vec::new();
    for (byte_s, byte_e, target) in scan_line_bracket_links(line_str) {
        if let Some((target_path, target_l0, target_len)) =
            resolve_link_target(session, current_file, &target)
        {
            out.push(ResolvedDocLink {
                range_line_0: l0,
                start_char: byte_to_utf16_char_in_line(line_str, byte_s),
                end_char: byte_to_utf16_char_in_line(line_str, byte_e),
                target_path,
                target_line_0: target_l0,
                target_line_len: target_len,
            });
        }
    }
    out
}

/// Collects all resolved document links and `#+INCLUDE:` directives in a source file.
fn collect_document_links(
    session: &LspSession,
    current_file: &Path,
    source: &SourceFile,
) -> Vec<ResolvedDocLink> {
    let mut results = Vec::new();
    let protected = protected_regions(&source.content);
    let parent = current_file.parent().unwrap_or(&session.default_workspace);

    for line_idx in 0..source.line_count() {
        if is_protected(line_idx, &protected) {
            continue;
        }
        let s = source.line_start(line_idx);
        let e = if line_idx + 1 < source.line_count() {
            source.line_start(line_idx + 1)
        } else {
            source.content.len()
        };
        let line_str = &source.content[s..e];
        let l0 = line_idx as u32;

        if let Some(inc_link) = process_include_line(session, parent, source, line_str, l0) {
            results.push(inc_link);
        }
        results.extend(process_bracket_links(session, current_file, line_str, l0));
    }
    results
}

/// Handles `textDocument/documentLink` requests.
fn handle_document_link(session: &LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let (source, _) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;

    let links = collect_document_links(session, &path, &source);
    let result_links: Vec<Value> = links
        .into_iter()
        .map(|link| {
            json!({
                "range": lsp_range(link.range_line_0, link.start_char, link.range_line_0, link.end_char),
                "target": path_to_file_uri(&link.target_path)
            })
        })
        .collect();

    Ok(Value::Array(result_links))
}

/// Handles `textDocument/definition` requests.
fn handle_definition(session: &LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let (source, _) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;

    let pos = params
        .get("position")
        .ok_or_else(|| (-32602, "Missing position".to_string()))?;
    let req_l0 = pos["line"].as_u64().unwrap_or(0) as u32;
    let req_c0 = pos["character"].as_u64().unwrap_or(0) as u32;

    let links = collect_document_links(session, &path, &source);
    for link in links {
        if link.range_line_0 == req_l0 && req_c0 >= link.start_char && req_c0 <= link.end_char {
            return Ok(json!({
                "uri": path_to_file_uri(&link.target_path),
                "range": lsp_range(link.target_line_0, 0, link.target_line_0, link.target_line_len)
            }));
        }
    }

    Ok(Value::Null)
}

// endregion

// region References & Folding

/// Target entity type to find incoming references for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RefTarget {
    /// Global UUID target.
    Id(String),
    /// File-scoped or document `:CUSTOM_ID:` target.
    CustomId(String),
    /// Heading title target.
    Heading(String),
}

/// Extracts a property value matching `key` case-insensitively from a line.
fn extract_property_from_line(line: &str, key: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with(':') {
        return None;
    }
    let rest = &trimmed[1..];
    let (prop_name, prop_val) = rest.split_once(':')?;
    if prop_name.trim().eq_ignore_ascii_case(key) {
        let val = prop_val.trim();
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }
    None
}

/// Scans bracket links on `line_str` enclosing `char_0` for a reference target.
fn detect_target_from_links(line_str: &str, char_0: u32) -> Option<RefTarget> {
    for (bs, be, raw) in scan_line_bracket_links(line_str) {
        let sc = byte_to_utf16_char_in_line(line_str, bs);
        let ec = byte_to_utf16_char_in_line(line_str, be);
        if char_0 >= sc && char_0 <= ec {
            if let Some(uuid) = raw.strip_prefix("id:") {
                return Some(RefTarget::Id(uuid.trim().to_string()));
            }
            if let Some(cid) = raw.strip_prefix('#') {
                return Some(RefTarget::CustomId(cid.trim().to_string()));
            }
            if let Some(h) = raw.strip_prefix('*') {
                return Some(RefTarget::Heading(h.trim().to_string()));
            }
        }
    }
    None
}

/// Detects a reference target from line properties, drawer contents, or heading attributes.
fn detect_target_from_line_or_entry(
    doc: &OrgDocument,
    line_str: &str,
    line_idx: usize,
) -> Option<RefTarget> {
    if let Some(uuid) = extract_property_from_line(line_str, "ID") {
        return Some(RefTarget::Id(uuid));
    }
    if let Some(cid) = extract_property_from_line(line_str, "CUSTOM_ID") {
        return Some(RefTarget::CustomId(cid));
    }
    if let Some(entry) = doc.entries.iter().find(|e| e.heading_line == line_idx + 1) {
        if let Some(id) = entry.properties.get("ID") {
            return Some(RefTarget::Id(id.clone()));
        }
        if let Some(cid) = entry.properties.get("CUSTOM_ID") {
            return Some(RefTarget::CustomId(cid.clone()));
        }
        return Some(RefTarget::Heading(entry.title.clone()));
    }
    if doc.file_properties_line.is_some()
        && doc
            .entries
            .first()
            .is_none_or(|e| line_idx < e.heading_line)
    {
        if let Some(id) = doc.file_id() {
            return Some(RefTarget::Id(id.to_string()));
        }
    }
    None
}

/// Detects the target reference entity under the cursor position.
fn detect_ref_target_at_position(
    source: &SourceFile,
    doc: &OrgDocument,
    line_0: u32,
    char_0: u32,
) -> Option<RefTarget> {
    let l_idx = line_0 as usize;
    if l_idx >= source.line_count() {
        return None;
    }
    let s = source.line_start(l_idx);
    let e = if l_idx + 1 < source.line_count() {
        source.line_start(l_idx + 1)
    } else {
        source.content.len()
    };
    let line_str = &source.content[s..e];

    detect_target_from_links(line_str, char_0)
        .or_else(|| detect_target_from_line_or_entry(doc, line_str, l_idx))
}

/// Finds the declaration location for an `id:<uuid>` reference target.
fn find_id_declaration(session: &LspSession, uuid: &str) -> Option<Value> {
    for (path, src, doc) in session.collect_workspace_documents() {
        if doc.file_id() == Some(uuid) {
            let l0 = doc
                .file_properties_line
                .map(|l| l.saturating_sub(1) as u32)
                .unwrap_or(0);
            let len = line_len_utf16(&src, l0);
            return Some(json!({
                "uri": path_to_file_uri(&path),
                "range": lsp_range(l0, 0, l0, len)
            }));
        }
        if let Some(idx) = doc.find_by_id(uuid) {
            let l0 = doc.entries[idx].heading_line.saturating_sub(1) as u32;
            let len = line_len_utf16(&src, l0);
            return Some(json!({
                "uri": path_to_file_uri(&path),
                "range": lsp_range(l0, 0, l0, len)
            }));
        }
    }
    None
}

/// Finds the declaration location for a `:CUSTOM_ID:` reference target.
fn find_custom_id_declaration(session: &LspSession, cid: &str) -> Option<Value> {
    for (path, src, doc) in session.collect_workspace_documents() {
        if let Some(idx) = doc.find_by_custom_id(cid) {
            let l0 = doc.entries[idx].heading_line.saturating_sub(1) as u32;
            let len = line_len_utf16(&src, l0);
            return Some(json!({
                "uri": path_to_file_uri(&path),
                "range": lsp_range(l0, 0, l0, len)
            }));
        }
    }
    None
}

/// Finds the declaration location for a heading title reference target.
fn find_heading_declaration(session: &LspSession, title: &str) -> Option<Value> {
    for (path, src, doc) in session.collect_workspace_documents() {
        if let Some(entry) = doc.entries.iter().find(|e| e.title == title) {
            let l0 = entry.heading_line.saturating_sub(1) as u32;
            let len = line_len_utf16(&src, l0);
            return Some(json!({
                "uri": path_to_file_uri(&path),
                "range": lsp_range(l0, 0, l0, len)
            }));
        }
    }
    None
}

/// Finds the declaration location for a given reference target.
fn find_declaration_location(session: &LspSession, target: &RefTarget) -> Option<Value> {
    match target {
        RefTarget::Id(uuid) => find_id_declaration(session, uuid),
        RefTarget::CustomId(cid) => find_custom_id_declaration(session, cid),
        RefTarget::Heading(title) => find_heading_declaration(session, title),
    }
}

/// Returns `true` when a raw link string matches `target`.
fn match_link_target(raw: &str, target: &RefTarget) -> bool {
    match target {
        RefTarget::Id(uuid) => raw.strip_prefix("id:").is_some_and(|u| u.trim() == uuid),
        RefTarget::CustomId(cid) => raw.ends_with(&format!("#{cid}")) || raw == format!("#{cid}"),
        RefTarget::Heading(title) => {
            raw.ends_with(&format!("*{title}")) || raw == format!("*{title}")
        }
    }
}

/// Scans a single document for incoming link references matching `target`.
fn scan_doc_for_incoming_references(
    path: &Path,
    source: &SourceFile,
    target: &RefTarget,
    out: &mut Vec<Value>,
) {
    let uri = path_to_file_uri(path);
    let protected = protected_regions(&source.content);

    for l_idx in 0..source.line_count() {
        if is_protected(l_idx, &protected) {
            continue;
        }
        let s = source.line_start(l_idx);
        let e = if l_idx + 1 < source.line_count() {
            source.line_start(l_idx + 1)
        } else {
            source.content.len()
        };
        let line_str = &source.content[s..e];
        let l0 = l_idx as u32;

        for (bs, be, raw) in scan_line_bracket_links(line_str) {
            if match_link_target(&raw, target) {
                out.push(json!({
                    "uri": uri,
                    "range": lsp_range(
                        l0,
                        byte_to_utf16_char_in_line(line_str, bs),
                        l0,
                        byte_to_utf16_char_in_line(line_str, be),
                    )
                }));
            }
        }
    }
}

/// Handles `textDocument/references` requests.
fn handle_references(session: &LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let (source, doc) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;

    let pos = params
        .get("position")
        .ok_or_else(|| (-32602, "Missing position".to_string()))?;
    let req_l0 = pos["line"].as_u64().unwrap_or(0) as u32;
    let req_c0 = pos["character"].as_u64().unwrap_or(0) as u32;
    let include_decl = params
        .get("context")
        .and_then(|c| c.get("includeDeclaration"))
        .and_then(|d| d.as_bool())
        .unwrap_or(false);

    let Some(target) = detect_ref_target_at_position(&source, &doc, req_l0, req_c0) else {
        return Ok(Value::Array(Vec::new()));
    };

    let mut locations = Vec::new();
    if include_decl {
        if let Some(decl) = find_declaration_location(session, &target) {
            locations.push(decl);
        }
    }

    for (p, src, _) in session.collect_workspace_documents() {
        scan_doc_for_incoming_references(&p, &src, &target, &mut locations);
    }

    Ok(Value::Array(locations))
}

/// Collects folding ranges for all heading entry subtrees.
fn collect_heading_folds(doc: &OrgDocument, source: &SourceFile, out: &mut Vec<Value>) {
    let max_line = source.line_count().saturating_sub(1);
    for entry in &doc.entries {
        let s0 = entry.heading_line.saturating_sub(1) as u32;
        let mut e0 = entry.subtree_end_line.saturating_sub(1).min(max_line);
        while e0 > (s0 as usize) {
            let ls = source.line_start(e0);
            let le = if e0 + 1 < source.line_count() {
                source.line_start(e0 + 1)
            } else {
                source.content.len()
            };
            if source.content[ls..le].trim().is_empty() {
                e0 -= 1;
            } else {
                break;
            }
        }
        if e0 > (s0 as usize) {
            out.push(json!({
                "startLine": s0,
                "endLine": e0 as u32,
                "kind": "region"
            }));
        }
    }
}

/// Collects folding ranges for `:PROPERTIES:`, `:LOGBOOK:`, and `#+BEGIN_` blocks.
fn collect_drawer_and_block_folds(source: &SourceFile, out: &mut Vec<Value>) {
    let mut block_stack: Vec<(usize, String)> = Vec::new();
    let mut drawer_stack: Vec<usize> = Vec::new();

    for l_idx in 0..source.line_count() {
        let s = source.line_start(l_idx);
        let e = if l_idx + 1 < source.line_count() {
            source.line_start(l_idx + 1)
        } else {
            source.content.len()
        };
        let trimmed = source.content[s..e].trim();

        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") || trimmed.eq_ignore_ascii_case(":LOGBOOK:")
        {
            drawer_stack.push(l_idx);
        } else if trimmed.eq_ignore_ascii_case(":END:") {
            if let Some(start_l) = drawer_stack.pop() {
                if l_idx > start_l {
                    out.push(json!({
                        "startLine": start_l as u32,
                        "endLine": l_idx as u32,
                        "kind": "region"
                    }));
                }
            }
        } else if trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("#+BEGIN_") {
            let name = trimmed[8..].trim().to_ascii_uppercase();
            block_stack.push((l_idx, name));
        } else if trimmed.len() >= 6 && trimmed[..6].eq_ignore_ascii_case("#+END_") {
            if let Some((start_l, name)) = block_stack.pop() {
                if l_idx > start_l {
                    let kind = if name == "COMMENT" {
                        "comment"
                    } else {
                        "region"
                    };
                    out.push(json!({
                        "startLine": start_l as u32,
                        "endLine": l_idx as u32,
                        "kind": kind
                    }));
                }
            }
        }
    }
}

/// Handles `textDocument/foldingRange` requests.
fn handle_folding_range(session: &LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let (source, doc) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;

    let mut ranges = Vec::new();
    collect_heading_folds(&doc, &source, &mut ranges);
    collect_drawer_and_block_folds(&source, &mut ranges);

    Ok(Value::Array(ranges))
}

// endregion
