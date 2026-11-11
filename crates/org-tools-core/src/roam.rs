// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pure-Rust `org-roam` v2 node, alias, reference, and `[[id:...]]` link graph engine.
//!
//! Indexes plain-text `.org` documents without reading or modifying Emacs's
//! `org-roam.db`. Supports level-0 file `:PROPERTIES:` drawers, heading drawers,
//! shell-quoted `:ROAM_ALIASES:` and `:ROAM_REFS:` tokens, `:ROAM_EXCLUDE: t`
//! filtering, and protected-block-aware `[[id:...]]` link extraction.
//!
//! Spec:
//! - Org Mode Manual: <https://orgmode.org/manual/Property-Syntax.html>
//! - Org Syntax: <https://orgmode.org/worg/org-syntax.html#Property_Drawers>
//! - org-roam v2 Manual: <https://www.orgroam.com/manual.html>

mod graph;
mod links;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::document::OrgDocument;

pub use graph::RoamIndex;
pub use links::{extract_roam_links, scan_id_links, RawIdLink};

// region Public Types

/// An indexed `org-roam` v2 file-level (`level == 0`) or heading (`level >= 1`) node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoamNode {
    /// Trimmed `:ID:` property value uniquely identifying this node.
    pub id: String,
    /// Optional `:ID_SEED:` provenance URN from the node's own drawer.
    pub id_seed: Option<String>,
    /// Node title (`#+TITLE:` or file stem for level-0 nodes; heading title for headings).
    pub title: String,
    /// Path to the Org source file defining this node.
    pub file: String,
    /// One-based line number (`1` for level-0 file nodes, `heading_line` for headings).
    pub line: usize,
    /// Outline level (`0` for level-0 file nodes, `>= 1` for headings).
    pub level: usize,
    /// Optional TODO keyword attached to a heading node.
    pub todo_keyword: Option<String>,
    /// Optional priority cookie character attached to a heading node.
    pub priority: Option<String>,
    /// Effective tags (`#+FILETAGS:` + inherited ancestor tags + local heading tags).
    pub tags: Vec<String>,
    /// Ancestor outline path titles from root to parent (excluding self).
    pub olp: Vec<String>,
    /// Aliases parsed from `:ROAM_ALIASES:` (including `:ROAM_ALIASES+:` continuations).
    pub aliases: Vec<String>,
    /// External references parsed from `:ROAM_REFS:` (including `:ROAM_REFS+:` continuations).
    pub refs: Vec<String>,
    /// Optional vault name or directory identifier when opened across federated `org-roam` roots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roam_dir: Option<String>,
}

/// A directed `[[id:...]]` link extracted from unprotected Org prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoamLink {
    /// Nearest enclosing non-excluded `RoamNode` `:ID:`, if any.
    pub source_id: Option<String>,
    /// Path to the Org file containing the link.
    pub source_file: String,
    /// Target `:ID:` with any `::search` suffix stripped.
    pub target_id: String,
    /// Optional link description from `[[id:<target>][<description>]]`.
    pub description: Option<String>,
    /// One-based line number of the link.
    pub line: usize,
    /// Trimmed source line containing the link.
    pub context: String,
}

/// An incoming `[[id:...]]` backlink enriched with resolved source node metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoamBacklink {
    /// Enclosing non-excluded source node `:ID:`, if any.
    pub source_id: Option<String>,
    /// Title of the enclosing source node when resolved.
    pub source_title: Option<String>,
    /// Path to the Org file containing the incoming link.
    pub source_file: String,
    /// Target node `:ID:` referenced by the link.
    pub target_id: String,
    /// One-based line number of the incoming link.
    pub line: usize,
    /// Optional link description text.
    pub description: Option<String>,
    /// Trimmed source line containing the incoming link.
    pub context: String,
}

/// An outgoing `[[id:...]]` link enriched with target resolution metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoamOutgoingLink {
    /// Source node `:ID:` owning this outgoing link.
    pub source_id: String,
    /// Path to the Org file containing the outgoing link.
    pub source_file: String,
    /// Target `:ID:` extracted from `[[id:<target>]]`.
    pub target_id: String,
    /// Title of the target node when resolved in the workspace index.
    pub target_title: Option<String>,
    /// File path of the target node when resolved in the workspace index.
    pub target_file: Option<String>,
    /// Whether `target_id` resolved to an indexed `RoamNode`.
    pub resolved: bool,
    /// One-based line number of the outgoing link.
    pub line: usize,
    /// Optional link description text.
    pub description: Option<String>,
    /// Trimmed source line containing the outgoing link.
    pub context: String,
}

// endregion

// region Property & Token Helpers

/// Looks up a property key in `props` using ASCII case-insensitive comparison.
pub(crate) fn lookup_prop_ci<'a>(props: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    if let Some(val) = props.get(key) {
        return Some(val.as_str());
    }
    props
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// Returns `true` when `props` sets `:ROAM_EXCLUDE:` to `"t"` (case-insensitive).
pub(crate) fn is_roam_excluded(props: &HashMap<String, String>) -> bool {
    lookup_prop_ci(props, "ROAM_EXCLUDE").is_some_and(|v| v.trim().eq_ignore_ascii_case("t"))
}

