// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dependency graph extraction and rendering from org-edna properties.
//!
//! Scans documents for `:BLOCKER:` and `:TRIGGER:` properties, extracts
//! dependency edges across `ids()`, `olp()`, `file()`, and structural finders,
//! and renders them as Mermaid, PlantUML, Graphviz DOT, or JSON.

use std::collections::HashMap;
use std::path::Path;

use org_tools_core::document::OrgDocument;
use org_tools_core::edna::{self, EdnaContext, EdnaTarget};
use org_tools_core::locator::FILE_NODE_ENTRY_INDEX;
use serde::Serialize;

/// Maximum display character count for rendered dependency node labels.
const MAX_NODE_TITLE_CHARS: usize = 40;

/// A dependency edge between two entries or file nodes.
#[derive(Debug, Clone, Serialize)]
pub struct DepEdge {
    /// Source entry label.
    pub source: String,
    /// Source entry ID (for graph node identity).
    pub source_id: String,
    /// Source entry status.
    pub source_status: Option<String>,
    /// Target entry label.
    pub target: String,
    /// Target entry ID.
    pub target_id: String,
    /// Target entry status.
    pub target_status: Option<String>,
    /// Edge type: `"blocks"` or `"triggers"`.
    pub edge_type: String,
}

/// Extracts `:BLOCKER:` and `:TRIGGER:` dependency edges from `docs`.
pub fn extract_edges(docs: &[&OrgDocument]) -> Vec<DepEdge> {
    let mut edges = Vec::new();
    for &doc in docs {
        for idx in 0..doc.entries.len() {
            let ctx = EdnaContext {
                all_docs: docs,
                doc,
                entry_idx: idx,
            };
            if let Some(blocker_val) = doc.property(idx, "BLOCKER") {
                append_blocker_edges(&ctx, blocker_val, docs, &mut edges);
            }
            if let Some(trigger_val) = doc.property(idx, "TRIGGER") {
                append_trigger_edges(&ctx, trigger_val, docs, &mut edges);
            }
        }
    }
    edges
}

/// Appends `"blocks"` edges resolved from `blocker_val` onto `edges`.
fn append_blocker_edges(
    ctx: &EdnaContext<'_>,
    blocker_val: &str,
    docs: &[&OrgDocument],
    edges: &mut Vec<DepEdge>,
) {
    let (exprs, _) = edna::parse_edna(blocker_val);
    let self_label = node_label(ctx.doc, ctx.entry_idx);
    let self_id = entry_node_id(ctx.doc, ctx.entry_idx);
    let self_status = node_status(ctx.doc, ctx.entry_idx);

    for target in edna::collect_resolved_targets(&exprs, ctx) {
        if let Some(dep_doc) = find_doc_by_path(docs, &target.file) {
            edges.push(DepEdge {
                source: node_label(dep_doc, target.entry_idx),
                source_id: entry_node_id(dep_doc, target.entry_idx),
                source_status: node_status(dep_doc, target.entry_idx),
                target: self_label.clone(),
                target_id: self_id.clone(),
                target_status: self_status.clone(),
                edge_type: "blocks".to_string(),
            });
        }
    }
}

/// Appends `"triggers"` edges resolved from `trigger_val` onto `edges`.
fn append_trigger_edges(
    ctx: &EdnaContext<'_>,
    trigger_val: &str,
    docs: &[&OrgDocument],
    edges: &mut Vec<DepEdge>,
) {
    let (exprs, _) = edna::parse_edna(trigger_val);
    let self_label = node_label(ctx.doc, ctx.entry_idx);
    let self_id = entry_node_id(ctx.doc, ctx.entry_idx);
    let self_status = node_status(ctx.doc, ctx.entry_idx);

    for target in edna::collect_resolved_targets(&exprs, ctx) {
        if let Some(dep_doc) = find_doc_by_path(docs, &target.file) {
            edges.push(build_trigger_edge(
                (&self_label, &self_id, &self_status),
                dep_doc,
                &target,
            ));
        }
    }
}

/// Constructs a `"triggers"` [`DepEdge`] from `source_meta` to `(dep_doc, target)`.
fn build_trigger_edge(
    source_meta: (&str, &str, &Option<String>),
    dep_doc: &OrgDocument,
    target: &EdnaTarget,
) -> DepEdge {
    DepEdge {
        source: source_meta.0.to_string(),
        source_id: source_meta.1.to_string(),
        source_status: source_meta.2.clone(),
        target: node_label(dep_doc, target.entry_idx),
        target_id: entry_node_id(dep_doc, target.entry_idx),
        target_status: node_status(dep_doc, target.entry_idx),
        edge_type: "triggers".to_string(),
    }
}

/// Finds a document in `docs` matching `path`.
fn find_doc_by_path<'a>(docs: &[&'a OrgDocument], path: &Path) -> Option<&'a OrgDocument> {
    if let Some(&exact) = docs.iter().find(|d| d.file == path) {
        return Some(exact);
    }
    let canon = path.canonicalize().ok()?;
    docs.iter()
        .find(|d| d.file.canonicalize().ok().as_ref() == Some(&canon))
        .copied()
}

