// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scoped single-node `org_show` MCP tool handler (`json`, `org`, and `md` formats).

use std::path::Path;
use std::time::Instant;

use serde_json::{json, Map, Value};

use org_tools_core::cache::MabuseCache;
use org_tools_core::document::{ChecklistSummary, OrgDocument};
use org_tools_core::export_md::{
    export_slice_to_markdown, extract_org_slice, resolve_workspace_locator,
    scan_file_node_checklists, truncate_unicode_chars, ResolvedWorkspaceNode,
};
use org_tools_core::locator::{
    locator_for_entry, resolve_in_document, OrgLocator, FILE_NODE_ENTRY_INDEX,
};
use org_tools_core::rules::timestamp::OrgTimestamp;
use org_tools_core::source::SourceFile;

use super::confinement::{
    canonical_workspace_root, confine_and_rewrite_locator, verify_existing_confined_org_file,
};
use super::schema::{
    finish_tool_call, optional_bool, optional_positive_usize, optional_str, require_non_empty_str,
};
use super::McpSession;
use crate::commands::MutationTelemetry;

/// Formats an [`OrgTimestamp`] as `YYYY-MM-DD` or `YYYY-MM-DD HH:MM`.
fn format_show_ts(ts: &OrgTimestamp) -> String {
    match (ts.hour, ts.minute) {
        (Some(h), Some(m)) => {
            format!("{:04}-{:02}-{:02} {h:02}:{m:02}", ts.year, ts.month, ts.day)
        }
        _ => format!("{:04}-{:02}-{:02}", ts.year, ts.month, ts.day),
    }
}

/// Resolves `locator` within `canon_ws`, using the warm `MabuseCache` fast-path for `id:` lookups.
fn resolve_show_node(
    cache: Option<&MabuseCache>,
    locator: &OrgLocator,
    display_ws: &Path,
    canon_ws: &Path,
    need_ws_docs: bool,
) -> Result<ResolvedWorkspaceNode, String> {
    if !need_ws_docs {
        if let (Some(c), OrgLocator::Id(uuid)) = (cache, locator) {
            if let Ok(nodes) = c.resolve_roam_nodes(display_ws, &format!("id:{uuid}"), false) {
                if let Some(first) = nodes.first() {
                    let cand = Path::new(&first.file);
                    verify_existing_confined_org_file(cand, canon_ws, &first.file)?;
                    if let Ok(source) = SourceFile::from_path(cand) {
                        let doc = OrgDocument::from_source(&source);
                        if let Ok(resolved) = resolve_in_document(locator, &doc) {
                            return Ok(ResolvedWorkspaceNode {
                                resolved,
                                source,
                                doc,
                                workspace_docs: Vec::new(),
                                files_scanned: 1,
                                cache_hits: 1,
                                cache_misses: 0,
                            });
                        }
                    }
                }
            }
        }
    }
    let loaded = resolve_workspace_locator(locator, Some(display_ws), display_ws, need_ws_docs)?;
    verify_existing_confined_org_file(
        &loaded.doc.file,
        canon_ws,
        &loaded.doc.file.display().to_string(),
    )?;
    Ok(loaded)
}

/// Builds the structured JSON response object for a level-0 file node in `org_show`.
fn build_file_node_payload(
    loaded: &ResolvedWorkspaceNode,
    format_name: &str,
    bounds: (usize, usize),
    no_children: bool,
    truncation: (String, bool, usize),
    telemetry: MutationTelemetry,
) -> Value {
    let checklist = scan_file_node_checklists(&loaded.source, &loaded.doc);
    let summary = ChecklistSummary::from_items(&checklist);
    json!({
        "locator": locator_for_entry(&loaded.doc, FILE_NODE_ENTRY_INDEX).to_string(),
        "format": format_name,
        "file": loaded.doc.file.display().to_string(),
        "line": bounds.0,
        "end_line": bounds.1,
        "level": 0,
        "title": loaded.resolved.heading_text,
        "keyword": null,
        "priority": null,
        "tags": loaded.doc.filetags,
        "properties": loaded.doc.file_properties,
        "planning": { "scheduled": null, "deadline": null, "closed": null },
        "checklist_summary": summary,
        "checklist": checklist,
        "no_children": no_children,
        "truncated": truncation.1,
        "char_count": truncation.2,
        "content": truncation.0,
        "telemetry": telemetry
    })
}

