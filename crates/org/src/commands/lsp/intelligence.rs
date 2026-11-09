// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=900 reason="Cohesive LSP context-aware completion and rich hover preview intelligence"

//! LSP context-aware completion (node links, keywords, tags, properties) and rich hover previews.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

use org_tools_core::document::{OrgDocument, OrgEntry};
use org_tools_core::edna::{blocking_details, EdnaContext};
use org_tools_core::export_md::export_slice_to_markdown;
use org_tools_core::roam::{RoamIndex, RoamNode};
use org_tools_core::rules::heading::{TagMember, TagSpec, DEFAULT_TODO_KEYWORDS};
use org_tools_core::source::SourceFile;
use org_tools_core::tblfm::calc_file;

use super::{byte_offset_to_utf16_pos, lsp_range, utf16_pos_to_byte_offset, LspSession};

// region Completion Intelligence

/// Canonical Org directives completed after `#+`.
const CANONICAL_DIRECTIVES: &[&str] = &[
    "#+TITLE:",
    "#+AUTHOR:",
    "#+DATE:",
    "#+FILETAGS:",
    "#+TAGS:",
    "#+TODO:",
    "#+SEQ_TODO:",
    "#+TYP_TODO:",
    "#+PRIORITIES:",
    "#+STARTUP:",
    "#+ARCHIVE:",
    "#+COLUMNS:",
    "#+INCLUDE:",
    "#+SETUPFILE:",
    "#+PROPERTY:",
    "#+TBLFM:",
    "#+BEGIN_SRC",
    "#+BEGIN_QUOTE",
    "#+BEGIN_EXAMPLE",
];

/// Standard property keys completed inside `:PROPERTIES:` drawers.
const STANDARD_PROPERTIES: &[&str] = &[
    ":ID:",
    ":ID_SEED:",
    ":CUSTOM_ID:",
    ":ROAM_ALIASES:",
    ":ROAM_REFS:",
    ":ROAM_EXCLUDE:",
    ":CATEGORY:",
    ":EFFORT:",
    ":LOGGING:",
    ":STYLE:",
    ":BLOCKER:",
    ":TRIGGER:",
    ":ARCHIVE:",
    ":COLUMNS:",
];

/// Formats relative file path and tags detail string for a Roam node.
fn format_node_detail(node: &RoamNode, session: &LspSession) -> String {
    let node_path = Path::new(&node.file);
    let rel_path = node_path
        .strip_prefix(&session.default_workspace)
        .or_else(|_| node_path.strip_prefix(&session.display_workspace))
        .unwrap_or(node_path)
        .to_string_lossy();
    if node.tags.is_empty() {
        rel_path.to_string()
    } else {
        format!("{rel_path} :{}:", node.tags.join(":"))
    }
}

/// Builds a completion item for a primary Roam node target.
fn node_to_completion_item(
    node: &RoamNode,
    detail: &str,
    line_0: u32,
    bracket_c: u32,
    end_c: u32,
) -> Value {
    let filter_text = format!("id:{} {} {}", node.id, node.title, node.aliases.join(" "));
    json!({
        "label": node.title,
        "kind": 18,
        "detail": detail,
        "filterText": filter_text,
        "textEdit": {
            "range": lsp_range(line_0, bracket_c, line_0, end_c),
            "newText": format!("[[id:{}][{}]]", node.id, node.title)
        }
    })
}

/// Builds a completion item for a Roam node alias target.
fn alias_to_completion_item(
    node: &RoamNode,
    alias: &str,
    detail: &str,
    line_0: u32,
    bracket_c: u32,
    end_c: u32,
) -> Value {
    let filter_text = format!("id:{} {} {}", node.id, alias, node.title);
    json!({
        "label": alias,
        "kind": 18,
        "detail": format!("{detail} (alias)"),
        "filterText": filter_text,
        "textEdit": {
            "range": lsp_range(line_0, bracket_c, line_0, end_c),
            "newText": format!("[[id:{}][{}]]", node.id, alias)
        }
    })
}

