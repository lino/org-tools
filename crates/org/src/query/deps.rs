// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dependency graph extraction and rendering from org-edna properties.
//!
//! Scans documents for `:BLOCKER:` and `:TRIGGER:` properties, extracts
//! dependency edges, and renders them as Mermaid, PlantUML, Graphviz DOT,
//! or JSON.

use org_tools_core::document::OrgDocument;
use org_tools_core::edna::{self, EdnaContext};
use serde::Serialize;

/// A dependency edge between two entries.
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
    /// Edge type: "blocks" or "triggers".
    pub edge_type: String,
}

/// Extract dependency edges from all documents.
pub fn extract_edges(docs: &[&OrgDocument]) -> Vec<DepEdge> {
    let mut edges = Vec::new();

    for doc in docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            let node_id = entry_node_id(doc, idx);
            let label = truncate_title(&entry.title, 40);

            // Check BLOCKER property.
            if let Some(blocker_val) = entry.properties.get("BLOCKER") {
                let dep_ids = edna::extract_dependency_ids(blocker_val);
                for dep_id in &dep_ids {
                    // Find the dependency entry.
                    for dep_doc in docs {
                        if let Some(dep_idx) = dep_doc.find_by_id(dep_id) {
                            let dep_entry = &dep_doc.entries[dep_idx];
                            edges.push(DepEdge {
                                source: truncate_title(&dep_entry.title, 40),
                                source_id: entry_node_id(dep_doc, dep_idx),
                                source_status: dep_entry.keyword.clone(),
                                target: label.clone(),
                                target_id: node_id.clone(),
                                target_status: entry.keyword.clone(),
                                edge_type: "blocks".to_string(),
                            });
                            break;
                        }
                    }
                }

                // Handle structural finders.
                let (exprs, _) = edna::parse_edna(blocker_val);
                for expr in &exprs {
                    if let edna::EdnaExpr::Finder(finder) = expr {
                        let ctx = EdnaContext {
                            all_docs: docs,
                            doc,
                            entry_idx: idx,
                        };
                        // For structural finders (not ids), resolve and add edges.
                        match finder {
                            edna::Finder::Ids(_) => {} // Already handled above.
                            edna::Finder::PreviousSibling
                            | edna::Finder::NextSibling
                            | edna::Finder::Parent
                            | edna::Finder::Children
                            | edna::Finder::Self_ => {
                                let resolved = resolve_structural(finder, &ctx);
                                for (res_doc, res_idx) in &resolved {
                                    let res_entry = &res_doc.entries[*res_idx];
                                    edges.push(DepEdge {
                                        source: truncate_title(&res_entry.title, 40),
                                        source_id: entry_node_id(res_doc, *res_idx),
                                        source_status: res_entry.keyword.clone(),
                                        target: label.clone(),
                                        target_id: node_id.clone(),
                                        target_status: entry.keyword.clone(),
                                        edge_type: "blocks".to_string(),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            // Check TRIGGER property (edges go from current to targets).
            if let Some(trigger_val) = entry.properties.get("TRIGGER") {
                let dep_ids = edna::extract_dependency_ids(trigger_val);
                for dep_id in &dep_ids {
                    for dep_doc in docs {
                        if let Some(dep_idx) = dep_doc.find_by_id(dep_id) {
                            let dep_entry = &dep_doc.entries[dep_idx];
                            edges.push(DepEdge {
                                source: label.clone(),
                                source_id: node_id.clone(),
                                source_status: entry.keyword.clone(),
                                target: truncate_title(&dep_entry.title, 40),
                                target_id: entry_node_id(dep_doc, dep_idx),
                                target_status: dep_entry.keyword.clone(),
                                edge_type: "triggers".to_string(),
                            });
                            break;
                        }
                    }
                }
            }
        }
    }

    edges
}

/// Resolve structural finders to (doc, entry_idx) pairs.
fn resolve_structural<'a>(
    finder: &edna::Finder,
    ctx: &EdnaContext<'a>,
) -> Vec<(&'a OrgDocument, usize)> {
    let entry = &ctx.doc.entries[ctx.entry_idx];
    match finder {
        edna::Finder::PreviousSibling => {
            let siblings = get_siblings(ctx.doc, ctx.entry_idx);
            let my_pos = siblings.iter().position(|&i| i == ctx.entry_idx);
            if let Some(pos) = my_pos {
                if pos > 0 {
                    return vec![(ctx.doc, siblings[pos - 1])];
                }
            }
            Vec::new()
        }
        edna::Finder::NextSibling => {
            let siblings = get_siblings(ctx.doc, ctx.entry_idx);
            let my_pos = siblings.iter().position(|&i| i == ctx.entry_idx);
            if let Some(pos) = my_pos {
                if pos + 1 < siblings.len() {
                    return vec![(ctx.doc, siblings[pos + 1])];
                }
            }
            Vec::new()
        }
        edna::Finder::Parent => {
            if let Some(parent_idx) = entry.parent {
                vec![(ctx.doc, parent_idx)]
            } else {
                Vec::new()
            }
        }
        edna::Finder::Children => entry.children.iter().map(|&i| (ctx.doc, i)).collect(),
        edna::Finder::Self_ => vec![(ctx.doc, ctx.entry_idx)],
        _ => Vec::new(),
    }
}

/// Get sibling indices for an entry.
fn get_siblings(doc: &OrgDocument, entry_idx: usize) -> Vec<usize> {
    let entry = &doc.entries[entry_idx];
    if let Some(parent_idx) = entry.parent {
        doc.entries[parent_idx].children.clone()
    } else {
        doc.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.parent.is_none())
            .map(|(i, _)| i)
            .collect()
    }
}

/// Generate a stable node ID for an entry.
fn entry_node_id(doc: &OrgDocument, entry_idx: usize) -> String {
    let entry = &doc.entries[entry_idx];
    if let Some(id) = entry.properties.get("ID") {
        id.clone()
    } else {
        format!(
            "{}:{}",
            doc.file.file_name().unwrap_or_default().to_string_lossy(),
            entry.heading_line
        )
    }
}

/// Truncate a title to `max_len` characters.
fn truncate_title(title: &str, max_len: usize) -> String {
    if title.len() <= max_len {
        title.to_string()
    } else {
        format!("{}…", &title[..max_len - 1])
    }
}

/// Sanitise a string for use as a Mermaid node ID.
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

/// Render edges as a Mermaid graph.
pub fn render_mermaid(edges: &[DepEdge], docs: &[&OrgDocument]) -> String {
    let mut out = String::from("graph LR\n");

    // Collect all unique node IDs.
    let mut nodes: std::collections::HashMap<String, (&str, &Option<String>, bool)> =
        std::collections::HashMap::new();

    for edge in edges {
        let src_done = is_done_keyword(edge.source_status.as_deref(), docs);
        let tgt_done = is_done_keyword(edge.target_status.as_deref(), docs);
        nodes.entry(edge.source_id.clone()).or_insert((
            &edge.source,
            &edge.source_status,
            src_done,
        ));
        nodes.entry(edge.target_id.clone()).or_insert((
            &edge.target,
            &edge.target_status,
            tgt_done,
        ));
    }

    // Render nodes.
    for (id, (label, _keyword, _done)) in &nodes {
        let mid = mermaid_id(id);
        // Escape quotes in label.
        let safe_label = label.replace('"', "'");
        out.push_str(&format!("    {mid}[\"{safe_label}\"]\n"));
    }

    // Render edges.
    for edge in edges {
        let src = mermaid_id(&edge.source_id);
        let tgt = mermaid_id(&edge.target_id);
        let label = &edge.edge_type;
        out.push_str(&format!("    {src} -->|{label}| {tgt}\n"));
    }

    // Style done nodes green, blocked nodes amber.
    for (id, (_label, _keyword, done)) in &nodes {
        let mid = mermaid_id(id);
        if *done {
            out.push_str(&format!("    style {mid} fill:#90EE90\n"));
        }
    }

    out
}

/// Render edges as PlantUML.
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

/// Render edges as Graphviz DOT.
pub fn render_dot(edges: &[DepEdge]) -> String {
    let mut out = String::from("digraph dependencies {\n    rankdir=LR;\n    node [shape=box];\n");

    // Collect unique nodes.
    let mut nodes: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for edge in edges {
        nodes
            .entry(edge.source_id.clone())
            .or_insert_with(|| edge.source.clone());
        nodes
            .entry(edge.target_id.clone())
            .or_insert_with(|| edge.target.clone());
    }

    // Render nodes with labels.
    for (id, label) in &nodes {
        let nid = mermaid_id(id);
        out.push_str(&format!(
            "    {nid} [label=\"{}\"];\n",
            label.replace('"', "\\\"")
        ));
    }

    // Render edges.
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

/// Render edges as JSON.
pub fn render_json(edges: &[DepEdge]) -> String {
    serde_json::to_string_pretty(edges).unwrap_or_default()
}

/// Check if a keyword is a done keyword.
fn is_done_keyword(keyword: Option<&str>, docs: &[&OrgDocument]) -> bool {
    let kw = match keyword {
        Some(k) => k,
        None => return false,
    };
    // Check against all documents' todo keyword configs.
    for doc in docs {
        if doc.todo_keywords.is_done(kw) {
            return true;
        }
    }
    // Fallback: DONE is always done.
    kw == "DONE"
}
