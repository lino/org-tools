// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only roam and GTD query MCP tool handlers (`org_roam_*`, `org_query_*`).

use std::path::Path;
use std::time::Instant;

use serde_json::{json, Map, Value};

use org_tools_core::cache::{
    load_cached_fts_candidate_docs, load_docs_with_bodies, RoamWorkspace, WorkspaceCache,
};
use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::roam::{RoamBacklink, RoamNode};

use super::schema::{
    finish_tool_call, optional_positive_usize, optional_str, require_non_empty_str,
};
use super::McpSession;
use crate::commands::query::collect_predicate_matches;
use crate::commands::{parse_org_files, MutationTelemetry};
use crate::{date, query};

/// Resolved roam nodes, optional incoming backlinks, and execution telemetry.
type RoamLookupResult =
    Result<(Vec<RoamNode>, Vec<RoamBacklink>, MutationTelemetry), (String, MutationTelemetry)>;

/// Resolves roam nodes (and optional backlinks) directly from the warm `WorkspaceCache`.
fn query_roam_cached(
    session: &mut McpSession,
    display_ws: &Path,
    norm_ws: &Path,
    query_str: &str,
    with_backlinks: bool,
    started: Instant,
) -> RoamLookupResult {
    let tel = session
        .sync_telemetry(norm_ws, true, started)
        .map_err(|e| (e, session.empty_telemetry(started)))?;
    let cache = session.warm_cache_for(true).expect("warm cache active");
    let ns = cache
        .resolve_roam_nodes(display_ws, query_str, true)
        .map_err(|e| (e, session.empty_telemetry(started)))?;
    let bl = if with_backlinks {
        cache
            .roam_backlinks_for(display_ws, &ns)
            .map_err(|e| (e, session.empty_telemetry(started)))?
    } else {
        Vec::new()
    };
    Ok((ns, bl, tel))
}

/// Resolves roam nodes (and optional backlinks) via warm `WorkspaceCache` or `RoamWorkspace`.
fn query_roam_nodes_and_backlinks(
    session: &mut McpSession,
    display_ws: &Path,
    norm_ws: &Path,
    is_default: bool,
    query_str: &str,
    with_backlinks: bool,
    started: Instant,
) -> RoamLookupResult {
    if !session.no_cache && is_default {
        return query_roam_cached(
            session,
            display_ws,
            norm_ws,
            query_str,
            with_backlinks,
            started,
        );
    }

    let backend = RoamWorkspace::open(&[display_ws.to_path_buf()], session.no_cache)
        .map_err(|e| (e, session.empty_telemetry(started)))?;
    let counts = backend.counts();
    let ns = backend
        .resolve(query_str)
        .map_err(|e| (e, session.counts_telemetry(started, counts)))?;
    let bl = if with_backlinks {
        backend
            .backlinks(&ns)
            .map_err(|e| (e, session.counts_telemetry(started, counts)))?
    } else {
        Vec::new()
    };
    Ok((ns, bl, session.counts_telemetry(started, counts)))
}

/// Executes `org_roam_node` against the warm `WorkspaceCache` or `RoamWorkspace` fallback.
pub(super) fn exec_roam_node(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let (display_ws, norm_ws, is_default) = session
            .prepare_workspace(args, &["query", "workspace"])
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let query_str = require_non_empty_str(args, "query")
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let (nodes, _, mut tel) = query_roam_nodes_and_backlinks(
            session,
            &display_ws,
            &norm_ws,
            is_default,
            &query_str,
            false,
            started,
        )?;
        tel.duration_us = started.elapsed().as_micros() as u64;
        Ok(json!({ "kind": "node", "query": query_str, "nodes": nodes, "telemetry": tel }))
    })())
}

/// Executes `org_roam_backlinks`, returning `isError: true` when `target` does not resolve.
pub(super) fn exec_roam_backlinks(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let (display_ws, norm_ws, is_default) = session
            .prepare_workspace(args, &["target", "workspace"])
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let target = require_non_empty_str(args, "target")
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let (nodes, backlinks, mut tel) = query_roam_nodes_and_backlinks(
            session,
            &display_ws,
            &norm_ws,
            is_default,
            &target,
            true,
            started,
        )?;
        tel.duration_us = started.elapsed().as_micros() as u64;
        if nodes.is_empty() {
            return Err((format!("target node not found: {target}"), tel));
        }
        Ok(json!({
            "kind": "backlinks",
            "target": target,
            "nodes": nodes,
            "backlinks": backlinks,
            "telemetry": tel
        }))
    })())
}

/// Attempts a warm FTS5 BM25 search via `cache` when `pred` has positive non-edna body phrases.
fn try_warm_fts_entries(
    cache: &WorkspaceCache,
    display_ws: &Path,
    pred: &query::parser::Predicate,
    limit: Option<usize>,
) -> Result<Option<Vec<Value>>, String> {
    let req = pred.required_body_phrases();
    if req.is_empty() || req != pred.positive_body_phrases() || pred.uses_edna() {
        return Ok(None);
    }
    let Some((hits, loaded)) = load_cached_fts_candidate_docs(cache, display_ws, &req)? else {
        return Ok(None);
    };
    if hits.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let doc_refs: Vec<&OrgDocument> = loaded.iter().map(|d| &d.doc).collect();
    let pairs = query::predicate::filter_fts_hits(
        pred,
        display_ws,
        hits,
        &loaded,
        &doc_refs,
        date::current_date(),
        limit,
    );
    let matches: Vec<_> = pairs
        .into_iter()
        .map(|(di, ei)| query::output::MatchedEntry {
            doc: &loaded[di].doc,
            entry_idx: ei,
        })
        .collect();
    let rendered = query::output::render_json(&matches, &doc_refs);
    Ok(Some(serde_json::from_str(&rendered).unwrap_or_default()))
}