/// Completes node link targets for `[[id:...` or `[[roam:...`.
fn complete_node_links(
    session: &LspSession,
    line_0: u32,
    bracket_c: u32,
    end_c: u32,
    query_prefix: &str,
) -> Value {
    let ws_docs = session.collect_workspace_documents();
    let sources: Vec<SourceFile> = ws_docs.into_iter().map(|(_, s, _)| s).collect();
    let index = RoamIndex::from_sources(&sources);
    let q_lower = query_prefix.trim().to_lowercase();
    let mut items = Vec::new();

    for node in &index.nodes {
        let matches_node = q_lower.is_empty()
            || node.id.to_lowercase().contains(&q_lower)
            || node.title.to_lowercase().contains(&q_lower)
            || node
                .aliases
                .iter()
                .any(|a| a.to_lowercase().contains(&q_lower));
        if !matches_node {
            continue;
        }
        let detail = format_node_detail(node, session);
        items.push(node_to_completion_item(
            node, &detail, line_0, bracket_c, end_c,
        ));
        for alias in &node.aliases {
            if q_lower.is_empty() || alias.to_lowercase().contains(&q_lower) {
                items.push(alias_to_completion_item(
                    node, alias, &detail, line_0, bracket_c, end_c,
                ));
            }
        }
    }
    json!({ "isIncomplete": false, "items": items })
}

/// Completes `#+` file and block directives.
fn complete_directives(line_0: u32, start_c: u32, cursor_c: u32, prefix: &str) -> Value {
    let p_lower = prefix.to_lowercase();
    let items: Vec<Value> = CANONICAL_DIRECTIVES
        .iter()
        .filter(|dir| p_lower.is_empty() || dir.to_lowercase().starts_with(&p_lower))
        .map(|dir| {
            json!({
                "label": *dir,
                "kind": 14,
                "textEdit": {
                    "range": lsp_range(line_0, start_c, line_0, cursor_c),
                    "newText": *dir
                }
            })
        })
        .collect();
    json!({ "isIncomplete": false, "items": items })
}

/// Appends declared tags from a [`TagSpec`] into `tags`.
fn collect_tags_from_spec(spec: &TagSpec, tags: &mut BTreeSet<String>) {
    for def in &spec.standalone {
        tags.insert(def.name.clone());
    }
    for group in &spec.groups {
        if let Some(ref gt) = group.group_tag {
            tags.insert(gt.name.clone());
        }
        for member in &group.members {
            if let TagMember::Literal(ref def) = member {
                tags.insert(def.name.clone());
            }
        }
    }
}

/// Collects all unique tag identifiers across the session workspace.
fn collect_workspace_tags(session: &LspSession, doc: &OrgDocument) -> BTreeSet<String> {
    let mut tags = BTreeSet::new();
    for t in &doc.filetags {
        tags.insert(t.clone());
    }
    collect_tags_from_spec(&doc.tag_spec, &mut tags);
    for (_, _, ws_doc) in session.collect_workspace_documents() {
        for t in &ws_doc.filetags {
            tags.insert(t.clone());
        }
        collect_tags_from_spec(&ws_doc.tag_spec, &mut tags);
        for entry in &ws_doc.entries {
            for t in &entry.tags {
                tags.insert(t.clone());
            }
        }
    }
    tags
}

/// Completes heading `:tag:` chains.
fn complete_tags(
    session: &LspSession,
    doc: &OrgDocument,
    line_0: u32,
    start_c: u32,
    end_c: u32,
    prefix: &str,
) -> Value {
    let tags = collect_workspace_tags(session, doc);
    let p_lower = prefix.to_lowercase();
    let items: Vec<Value> = tags
        .into_iter()
        .filter(|t| p_lower.is_empty() || t.to_lowercase().contains(&p_lower))
        .map(|t| {
            json!({
                "label": format!(":{t}:"),
                "kind": 20,
                "textEdit": {
                    "range": lsp_range(line_0, start_c, line_0, end_c),
                    "newText": format!(":{t}:")
                }
            })
        })
        .collect();
    json!({ "isIncomplete": false, "items": items })
}

