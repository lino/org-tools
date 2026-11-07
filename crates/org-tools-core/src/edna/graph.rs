// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `:BLOCKER:` dependency graph analysis, shortest-cycle detection, and
//! symmetric `:BLOCKER:` / `:TRIGGER:` property editing helpers.
//!
//! Spec: <https://www.nongnu.org/org-edna-el/>

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use crate::document::OrgDocument;
use crate::locator::FILE_NODE_ENTRY_INDEX;

use super::eval::{resolve_finder_targets, EdnaContext, EdnaTarget};
use super::parser::{format_edna_exprs, parse_edna, Action, Condition, EdnaExpr, Finder};

/// Canonical node identifier in a workspace `:BLOCKER:` dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GraphNode {
    /// Canonical or normalized file path of the document.
    pub file: PathBuf,
    /// Zero-based entry index in [`OrgDocument::entries`] (or [`FILE_NODE_ENTRY_INDEX`]).
    pub entry_idx: usize,
}

impl GraphNode {
    /// Builds a [`GraphNode`] for `(doc, entry_idx)` with canonicalized path when possible.
    pub fn from_doc(doc: &OrgDocument, entry_idx: usize) -> Self {
        let file = doc.file.canonicalize().unwrap_or_else(|_| doc.file.clone());
        Self { file, entry_idx }
    }
}

/// Resolves all top-level and `if`-branch finders in `exprs` into deduplicated [`EdnaTarget`]s.
pub fn collect_resolved_targets(exprs: &[EdnaExpr], ctx: &EdnaContext<'_>) -> Vec<EdnaTarget> {
    let mut out = Vec::new();
    for expr in exprs {
        match expr {
            EdnaExpr::Finder(f) => {
                for target in resolve_finder_targets(f, ctx) {
                    if !out.contains(&target) {
                        out.push(target);
                    }
                }
            }
            EdnaExpr::If { cond, then, else_ } => {
                for branch in [
                    std::slice::from_ref(cond.as_ref()),
                    then.as_slice(),
                    else_.as_slice(),
                ] {
                    for target in collect_resolved_targets(branch, ctx) {
                        if !out.contains(&target) {
                            out.push(target);
                        }
                    }
                }
            }
            EdnaExpr::Action(_) | EdnaExpr::Condition(_) => {}
        }
    }
    out
}

/// Adjacency table and human-readable labels for `:BLOCKER:` dependency edges.
struct BlockerGraph {
    /// Directed edges `dependent -> [blocker, ...]`.
    edges: HashMap<GraphNode, Vec<GraphNode>>,
    /// Display label (`:ID:` or heading title) for each node.
    labels: HashMap<GraphNode, String>,
}

/// Builds the workspace `:BLOCKER:` graph across `all_docs`.
fn build_blocker_graph(all_docs: &[&OrgDocument]) -> BlockerGraph {
    let mut edges: HashMap<GraphNode, Vec<GraphNode>> = HashMap::new();
    let mut labels: HashMap<GraphNode, String> = HashMap::new();

    for &doc in all_docs {
        if doc.file_id().is_some() {
            let fnode = GraphNode::from_doc(doc, FILE_NODE_ENTRY_INDEX);
            labels.insert(fnode, node_display_label(doc, FILE_NODE_ENTRY_INDEX));
        }
        for idx in 0..doc.entries.len() {
            let src_node = GraphNode::from_doc(doc, idx);
            labels.insert(src_node.clone(), node_display_label(doc, idx));
            let Some(raw_blocker) = doc.property(idx, "BLOCKER") else {
                continue;
            };
            let (exprs, _) = parse_edna(raw_blocker);
            if exprs.is_empty() {
                continue;
            }
            let ctx = EdnaContext {
                all_docs,
                doc,
                entry_idx: idx,
            };
            let resolved = collect_resolved_targets(&exprs, &ctx);
            let adj = edges.entry(src_node).or_default();
            for target in resolved {
                let file = target
                    .file
                    .canonicalize()
                    .unwrap_or_else(|_| target.file.clone());
                let dst = GraphNode {
                    file,
                    entry_idx: target.entry_idx,
                };
                if !adj.contains(&dst) {
                    adj.push(dst);
                }
            }
        }
    }

    BlockerGraph { edges, labels }
}

/// Returns the preferred human-readable label (`:ID:` if present, else heading/file title).
fn node_display_label(doc: &OrgDocument, entry_idx: usize) -> String {
    if let Some(id) = doc.property(entry_idx, "ID") {
        return id.to_string();
    }
    if let Some(entry) = doc.entries.get(entry_idx) {
        return entry.title.clone();
    }
    doc.file_title_or_stem()
}

/// Reconstructs the shortest BFS path `start -> ... -> curr -> goal`.
fn reconstruct_bfs_path(
    start: &GraphNode,
    curr: &GraphNode,
    goal: &GraphNode,
    parent: &HashMap<GraphNode, GraphNode>,
) -> Vec<GraphNode> {
    let mut chain = vec![goal.clone(), curr.clone()];
    let mut walk = curr.clone();
    while let Some(prev) = parent.get(&walk) {
        chain.push(prev.clone());
        if prev == start {
            break;
        }
        walk = prev.clone();
    }
    chain.reverse();
    chain
}