/// Returns `true` when `doc` defines a file-level or heading `:ID:` matching `target_id` case-insensitively.
pub fn doc_contains_id_ci(doc: &OrgDocument, target_id: &str) -> bool {
    let needle = target_id.trim();
    if needle.is_empty() {
        return false;
    }
    let file_match = lookup_prop_ci(&doc.file_properties, "ID")
        .map(str::trim)
        .is_some_and(|id| id.eq_ignore_ascii_case(needle));
    if file_match {
        return true;
    }
    doc.entries.iter().any(|entry| {
        lookup_prop_ci(&entry.properties, "ID")
            .map(str::trim)
            .is_some_and(|id| id.eq_ignore_ascii_case(needle))
    })
}

/// Consumes a double-quoted token supporting `\"` and `\\` escapes.
fn consume_quoted_token(chars: &mut std::str::Chars<'_>) -> String {
    let mut buf = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(escaped) = chars.next() {
                buf.push(escaped);
            }
        } else if ch == '"' {
            break;
        } else {
            buf.push(ch);
        }
    }
    buf
}

/// Consumes an unquoted whitespace-delimited token starting with `first`.
fn consume_bare_token(first: char, chars: &mut std::str::Chars<'_>) -> String {
    let mut buf = String::from(first);
    for ch in chars.by_ref() {
        if ch.is_whitespace() {
            break;
        }
        buf.push(ch);
    }
    buf
}

/// Splits shell-quoted and whitespace-delimited tokens from `:ROAM_ALIASES:` or `:ROAM_REFS:`.
pub fn split_roam_tokens(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        let tok = if ch == '"' {
            consume_quoted_token(&mut chars)
        } else {
            consume_bare_token(ch, &mut chars)
        };
        if !tok.is_empty() {
            tokens.push(tok);
        }
    }
    tokens
}

// endregion

// region Node Extraction

/// Deduplicates `items` while preserving their first-seen order.
fn dedup_preserve_order(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

/// Collects `#+FILETAGS:` + ancestor heading tags + local heading tags in top-down order.
fn collect_heading_tags(doc: &OrgDocument, entry_idx: usize) -> Vec<String> {
    let mut chain = Vec::new();
    let mut cur = Some(entry_idx);
    while let Some(idx) = cur {
        chain.push(idx);
        cur = doc.entries[idx].parent;
    }
    chain.reverse();

    let combined = doc.filetags.iter().cloned().chain(
        chain
            .into_iter()
            .flat_map(|i| doc.entries[i].tags.iter().cloned()),
    );
    dedup_preserve_order(combined)
}

/// Returns ancestor heading titles from root to parent, excluding `entry_idx` itself.
fn parent_outline_path(doc: &OrgDocument, entry_idx: usize) -> Vec<String> {
    let mut path = doc.outline_path(entry_idx);
    path.pop();
    path.into_iter().map(str::to_string).collect()
}

/// Extracts an optional trimmed non-empty property value from `props`.
fn non_empty_prop(props: &HashMap<String, String>, key: &str) -> Option<String> {
    lookup_prop_ci(props, key)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Builds a level-0 file [`RoamNode`] when `doc.file_properties` defines `:ID:` and is not excluded.
fn build_file_node(doc: &OrgDocument) -> Option<RoamNode> {
    if is_roam_excluded(&doc.file_properties) {
        return None;
    }
    let id = non_empty_prop(&doc.file_properties, "ID")?;
    let aliases = lookup_prop_ci(&doc.file_properties, "ROAM_ALIASES")
        .map(split_roam_tokens)
        .unwrap_or_default();
    let refs = lookup_prop_ci(&doc.file_properties, "ROAM_REFS")
        .map(split_roam_tokens)
        .unwrap_or_default();

    Some(RoamNode {
        id,
        id_seed: non_empty_prop(&doc.file_properties, "ID_SEED"),
        title: doc.file_title_or_stem(),
        file: doc.file.display().to_string(),
        line: 1,
        level: 0,
        todo_keyword: None,
        priority: None,
        tags: dedup_preserve_order(doc.filetags.iter().cloned()),
        olp: Vec::new(),
        aliases,
        refs,
        roam_dir: None,
    })
}

/// Builds a heading [`RoamNode`] for `entry_idx` when its own drawer defines `:ID:` and is not excluded.
fn build_heading_node(doc: &OrgDocument, entry_idx: usize) -> Option<RoamNode> {
    let entry = doc.entries.get(entry_idx)?;
    if is_roam_excluded(&entry.properties) {
        return None;
    }
    let id = non_empty_prop(&entry.properties, "ID")?;
    let aliases = lookup_prop_ci(&entry.properties, "ROAM_ALIASES")
        .map(split_roam_tokens)
        .unwrap_or_default();
    let refs = lookup_prop_ci(&entry.properties, "ROAM_REFS")
        .map(split_roam_tokens)
        .unwrap_or_default();

    Some(RoamNode {
        id,
        id_seed: non_empty_prop(&entry.properties, "ID_SEED"),
        title: entry.title.clone(),
        file: doc.file.display().to_string(),
        line: entry.heading_line,
        level: entry.level,
        todo_keyword: entry.keyword.clone(),
        priority: entry.priority.map(|c| c.to_string()),
        tags: collect_heading_tags(doc, entry_idx),
        olp: parent_outline_path(doc, entry_idx),
        aliases,
        refs,
        roam_dir: None,
    })
}

/// Extracts all non-excluded file and heading [`RoamNode`]s from `doc`.
pub fn extract_roam_nodes(doc: &OrgDocument) -> Vec<RoamNode> {
    let mut nodes = Vec::new();
    if let Some(file_node) = build_file_node(doc) {
        nodes.push(file_node);
    }
    for idx in 0..doc.entries.len() {
        if let Some(heading_node) = build_heading_node(doc, idx) {
            nodes.push(heading_node);
        }
    }
    nodes
}

// endregion