/// Collects all active TODO keywords matching `token`.
fn collect_active_todo_items(
    doc: &OrgDocument,
    line_0: u32,
    start_c: u32,
    cursor_c: u32,
    token: &str,
) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    let mut kws = vec!["TODO", "NEXT", "WAIT", "BLOCKED", "DONE"];
    for def in DEFAULT_TODO_KEYWORDS {
        if !kws.contains(def) {
            kws.push(def);
        }
    }
    for kw in doc.todo_keywords.all() {
        if !kws.contains(&kw) {
            kws.push(kw);
        }
    }
    let t_upper = token.to_uppercase();
    let mut items = Vec::new();
    for kw in kws {
        if (t_upper.is_empty() || kw.starts_with(&t_upper)) && seen.insert(kw) {
            items.push(json!({
                "label": kw,
                "kind": 14,
                "textEdit": {
                    "range": lsp_range(line_0, start_c, line_0, cursor_c),
                    "newText": kw
                }
            }));
        }
    }
    items
}

/// Completes heading TODO keywords and priority cookies.
fn complete_heading_todo_or_priorities(
    doc: &OrgDocument,
    line_0: u32,
    start_c: u32,
    cursor_c: u32,
    token: &str,
) -> Value {
    let mut items = Vec::new();
    let is_pri = token.starts_with("[#") || token.starts_with('[');

    if is_pri || token.is_empty() {
        for pri in &["[#A]", "[#B]", "[#C]"] {
            items.push(json!({
                "label": *pri,
                "kind": 12,
                "textEdit": {
                    "range": lsp_range(line_0, start_c, line_0, cursor_c),
                    "newText": *pri
                }
            }));
        }
    }
    if !is_pri {
        items.extend(collect_active_todo_items(
            doc, line_0, start_c, cursor_c, token,
        ));
    }
    json!({ "isIncomplete": false, "items": items })
}

/// Checks whether `line_0` is situated inside a `:PROPERTIES:` drawer.
fn is_inside_properties_drawer(source: &SourceFile, line_0: u32) -> bool {
    let idx = line_0 as usize;
    let mut found_open = false;
    for i in (0..idx).rev() {
        let s = source.line_start(i);
        let e = if i + 1 < source.line_count() {
            source.line_start(i + 1)
        } else {
            source.content.len()
        };
        let trimmed = source.content[s..e].trim();
        if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
            found_open = true;
            break;
        }
        if trimmed.starts_with('*') || trimmed.eq_ignore_ascii_case(":END:") {
            break;
        }
    }
    if !found_open {
        return false;
    }
    for i in (idx + 1)..source.line_count() {
        let s = source.line_start(i);
        let e = if i + 1 < source.line_count() {
            source.line_start(i + 1)
        } else {
            source.content.len()
        };
        let trimmed = source.content[s..e].trim();
        if trimmed.eq_ignore_ascii_case(":END:") {
            return true;
        }
        if trimmed.starts_with('*') || trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
            break;
        }
    }
    false
}

/// Completes standard property keys inside `:PROPERTIES:` drawers.
fn complete_drawer_properties(line_0: u32, start_c: u32, end_c: u32, prefix: &str) -> Value {
    let p_upper = prefix.to_uppercase();
    let items: Vec<Value> = STANDARD_PROPERTIES
        .iter()
        .filter(|prop| p_upper.is_empty() || prop.to_uppercase().starts_with(&p_upper))
        .map(|prop| {
            json!({
                "label": *prop,
                "kind": 10,
                "textEdit": {
                    "range": lsp_range(line_0, start_c, line_0, end_c),
                    "newText": *prop
                }
            })
        })
        .collect();
    json!({ "isIncomplete": false, "items": items })
}

/// Coordinate and slice context for completion evaluation.
struct CompletionContext<'a> {
    /// Full line text containing the cursor.
    full_line: &'a str,
    /// Slice of the line before the cursor.
    before_cursor: &'a str,
    /// Slice of the line after the cursor.
    after_cursor: &'a str,
    /// Zero-based line number.
    line_0: u32,
    /// Line start byte offset in the source document.
    line_start: usize,
    /// Cursor byte offset relative to the line start.
    cursor_rel: usize,
    /// Zero-based UTF-16 character position of the cursor.
    char_utf16_0: u32,
}