/// Finds the shortest directed path `start ->* goal` (allowing `start == goal` non-empty cycles).
fn bfs_cycle_or_path(
    edges: &HashMap<GraphNode, Vec<GraphNode>>,
    start: &GraphNode,
    goal: &GraphNode,
) -> Option<Vec<GraphNode>> {
    let first_neighbors = edges.get(start)?;
    let mut queue: VecDeque<GraphNode> = VecDeque::new();
    let mut parent: HashMap<GraphNode, GraphNode> = HashMap::new();
    let mut visited: HashSet<GraphNode> = HashSet::new();

    for next in first_neighbors {
        if next == goal {
            return Some(vec![start.clone(), goal.clone()]);
        }
        if visited.insert(next.clone()) {
            parent.insert(next.clone(), start.clone());
            queue.push_back(next.clone());
        }
    }

    while let Some(curr) = queue.pop_front() {
        let Some(neighbors) = edges.get(&curr) else {
            continue;
        };
        for next in neighbors {
            if next == goal {
                return Some(reconstruct_bfs_path(start, &curr, goal, &parent));
            }
            if visited.insert(next.clone()) {
                parent.insert(next.clone(), curr.clone());
                queue.push_back(next.clone());
            }
        }
    }

    None
}

/// Detects whether `(doc, entry_idx)` participates in a `:BLOCKER:` dependency cycle.
///
/// Returns the ordered cycle labels (e.g., `["a", "b", "a"]`) when a cycle exists.
pub fn find_entry_blocker_cycle(
    all_docs: &[&OrgDocument],
    doc: &OrgDocument,
    entry_idx: usize,
) -> Option<Vec<String>> {
    let graph = build_blocker_graph(all_docs);
    let node = GraphNode::from_doc(doc, entry_idx);
    let cycle_nodes = bfs_cycle_or_path(&graph.edges, &node, &node)?;
    Some(format_node_labels(&cycle_nodes, &graph.labels))
}

/// Checks whether wiring `blocker` as a `:BLOCKER:` of `blocked` would create a cycle.
///
/// Returns `Some(cycle_labels)` when `blocker == blocked` or `blocked` is already
/// reachable from `blocker` via existing `:BLOCKER:` edges.
pub fn would_create_blocker_cycle(
    all_docs: &[&OrgDocument],
    blocker: (&OrgDocument, usize),
    blocked: (&OrgDocument, usize),
) -> Option<Vec<String>> {
    let graph = build_blocker_graph(all_docs);
    let blocker_node = GraphNode::from_doc(blocker.0, blocker.1);
    let blocked_node = GraphNode::from_doc(blocked.0, blocked.1);

    if blocker_node == blocked_node {
        let lbl = graph
            .labels
            .get(&blocked_node)
            .cloned()
            .unwrap_or_else(|| node_display_label(blocked.0, blocked.1));
        return Some(vec![lbl.clone(), lbl]);
    }

    let mut path = bfs_cycle_or_path(&graph.edges, &blocker_node, &blocked_node)?;
    path.insert(0, blocked_node);
    Some(format_node_labels(&path, &graph.labels))
}

/// Maps [`GraphNode`]s to their human-readable labels.
fn format_node_labels(nodes: &[GraphNode], labels: &HashMap<GraphNode, String>) -> Vec<String> {
    nodes
        .iter()
        .map(|n| {
            labels
                .get(n)
                .cloned()
                .unwrap_or_else(|| format!("{}:{}", n.file.display(), n.entry_idx))
        })
        .collect()
}

/// Returns `true` when every non-finder expression in `clause` is `Condition::Done`.
fn clause_is_default_done_blocker(clause: &[EdnaExpr]) -> bool {
    clause.iter().all(|e| {
        matches!(
            e,
            EdnaExpr::Finder(_) | EdnaExpr::Condition(Condition::Done)
        )
    })
}

/// Merges `blocker_id` into an existing `:BLOCKER:` property string without duplicates.
pub fn merge_blocker_id(existing: Option<&str>, blocker_id: &str) -> String {
    let Some(raw) = existing.map(str::trim).filter(|s| !s.is_empty()) else {
        return format!("ids(\"{blocker_id}\")");
    };
    let (exprs, errors) = parse_edna(raw);
    if !errors.is_empty() || exprs.is_empty() {
        return format!("{raw} ids(\"{blocker_id}\")");
    }

    let mut clauses = split_into_clauses(exprs);
    for clause in &mut clauses {
        if !clause_is_default_done_blocker(clause) {
            continue;
        }
        for expr in clause.iter_mut() {
            if let EdnaExpr::Finder(Finder::Ids(ref mut ids)) = expr {
                if ids.iter().any(|existing_id| existing_id == blocker_id) {
                    return raw.to_string();
                }
                ids.push(blocker_id.to_string());
                let flat: Vec<EdnaExpr> = clauses.into_iter().flatten().collect();
                return format_edna_exprs(&flat);
            }
        }
    }

    let mut flat: Vec<EdnaExpr> = clauses.into_iter().flatten().collect();
    flat.push(EdnaExpr::Finder(Finder::Ids(vec![blocker_id.to_string()])));
    format_edna_exprs(&flat)
}