/// Evaluates `pred` directly across `.org` files in `display_ws` and serializes `JsonEntry` items.
fn eval_direct_search_entries(
    display_ws: &Path,
    pred: &query::parser::Predicate,
    limit: Option<usize>,
) -> Vec<Value> {
    let files = collect_org_files(&[display_ws.to_path_buf()]);
    if files.is_empty() {
        return Vec::new();
    }
    if pred.uses_body() {
        let loaded = load_docs_with_bodies(&files);
        let doc_refs: Vec<&OrgDocument> = loaded.iter().map(|d| &d.doc).collect();
        let pairs = query::predicate::eval_and_rank_body_docs(
            pred,
            &loaded,
            &doc_refs,
            date::current_date(),
            limit,
        );
        let matches: Vec<_> = pairs
            .into_iter()
            .map(|(di, ei)| query::output::MatchedEntry {
                doc: &loaded[di].doc,
                entry_idx: ei,
            })
            .collect();
        let rendered = query::output::render_json(&matches, &doc_refs);
        return serde_json::from_str(&rendered).unwrap_or_default();
    }
    let docs = parse_org_files(&files);
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let matches = collect_predicate_matches(pred, &docs, &doc_refs, date::current_date(), limit);
    serde_json::from_str(&query::output::render_json(&matches, &doc_refs)).unwrap_or_default()
}

/// Executes `org_roam_search` using `parse_query` with automatic `Predicate::Body` fallback.
pub(super) fn exec_roam_search(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let (display_ws, norm_ws, is_default) = session
            .prepare_workspace(args, &["query", "limit", "workspace"])
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let query_str = require_non_empty_str(args, "query")
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let limit = optional_positive_usize(args, "limit")
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let mut tel = session
            .sync_telemetry(&norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let pred = query::parser::parse_query(&query_str)
            .unwrap_or_else(|_| query::parser::Predicate::Body(query_str.clone()));
        let fts_attempt = match session.warm_cache_for(is_default) {
            Some(cache) => try_warm_fts_entries(cache, &display_ws, &pred, limit)
                .map_err(|e| (e, session.empty_telemetry(started)))?,
            None => None,
        };
        let entries =
            fts_attempt.unwrap_or_else(|| eval_direct_search_entries(&display_ws, &pred, limit));
        tel.duration_us = started.elapsed().as_micros() as u64;
        Ok(json!({ "entries": entries, "count": entries.len(), "telemetry": tel }))
    })())
}

/// Collects actionable entries matching optional `context` across `docs`.
fn collect_actionable_json_entries(
    docs: &[OrgDocument],
    context: Option<&str>,
    limit: Option<usize>,
) -> Vec<Value> {
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let today = date::current_date();
    let mut matches = Vec::new();
    for doc in docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            let ctx_ok = context.is_none_or(|c| {
                doc.inherited_tags(idx)
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(c))
            });
            if ctx_ok
                && query::predicate::matches(
                    &query::parser::Predicate::Actionable,
                    entry,
                    doc,
                    &doc_refs,
                    today,
                )
            {
                matches.push(query::output::MatchedEntry {
                    doc,
                    entry_idx: idx,
                });
            }
        }
    }
    if let Some(n) = limit {
        matches.truncate(n);
    }
    serde_json::from_str(&query::output::render_json(&matches, &doc_refs)).unwrap_or_default()
}

/// Executes `org_query_next` (`Predicate::Actionable` + optional `@context` filter).
pub(super) fn exec_query_next(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let (display_ws, norm_ws, is_default) = session
            .prepare_workspace(args, &["context", "limit", "workspace"])
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let context =
            optional_str(args, "context").map_err(|e| (e, session.empty_telemetry(started)))?;
        let limit = optional_positive_usize(args, "limit")
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let mut tel = session
            .sync_telemetry(&norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let docs = parse_org_files(&collect_org_files(&[display_ws]));
        let entries = collect_actionable_json_entries(&docs, context.as_deref(), limit);
        tel.duration_us = started.elapsed().as_micros() as u64;
        Ok(json!({ "entries": entries, "count": entries.len(), "telemetry": tel }))
    })())
}

/// Executes `org_query_blocked` (`Predicate::Blocked` with `blocking_entries` details).
pub(super) fn exec_query_blocked(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let (display_ws, norm_ws, is_default) = session
            .prepare_workspace(args, &["limit", "workspace"])
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let limit = optional_positive_usize(args, "limit")
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let mut tel = session
            .sync_telemetry(&norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let docs = parse_org_files(&collect_org_files(&[display_ws]));
        let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
        let matches = collect_predicate_matches(
            &query::parser::Predicate::Blocked,
            &docs,
            &doc_refs,
            date::current_date(),
            limit,
        );
        let entries: Vec<Value> =
            serde_json::from_str(&query::output::render_blocked_json(&matches, &doc_refs))
                .unwrap_or_default();
        tel.duration_us = started.elapsed().as_micros() as u64;
        Ok(json!({ "entries": entries, "count": entries.len(), "telemetry": tel }))
    })())
}