impl<'a> CompletionContext<'a> {
    /// Builds a completion context for `source` at `(line_0, char_utf16_0)`.
    fn from_position(source: &'a SourceFile, line_0: u32, char_utf16_0: u32) -> Self {
        let line_idx = line_0 as usize;
        let line_start = source.line_start(line_idx);
        let line_end = if line_idx + 1 < source.line_count() {
            source.line_start(line_idx + 1)
        } else {
            source.content.len()
        };
        let full_line = &source.content[line_start..line_end];
        let cursor_byte =
            utf16_pos_to_byte_offset(source, line_0, char_utf16_0).unwrap_or(line_start);
        let cursor_rel = cursor_byte.saturating_sub(line_start).min(full_line.len());
        let before_cursor = &full_line[..cursor_rel];
        let after_cursor = &full_line[cursor_rel..];
        Self {
            full_line,
            before_cursor,
            after_cursor,
            line_0,
            line_start,
            cursor_rel,
            char_utf16_0,
        }
    }
}

/// Tests and handles unclosed `[[` bracket link completion.
fn check_link_completion(
    session: &LspSession,
    source: &SourceFile,
    ctx: &CompletionContext<'_>,
) -> Option<Value> {
    let bracket_pos = ctx.before_cursor.rfind("[[")?;
    if ctx.before_cursor[bracket_pos..].contains("]]") {
        return None;
    }
    let link_body = &ctx.before_cursor[bracket_pos + 2..];
    let q_prefix = link_body
        .strip_prefix("id:")
        .or_else(|| link_body.strip_prefix("roam:"))
        .unwrap_or(link_body);
    let (_, b_c) = byte_offset_to_utf16_pos(source, ctx.line_start + bracket_pos);
    let end_rel = if let Some(close_idx) = ctx.after_cursor.find("]]") {
        let next_open = ctx.after_cursor.find("[[").unwrap_or(usize::MAX);
        if close_idx < next_open {
            ctx.cursor_rel + close_idx + 2
        } else {
            ctx.cursor_rel
        }
    } else {
        ctx.cursor_rel
    };
    let (_, end_c) = byte_offset_to_utf16_pos(source, ctx.line_start + end_rel);
    Some(complete_node_links(
        session, ctx.line_0, b_c, end_c, q_prefix,
    ))
}

/// Tests and handles `#+` directive completion.
fn check_directive_completion(source: &SourceFile, ctx: &CompletionContext<'_>) -> Option<Value> {
    let trimmed = ctx.before_cursor.trim_start();
    if !trimmed.starts_with("#+") {
        return None;
    }
    let start_b = ctx.before_cursor.len() - trimmed.len();
    let (_, start_c) = byte_offset_to_utf16_pos(source, ctx.line_start + start_b);
    Some(complete_directives(
        ctx.line_0,
        start_c,
        ctx.char_utf16_0,
        trimmed,
    ))
}

/// Tests and handles heading tags and TODO keyword completions.
fn check_heading_completion(
    session: &LspSession,
    doc: &OrgDocument,
    source: &SourceFile,
    ctx: &CompletionContext<'_>,
) -> Option<Value> {
    let star_count = ctx.full_line.chars().take_while(|&c| c == '*').count();
    let is_heading = star_count > 0 && ctx.full_line.as_bytes().get(star_count) == Some(&b' ');
    if !is_heading {
        return None;
    }
    let token = ctx
        .before_cursor
        .rsplit_once(|c: char| c.is_whitespace())
        .map_or(ctx.before_cursor, |(_, t)| t);
    if token.starts_with(':') {
        let last_colon = token.rfind(':').unwrap_or(0);
        let token_start_b = ctx.before_cursor.len() - token.len() + last_colon;
        let (_, start_c) = byte_offset_to_utf16_pos(source, ctx.line_start + token_start_b);
        let end_b = if ctx.after_cursor.starts_with(':') {
            ctx.cursor_rel + 1
        } else {
            ctx.cursor_rel
        };
        let (_, end_c) = byte_offset_to_utf16_pos(source, ctx.line_start + end_b);
        let prefix = &token[last_colon + 1..];
        return Some(complete_tags(
            session, doc, ctx.line_0, start_c, end_c, prefix,
        ));
    }
    let stars_len = star_count + 1;
    if ctx.cursor_rel >= stars_len {
        let token_start_b = ctx.before_cursor.len() - token.len();
        let (_, start_c) = byte_offset_to_utf16_pos(source, ctx.line_start + token_start_b);
        return Some(complete_heading_todo_or_priorities(
            doc,
            ctx.line_0,
            start_c,
            ctx.char_utf16_0,
            token,
        ));
    }
    None
}