/// Removes `blocker_id` from `:BLOCKER:`, returning `None` when no blockers remain.
pub fn remove_blocker_id(existing: Option<&str>, blocker_id: &str) -> Option<String> {
    let raw = existing.map(str::trim).filter(|s| !s.is_empty())?;
    let (exprs, _) = parse_edna(raw);
    let clauses = split_into_clauses(exprs);
    let mut kept = Vec::new();

    for clause in clauses {
        if let Some(pruned) = prune_id_from_clause(clause, blocker_id) {
            kept.extend(pruned);
        }
    }

    if kept.is_empty() {
        None
    } else {
        Some(format_edna_exprs(&kept))
    }
}

/// Merges `ids("<blocked_id>") todo!("<trigger_state>")` into `:TRIGGER:` without duplicates.
pub fn merge_trigger_id(existing: Option<&str>, blocked_id: &str, trigger_state: &str) -> String {
    let Some(raw) = existing.map(str::trim).filter(|s| !s.is_empty()) else {
        return format!("ids(\"{blocked_id}\") todo!(\"{trigger_state}\")");
    };
    let (exprs, errors) = parse_edna(raw);
    if !errors.is_empty() || exprs.is_empty() {
        return format!("{raw} ids(\"{blocked_id}\") todo!(\"{trigger_state}\")");
    }

    let mut clauses = split_into_clauses(exprs);
    for clause in &mut clauses {
        let matches_todo = clause.iter().any(|e| {
            matches!(
                e,
                EdnaExpr::Action(Action::Todo(st)) if st.eq_ignore_ascii_case(trigger_state)
            )
        });
        if !matches_todo {
            continue;
        }
        for expr in clause.iter_mut() {
            if let EdnaExpr::Finder(Finder::Ids(ref mut ids)) = expr {
                if ids.iter().any(|id| id == blocked_id) {
                    return raw.to_string();
                }
                ids.push(blocked_id.to_string());
                let flat: Vec<EdnaExpr> = clauses.into_iter().flatten().collect();
                return format_edna_exprs(&flat);
            }
        }
    }

    let mut flat: Vec<EdnaExpr> = clauses.into_iter().flatten().collect();
    flat.push(EdnaExpr::Finder(Finder::Ids(vec![blocked_id.to_string()])));
    flat.push(EdnaExpr::Action(Action::Todo(trigger_state.to_string())));
    format_edna_exprs(&flat)
}

/// Removes `blocked_id` from `:TRIGGER:`, returning `None` when no trigger clauses remain.
pub fn remove_trigger_id(existing: Option<&str>, blocked_id: &str) -> Option<String> {
    let raw = existing.map(str::trim).filter(|s| !s.is_empty())?;
    let (exprs, _) = parse_edna(raw);
    let clauses = split_into_clauses(exprs);
    let mut kept = Vec::new();

    for clause in clauses {
        if let Some(pruned) = prune_id_from_clause(clause, blocked_id) {
            kept.extend(pruned);
        }
    }

    if kept.is_empty() {
        None
    } else {
        Some(format_edna_exprs(&kept))
    }
}

/// Splits a flat [`EdnaExpr`] sequence into finder-scoped clauses.
fn split_into_clauses(exprs: Vec<EdnaExpr>) -> Vec<Vec<EdnaExpr>> {
    let mut clauses: Vec<Vec<EdnaExpr>> = Vec::new();
    let mut current: Vec<EdnaExpr> = Vec::new();
    let mut seen_non_finder = false;

    for expr in exprs {
        let is_finder = matches!(expr, EdnaExpr::Finder(_));
        if is_finder && seen_non_finder && !current.is_empty() {
            clauses.push(std::mem::take(&mut current));
            seen_non_finder = false;
        }
        if !is_finder {
            seen_non_finder = true;
        }
        current.push(expr);
    }
    if !current.is_empty() {
        clauses.push(current);
    }
    clauses
}

/// Removes `target_id` from `Finder::Ids` in `clause`, dropping the clause if all finders vanish.
fn prune_id_from_clause(clause: Vec<EdnaExpr>, target_id: &str) -> Option<Vec<EdnaExpr>> {
    let had_finder = clause.iter().any(|e| matches!(e, EdnaExpr::Finder(_)));
    let mut updated = Vec::with_capacity(clause.len());

    for expr in clause {
        match expr {
            EdnaExpr::Finder(Finder::Ids(mut ids)) => {
                ids.retain(|id| id != target_id);
                if !ids.is_empty() {
                    updated.push(EdnaExpr::Finder(Finder::Ids(ids)));
                }
            }
            other => updated.push(other),
        }
    }

    let still_has_finder = updated.iter().any(|e| matches!(e, EdnaExpr::Finder(_)));
    if (had_finder && !still_has_finder) || updated.is_empty() {
        None
    } else {
        Some(updated)
    }
}