/// Returns the truncated display label for `(doc, entry_idx)`.
fn node_label(doc: &OrgDocument, entry_idx: usize) -> String {
    if let Some(entry) = doc.entries.get(entry_idx) {
        truncate_title(&entry.title, MAX_NODE_TITLE_CHARS)
    } else {
        truncate_title(&doc.file_title_or_stem(), MAX_NODE_TITLE_CHARS)
    }
}

/// Returns the TODO keyword status for `(doc, entry_idx)`, if any.
fn node_status(doc: &OrgDocument, entry_idx: usize) -> Option<String> {
    doc.entries.get(entry_idx).and_then(|e| e.keyword.clone())
}

/// Generates a stable graph node identifier for `(doc, entry_idx)`.
fn entry_node_id(doc: &OrgDocument, entry_idx: usize) -> String {
    if let Some(id) = doc.property(entry_idx, "ID") {
        return id.to_string();
    }
    let fname = doc.file.file_name().unwrap_or_default().to_string_lossy();
    if entry_idx == FILE_NODE_ENTRY_INDEX {
        return format!("{fname}:0");
    }
    let line = doc.entries.get(entry_idx).map_or(1, |e| e.heading_line);
    format!("{fname}:{line}")
}

/// Truncates `title` to `max_chars` Unicode scalar values cleanly.
fn truncate_title(title: &str, max_chars: usize) -> String {
    let chars: Vec<char> = title.chars().collect();
    if chars.len() <= max_chars {
        title.to_string()
    } else {
        let prefix: String = chars[..max_chars.saturating_sub(1)].iter().collect();
        format!("{prefix}…")
    }
}

/// Sanitizes a raw identifier for use as a Mermaid/DOT node key.
fn mermaid_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Renders `edges` as a Mermaid left-to-right flowchart.
pub fn render_mermaid(edges: &[DepEdge], docs: &[&OrgDocument]) -> String {
    let mut out = String::from("graph LR\n");
    let mut nodes: HashMap<String, (&str, bool)> = HashMap::new();

    for edge in edges {
        let src_done = is_done_keyword(edge.source_status.as_deref(), docs);
        let tgt_done = is_done_keyword(edge.target_status.as_deref(), docs);
        nodes
            .entry(edge.source_id.clone())
            .or_insert((&edge.source, src_done));
        nodes
            .entry(edge.target_id.clone())
            .or_insert((&edge.target, tgt_done));
    }

    for (id, (label, _)) in &nodes {
        let mid = mermaid_id(id);
        let safe_label = label.replace('"', "'");
        out.push_str(&format!("    {mid}[\"{safe_label}\"]\n"));
    }
    for edge in edges {
        let src = mermaid_id(&edge.source_id);
        let tgt = mermaid_id(&edge.target_id);
        out.push_str(&format!("    {src} -->|{}| {tgt}\n", edge.edge_type));
    }
    for (id, (_, done)) in &nodes {
        if *done {
            out.push_str(&format!("    style {} fill:#90EE90\n", mermaid_id(id)));
        }
    }
    out
}

/// Renders `edges` as PlantUML component syntax.
pub fn render_plantuml(edges: &[DepEdge]) -> String {
    let mut out = String::from("@startuml\n");
    for edge in edges {
        out.push_str(&format!(
            "[{}] --> [{}] : {}\n",
            edge.source, edge.target, edge.edge_type
        ));
    }
    out.push_str("@enduml\n");
    out
}

/// Renders `edges` as Graphviz DOT syntax.
pub fn render_dot(edges: &[DepEdge]) -> String {
    let mut out = String::from("digraph dependencies {\n    rankdir=LR;\n    node [shape=box];\n");
    let mut nodes: HashMap<String, String> = HashMap::new();
    for edge in edges {
        nodes
            .entry(edge.source_id.clone())
            .or_insert_with(|| edge.source.clone());
        nodes
            .entry(edge.target_id.clone())
            .or_insert_with(|| edge.target.clone());
    }
    for (id, label) in &nodes {
        let nid = mermaid_id(id);
        out.push_str(&format!(
            "    {nid} [label=\"{}\"];\n",
            label.replace('"', "\\\"")
        ));
    }
    for edge in edges {
        let src = mermaid_id(&edge.source_id);
        let tgt = mermaid_id(&edge.target_id);
        out.push_str(&format!(
            "    {src} -> {tgt} [label=\"{}\"];\n",
            edge.edge_type
        ));
    }
    out.push_str("}\n");
    out
}

/// Renders `edges` as pretty-printed JSON.
pub fn render_json(edges: &[DepEdge]) -> String {
    serde_json::to_string_pretty(edges).unwrap_or_default()
}

/// Returns `true` when `keyword` is a done state in any of `docs` (or `"DONE"`).
fn is_done_keyword(keyword: Option<&str>, docs: &[&OrgDocument]) -> bool {
    let Some(kw) = keyword else {
        return false;
    };
    docs.iter().any(|doc| doc.todo_keywords.is_done(kw)) || kw == "DONE"
}