/// Tests and handles property drawer key completions.
fn check_drawer_prop_completion(source: &SourceFile, ctx: &CompletionContext<'_>) -> Option<Value> {
    let trimmed = ctx.before_cursor.trim_start();
    if !is_inside_properties_drawer(source, ctx.line_0) || !trimmed.starts_with(':') {
        return None;
    }
    let start_b = ctx.before_cursor.len() - trimmed.len();
    let (_, start_c) = byte_offset_to_utf16_pos(source, ctx.line_start + start_b);
    let end_b = if ctx.after_cursor.starts_with(':') {
        ctx.cursor_rel + 1
    } else {
        ctx.cursor_rel
    };
    let (_, end_c) = byte_offset_to_utf16_pos(source, ctx.line_start + end_b);
    Some(complete_drawer_properties(
        ctx.line_0, start_c, end_c, trimmed,
    ))
}

/// Handles incoming `textDocument/completion` requests.
fn handle_completion(session: &mut LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let (source, doc) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;
    let pos = params
        .get("position")
        .ok_or_else(|| (-32602, "Missing position".to_string()))?;
    let line_0 = pos["line"].as_u64().unwrap_or(0) as u32;
    let char_utf16_0 = pos["character"].as_u64().unwrap_or(0) as u32;
    if (line_0 as usize) >= source.line_count() {
        return Ok(json!({ "isIncomplete": false, "items": [] }));
    }

    let ctx = CompletionContext::from_position(&source, line_0, char_utf16_0);

    if let Some(res) = check_link_completion(session, &source, &ctx) {
        return Ok(res);
    }
    if let Some(res) = check_directive_completion(&source, &ctx) {
        return Ok(res);
    }
    if let Some(res) = check_heading_completion(session, &doc, &source, &ctx) {
        return Ok(res);
    }
    if let Some(res) = check_drawer_prop_completion(&source, &ctx) {
        return Ok(res);
    }

    Ok(json!({ "isIncomplete": false, "items": [] }))
}

// endregion

// region Hover Intelligence

/// Finds the most deeply nested heading entry containing 1-based line number `line_1`.
fn find_entry_at_line(doc: &OrgDocument, line_1: usize) -> Option<usize> {
    doc.entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.heading_line <= line_1 && line_1 < e.subtree_end_line)
        .max_by_key(|(_, e)| e.heading_line)
        .map(|(idx, _)| idx)
}

/// Scans line for `[[...]]` links and extracts target under character position `char_utf16_0`.
fn find_link_target_at_cursor(
    source: &SourceFile,
    line_0: u32,
    char_utf16_0: u32,
) -> Option<String> {
    let line_idx = line_0 as usize;
    if line_idx >= source.line_count() {
        return None;
    }
    let s = source.line_start(line_idx);
    let e = if line_idx + 1 < source.line_count() {
        source.line_start(line_idx + 1)
    } else {
        source.content.len()
    };
    let line_str = &source.content[s..e];
    let mut i = 0;
    while let Some(open) = line_str[i..].find("[[") {
        let link_start = i + open;
        if let Some(close) = line_str[link_start..].find("]]") {
            let link_end = link_start + close + 2;
            let inner = &line_str[link_start + 2..link_start + close];
            let raw_target = inner.split_once("][").map_or(inner, |(t, _)| t);
            let (_, sc) = byte_offset_to_utf16_pos(source, s + link_start);
            let (_, ec) = byte_offset_to_utf16_pos(source, s + link_end);
            if char_utf16_0 >= sc && char_utf16_0 <= ec {
                return Some(raw_target.to_string());
            }
            i = link_end;
        } else {
            break;
        }
    }
    None
}

/// Renders a Markdown preview slice for the resolved target node.
fn render_node_preview_slice(session: &LspSession, node: &RoamNode) -> Option<String> {
    let (target_src, target_doc) = session.get_or_load_document(Path::new(&node.file)).ok()?;
    let entry_idx = find_entry_at_line(&target_doc, node.line)?;
    let entry = &target_doc.entries[entry_idx];
    let s_line = entry.heading_line.saturating_sub(1);
    let e_line = entry.content_end_line.min(s_line + 8);
    if s_line >= target_src.line_count() {
        return None;
    }
    let sb = target_src.line_start(s_line);
    let eb = if e_line < target_src.line_count() {
        target_src.line_start(e_line)
    } else {
        target_src.content.len()
    };
    let slice = &target_src.content[sb..eb];
    let rendered = export_slice_to_markdown(slice, Path::new(&node.file), &[]);
    (!rendered.trim().is_empty()).then_some(rendered)
}