/// Builds the structured JSON response object for `org_show`.
fn build_show_payload(
    loaded: &ResolvedWorkspaceNode,
    format_name: &str,
    no_children: bool,
    max_chars: Option<usize>,
    telemetry: MutationTelemetry,
) -> Value {
    let slice = extract_org_slice(&loaded.source, &loaded.doc, &loaded.resolved, no_children);
    let raw_rendered = if format_name == "md" {
        export_slice_to_markdown(slice.raw, &loaded.doc.file, &loaded.workspace_docs)
    } else {
        slice.raw.to_string()
    };
    let truncation = truncate_unicode_chars(&raw_rendered, max_chars);

    if loaded.resolved.is_file_node() {
        return build_file_node_payload(
            loaded,
            format_name,
            (slice.start_line, slice.end_line),
            no_children,
            truncation,
            telemetry,
        );
    }

    let entry = &loaded.doc.entries[loaded.resolved.entry_index];
    json!({
        "locator": locator_for_entry(&loaded.doc, loaded.resolved.entry_index).to_string(),
        "format": format_name,
        "file": loaded.doc.file.display().to_string(),
        "line": slice.start_line,
        "end_line": slice.end_line,
        "level": entry.level,
        "title": entry.title,
        "keyword": entry.keyword,
        "priority": entry.priority.map(|c| c.to_string()),
        "tags": entry.tags,
        "properties": entry.properties,
        "planning": {
            "scheduled": entry.planning.scheduled.as_ref().map(format_show_ts),
            "deadline": entry.planning.deadline.as_ref().map(format_show_ts),
            "closed": entry.planning.closed.as_ref().map(format_show_ts),
        },
        "checklist_summary": entry.checklist_summary(),
        "checklist": entry.checklist,
        "no_children": no_children,
        "truncated": truncation.1,
        "char_count": truncation.2,
        "content": truncation.0,
        "telemetry": telemetry
    })
}

/// Validated parameters for an `org_show` tool call.
struct ShowParams {
    /// Parsed locator string.
    locator_str: String,
    /// Validated output format (`"org"`, `"json"`, or `"md"`).
    fmt_str: String,
    /// Whether child subheadings should be omitted.
    no_children: bool,
    /// Optional Unicode character limit.
    max_chars: Option<usize>,
}

/// Parses and validates `org_show` tool arguments.
fn parse_show_params(args: &Map<String, Value>) -> Result<ShowParams, String> {
    let locator_str = require_non_empty_str(args, "locator")?;
    let fmt_str = match optional_str(args, "format")? {
        Some(f) if matches!(f.as_str(), "org" | "json" | "md") => f,
        Some(f) => {
            return Err(format!(
                "unsupported format '{f}' (expected org, json, or md)"
            ))
        }
        None => "json".to_string(),
    };
    let no_children = optional_bool(args, "no_children", false)?;
    let max_chars = optional_positive_usize(args, "max_chars")?;
    Ok(ShowParams {
        locator_str,
        fmt_str,
        no_children,
        max_chars,
    })
}

/// Executes `org_show` for a single confined workspace locator in `json`, `org`, or `md` format.
pub(super) fn exec_show(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let allowed = &["locator", "format", "no_children", "max_chars", "workspace"];
        let (display_ws, norm_ws, is_default) = session
            .prepare_workspace(args, allowed)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let params = parse_show_params(args).map_err(|e| (e, session.empty_telemetry(started)))?;
        let canon_ws = canonical_workspace_root(&norm_ws)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let raw_locator = OrgLocator::parse(&params.locator_str)
            .map_err(|err| (err.to_string(), session.empty_telemetry(started)))?;
        let locator = confine_and_rewrite_locator(&raw_locator, &display_ws, &canon_ws)
            .map_err(|msg| (msg, session.empty_telemetry(started)))?;

        let mut tel = session
            .sync_telemetry(&norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let loaded = match resolve_show_node(
            session.warm_cache_for(is_default),
            &locator,
            &display_ws,
            &canon_ws,
            params.fmt_str == "md",
        ) {
            Ok(node) => node,
            Err(msg) => return Err((msg, tel)),
        };
        tel.duration_us = started.elapsed().as_micros() as u64;
        Ok(build_show_payload(
            &loaded,
            &params.fmt_str,
            params.no_children,
            params.max_chars,
            tel,
        ))
    })())
}