/// Formats a rich Markdown hover preview card for a resolved Roam node.
fn format_node_hover_md(node: &RoamNode, backlinks_count: usize, preview: Option<&str>) -> String {
    let mut md = format!("**{}**\n\n", node.title);
    md.push_str(&format!("- **File**: `{}`\n", node.file));
    md.push_str(&format!("- **ID**: `{}`\n", node.id));
    if let Some(ref kw) = node.todo_keyword {
        md.push_str(&format!("- **TODO**: `{}`\n", kw));
    }
    if !node.tags.is_empty() {
        md.push_str(&format!("- **Tags**: `:{}:`\n", node.tags.join(":")));
    }
    md.push_str(&format!("- **Backlinks**: {backlinks_count}\n"));
    if let Some(body) = preview {
        md.push_str("\n---\n\n");
        md.push_str(body);
    }
    md
}

/// Resolves link under cursor and renders a Markdown preview card.
fn hover_link(
    session: &LspSession,
    source: &SourceFile,
    _current_doc: &OrgDocument,
    line_0: u32,
    char_utf16_0: u32,
) -> Option<Value> {
    let target = find_link_target_at_cursor(source, line_0, char_utf16_0)?;
    let ws_docs = session.collect_workspace_documents();
    let sources: Vec<SourceFile> = ws_docs.into_iter().map(|(_, s, _)| s).collect();
    let roam_index = RoamIndex::from_sources(&sources);

    let id_part = target
        .strip_prefix("id:")
        .or_else(|| target.strip_prefix("roam:"))
        .unwrap_or(&target);
    let clean_id = id_part.split_once("::").map_or(id_part, |(base, _)| base);

    let node = roam_index
        .nodes
        .iter()
        .find(|n| n.id.eq_ignore_ascii_case(clean_id))?;
    let backlinks = roam_index.backlinks_for(std::slice::from_ref(node));
    let preview = render_node_preview_slice(session, node);
    let md = format_node_hover_md(node, backlinks.len(), preview.as_deref());
    Some(json!({ "contents": { "kind": "markdown", "value": md } }))
}

/// Evaluates org-edna blockers and triggers for heading under cursor.
fn hover_edna(
    session: &LspSession,
    source: &SourceFile,
    doc: &OrgDocument,
    line_0: u32,
) -> Option<Value> {
    let line_idx = line_0 as usize;
    if line_idx >= source.line_count() {
        return None;
    }
    let s = source.line_start(line_idx);
    let e = if line_idx + 1 < source.line_count() {
        source.line_start(line_idx + 1)
    } else {
        source.content.len()
    };
    let line_str = source.content[s..e].trim();

    let entry_idx = find_entry_at_line(doc, line_idx + 1)?;
    let entry: &OrgEntry = &doc.entries[entry_idx];
    let is_heading = line_idx + 1 == entry.heading_line;
    let is_blocker_line = line_str.starts_with(":BLOCKER:");
    let is_trigger_line = line_str.starts_with(":TRIGGER:");

    if (!is_heading && !is_blocker_line && !is_trigger_line)
        || (!entry.properties.contains_key("BLOCKER") && !entry.properties.contains_key("TRIGGER"))
    {
        return None;
    }

    let ws_docs = session.collect_workspace_documents();
    let all_docs: Vec<&OrgDocument> = ws_docs.iter().map(|(_, _, d)| d).collect();
    let ctx = EdnaContext {
        all_docs: &all_docs,
        doc,
        entry_idx,
    };
    let details = blocking_details(&ctx);

    let mut md = String::from("### Edna Task Dependencies\n\n");
    if details.is_empty() {
        md.push_str("- **Edna Status**: READY\n");
    } else {
        md.push_str("- **Edna Status**: BLOCKED by:\n");
        for d in &details {
            let kw_str = d
                .keyword
                .as_deref()
                .map(|k| format!(" [{k}]"))
                .unwrap_or_default();
            md.push_str(&format!(
                "  - **{}**{} (`{}:{}`) — {}\n",
                d.title, kw_str, d.file, d.line, d.condition_desc
            ));
        }
    }
    if let Some(trigger) = entry.properties.get("TRIGGER") {
        md.push_str(&format!("\n- **Trigger Actions**: `{}`\n", trigger.trim()));
    }

    Some(json!({ "contents": { "kind": "markdown", "value": md } }))
}

/// Identifies top and bottom line indices of a table enclosing `line_idx`.
fn find_table_bounds(source: &SourceFile, line_idx: usize) -> (usize, usize) {
    let mut table_start = line_idx;
    while table_start > 0 {
        let s_prev = source.line_start(table_start - 1);
        let e_prev = source.line_start(table_start);
        let trimmed = source.content[s_prev..e_prev].trim();
        if trimmed.starts_with('|')
            || trimmed.starts_with("#+TBLFM:")
            || trimmed.starts_with("#+NAME:")
            || trimmed.starts_with("#+CAPTION:")
        {
            table_start -= 1;
        } else {
            break;
        }
    }
    let mut table_end = line_idx;
    while table_end + 1 < source.line_count() {
        let s_next = source.line_start(table_end + 1);
        let e_next = if table_end + 2 < source.line_count() {
            source.line_start(table_end + 2)
        } else {
            source.content.len()
        };
        let trimmed = source.content[s_next..e_next].trim();
        if trimmed.starts_with('|') || trimmed.starts_with("#+TBLFM:") {
            table_end += 1;
        } else {
            break;
        }
    }
    (table_start, table_end)
}

/// Evaluates `#+TBLFM:` formulas and renders a calculated table preview.
fn hover_table(source: &SourceFile, doc: &OrgDocument, line_0: u32) -> Option<Value> {
    let line_idx = line_0 as usize;
    if line_idx >= source.line_count() {
        return None;
    }
    let s = source.line_start(line_idx);
    let e = if line_idx + 1 < source.line_count() {
        source.line_start(line_idx + 1)
    } else {
        source.content.len()
    };
    let line_str = source.content[s..e].trim();
    if !line_str.starts_with('|') && !line_str.starts_with("#+TBLFM:") {
        return None;
    }

    let calc = calc_file(&source.content, &doc.table_constants);
    let (table_start, table_end) = find_table_bounds(source, line_idx);
    let calc_source = SourceFile::new(&source.path, calc.content);
    let cs = calc_source.line_start(table_start.min(calc_source.line_count().saturating_sub(1)));
    let ce = if table_end + 1 < calc_source.line_count() {
        calc_source.line_start(table_end + 1)
    } else {
        calc_source.content.len()
    };
    let table_text = calc_source.content[cs..ce].trim_end();

    let mut md = String::from("### Table (Evaluated #+TBLFM)\n\n```org\n");
    md.push_str(table_text);
    md.push_str("\n```\n");

    Some(json!({ "contents": { "kind": "markdown", "value": md } }))
}

/// Handles incoming `textDocument/hover` requests.
fn handle_hover(session: &mut LspSession, params: &Value) -> Result<Value, (i64, String)> {
    let (_uri, path) = session.resolve_uri_and_path(params)?;
    let (source, doc) = session
        .get_or_load_document(&path)
        .map_err(|e| (-32603, e))?;
    let pos = params
        .get("position")
        .ok_or_else(|| (-32602, "Missing position".to_string()))?;
    let line_0 = pos["line"].as_u64().unwrap_or(0) as u32;
    let char_utf16_0 = pos["character"].as_u64().unwrap_or(0) as u32;

    if let Some(res) = hover_link(session, &source, &doc, line_0, char_utf16_0) {
        return Ok(res);
    }
    if let Some(res) = hover_edna(session, &source, &doc, line_0) {
        return Ok(res);
    }
    if let Some(res) = hover_table(&source, &doc, line_0) {
        return Ok(res);
    }

    Ok(Value::Null)
}

// endregion

// region Intelligence Request Routing

/// Handles incoming LSP intelligence requests (`completion`, `hover`).
pub(crate) fn handle_intelligence_request(
    session: &mut LspSession,
    method: &str,
    params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "textDocument/completion" => handle_completion(session, params),
        "textDocument/hover" => handle_hover(session, params),
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}

// endregion
