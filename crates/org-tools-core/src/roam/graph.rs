// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Workspace `org-roam` v2 graph index, node/backlink queries, and Mermaid/DOT exporters.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::document::OrgDocument;
use crate::export_md::normalize_workspace_path;
use crate::locator::{resolve_in_document, OrgLocator};
use crate::roam::{
    extract_roam_links, extract_roam_nodes, lookup_prop_ci, RoamBacklink, RoamLink, RoamNode,
    RoamOutgoingLink,
};
use crate::source::SourceFile;

/// In-memory `org-roam` v2 workspace graph index built from plain-text `.org` files.
#[derive(Debug, Default)]
pub struct RoamIndex {
    /// All non-excluded [`RoamNode`]s sorted deterministically by `(file, line)`.
    pub nodes: Vec<RoamNode>,
    /// All extracted [`RoamLink`] edges sorted deterministically by `(source_file, line)`.
    pub links: Vec<RoamLink>,
    /// Parsed [`OrgDocument`]s retained for file-scoped `OrgLocator` resolution.
    pub docs: Vec<OrgDocument>,
    /// Number of `.org` files scanned while constructing this index.
    pub files_scanned: usize,
}

impl RoamIndex {
    /// Builds a [`RoamIndex`] from an ordered slice of `.org` file paths on disk.
    pub fn from_files(files: &[PathBuf]) -> Result<Self, String> {
        let mut sources = Vec::with_capacity(files.len());
        for path in files {
            let src = SourceFile::from_path(path)
                .map_err(|e| format!("error reading {}: {e}", path.display()))?;
            sources.push(src);
        }
        Ok(Self::from_sources(&sources))
    }

    /// Builds a [`RoamIndex`] from in-memory [`SourceFile`] buffers.
    pub fn from_sources(sources: &[SourceFile]) -> Self {
        let mut nodes = Vec::new();
        let mut links = Vec::new();
        let mut docs = Vec::with_capacity(sources.len());

        for source in sources {
            let doc = OrgDocument::from_source(source);
            nodes.extend(extract_roam_nodes(&doc));
            links.extend(extract_roam_links(source, &doc));
            docs.push(doc);
        }

        nodes.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
        links.sort_by(|a, b| (&a.source_file, a.line).cmp(&(&b.source_file, b.line)));

        Self {
            nodes,
            links,
            docs,
            files_scanned: sources.len(),
        }
    }

    /// Resolves `query` by `id:<uuid>`, `:ID:`, title, `ROAM_ALIASES`, or optional `OrgLocator`.
    pub fn resolve_nodes(&self, query: &str, allow_locator: bool) -> Vec<RoamNode> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        if let Some(id_only) = strip_id_prefix_ci(trimmed) {
            return self.match_by_id(id_only);
        }

        let direct: Vec<RoamNode> = self
            .nodes
            .iter()
            .filter(|node| node_matches_text(node, trimmed))
            .cloned()
            .collect();
        if !direct.is_empty() || !allow_locator || !trimmed.contains("::") {
            return direct;
        }
        self.resolve_via_locator(trimmed)
    }

    /// Collects incoming [`RoamBacklink`]s pointing to any node in `targets`, sorted by `(source_file, line)`.
    pub fn backlinks_for(&self, targets: &[RoamNode]) -> Vec<RoamBacklink> {
        let node_by_id = self.node_lookup_map();
        let mut out: Vec<RoamBacklink> = self
            .links
            .iter()
            .filter(|link| {
                targets
                    .iter()
                    .any(|t| t.id.eq_ignore_ascii_case(&link.target_id))
            })
            .map(|link| {
                let source_title = link
                    .source_id
                    .as_deref()
                    .and_then(|sid| node_by_id.get(&sid.to_ascii_lowercase()))
                    .map(|n| n.title.clone());
                RoamBacklink {
                    source_id: link.source_id.clone(),
                    source_title,
                    source_file: link.source_file.clone(),
                    target_id: link.target_id.clone(),
                    line: link.line,
                    description: link.description.clone(),
                    context: link.context.clone(),
                }
            })
            .collect();
        out.sort_by(|a, b| (&a.source_file, a.line).cmp(&(&b.source_file, b.line)));
        out
    }

    /// Collects outgoing [`RoamOutgoingLink`]s attributed to `sources`, sorted by `(source_file, line)`.
    pub fn outgoing_links_for(&self, sources: &[RoamNode]) -> Vec<RoamOutgoingLink> {
        let node_by_id = self.node_lookup_map();
        let mut out = Vec::new();
        for link in &self.links {
            let Some(ref sid) = link.source_id else {
                continue;
            };
            if !sources.iter().any(|s| s.id.eq_ignore_ascii_case(sid)) {
                continue;
            }
            let target_node = node_by_id
                .get(&link.target_id.to_ascii_lowercase())
                .copied();
            out.push(RoamOutgoingLink {
                source_id: sid.clone(),
                source_file: link.source_file.clone(),
                target_id: link.target_id.clone(),
                target_title: target_node.map(|n| n.title.clone()),
                target_file: target_node.map(|n| n.file.clone()),
                resolved: target_node.is_some(),
                line: link.line,
                description: link.description.clone(),
                context: link.context.clone(),
            });
        }
        out.sort_by(|a, b| (&a.source_file, a.line).cmp(&(&b.source_file, b.line)));
        out
    }

    /// Renders the indexed graph as a Mermaid `flowchart LR` diagram with safe escaped labels.
    pub fn render_mermaid(&self) -> String {
        let id_to_safe = self.safe_node_ids();
        let mut lines = vec!["flowchart LR".to_string()];
        for (idx, node) in self.nodes.iter().enumerate() {
            lines.push(format!(
                "    N{idx}[\"{}\"]",
                escape_mermaid_label(&node.title)
            ));
        }
        for (src_idx, dst_idx) in self.resolved_edge_pairs(&id_to_safe) {
            lines.push(format!("    N{src_idx} --> N{dst_idx}"));
        }
        let mut rendered = lines.join("\n");
        rendered.push('\n');
        rendered
    }

    /// Renders the indexed graph as a Graphviz `digraph org_roam` specification.
    pub fn render_dot(&self) -> String {
        let id_to_safe = self.safe_node_ids();
        let mut lines = vec![
            "digraph org_roam {".to_string(),
            "  rankdir=LR;".to_string(),
        ];
        for (idx, node) in self.nodes.iter().enumerate() {
            lines.push(format!(
                "  N{idx} [label=\"{}\"];",
                escape_dot_label(&node.title)
            ));
        }
        for (src_idx, dst_idx) in self.resolved_edge_pairs(&id_to_safe) {
            lines.push(format!("  N{src_idx} -> N{dst_idx};"));
        }
        lines.push("}".to_string());
        let mut rendered = lines.join("\n");
        rendered.push('\n');
        rendered
    }

    /// Returns all [`RoamNode`]s whose `:ID:` matches `target_id` case-insensitively.
    fn match_by_id(&self, target_id: &str) -> Vec<RoamNode> {
        let needle = target_id.trim();
        self.nodes
            .iter()
            .filter(|n| n.id.eq_ignore_ascii_case(needle))
            .cloned()
            .collect()
    }

    /// Resolves a file-scoped `OrgLocator` against `self.docs` and returns the matching [`RoamNode`].
    fn resolve_via_locator(&self, raw_locator: &str) -> Vec<RoamNode> {
        let Ok(parsed) = OrgLocator::parse(raw_locator) else {
            return Vec::new();
        };
        for doc in &self.docs {
            if let Some(rebound) = rebind_if_matching_doc(&parsed, &doc.file) {
                if let Ok(resolved) = resolve_in_document(&rebound, doc) {
                    let id_opt = if resolved.is_file_node() {
                        lookup_prop_ci(&doc.file_properties, "ID")
                    } else {
                        doc.entries
                            .get(resolved.entry_index)
                            .and_then(|e| lookup_prop_ci(&e.properties, "ID"))
                    };
                    if let Some(id) = id_opt {
                        return self.match_by_id(id);
                    }
                }
            }
        }
        Vec::new()
    }

    /// Maps lowercase node `:ID:` strings to `&RoamNode` references.
    fn node_lookup_map(&self) -> HashMap<String, &RoamNode> {
        let mut map = HashMap::with_capacity(self.nodes.len());
        for node in &self.nodes {
            map.entry(node.id.to_ascii_lowercase()).or_insert(node);
        }
        map
    }

    /// Maps lowercase node `:ID:` strings to their deterministic `0..N` node index.
    fn safe_node_ids(&self) -> HashMap<String, usize> {
        let mut map = HashMap::with_capacity(self.nodes.len());
        for (idx, node) in self.nodes.iter().enumerate() {
            map.entry(node.id.to_ascii_lowercase()).or_insert(idx);
        }
        map
    }

    /// Returns deduplicated `(source_idx, target_idx)` pairs for links between indexed nodes.
    fn resolved_edge_pairs(&self, id_to_safe: &HashMap<String, usize>) -> Vec<(usize, usize)> {
        let mut seen = BTreeSet::new();
        let mut pairs = Vec::new();
        for link in &self.links {
            let Some(ref src) = link.source_id else {
                continue;
            };
            let src_idx = id_to_safe.get(&src.to_ascii_lowercase());
            let dst_idx = id_to_safe.get(&link.target_id.to_ascii_lowercase());
            if let (Some(&s), Some(&d)) = (src_idx, dst_idx) {
                if seen.insert((s, d)) {
                    pairs.push((s, d));
                }
            }
        }
        pairs
    }
}

/// Strips an ASCII case-insensitive `"id:"` prefix when present.
fn strip_id_prefix_ci(input: &str) -> Option<&str> {
    input
        .get(..3)
        .is_some_and(|p| p.eq_ignore_ascii_case("id:"))
        .then(|| &input[3..])
}

/// Returns `true` when `node` matches `query` by exact `:ID:`, `title`, or `ROAM_ALIASES` token.
fn node_matches_text(node: &RoamNode, query: &str) -> bool {
    node.id.eq_ignore_ascii_case(query)
        || node.title.eq_ignore_ascii_case(query)
        || node.aliases.iter().any(|a| a.eq_ignore_ascii_case(query))
}

/// Rebinds a file-scoped [`OrgLocator`] to `doc_file` when `locator`'s path matches `doc_file`.
fn rebind_if_matching_doc(locator: &OrgLocator, doc_file: &Path) -> Option<OrgLocator> {
    let loc_file = match locator {
        OrgLocator::Id(_) => return Some(locator.clone()),
        OrgLocator::CustomId { file, .. }
        | OrgLocator::OutlinePath { file, .. }
        | OrgLocator::LineRef { file, .. } => file,
    };
    let matches = doc_file == loc_file
        || doc_file.ends_with(loc_file)
        || normalize_workspace_path(doc_file) == normalize_workspace_path(loc_file);
    if !matches {
        return None;
    }
    Some(match locator {
        OrgLocator::Id(id) => OrgLocator::Id(id.clone()),
        OrgLocator::CustomId { custom_id, .. } => OrgLocator::CustomId {
            file: doc_file.to_path_buf(),
            custom_id: custom_id.clone(),
        },
        OrgLocator::OutlinePath { path, .. } => OrgLocator::OutlinePath {
            file: doc_file.to_path_buf(),
            path: path.clone(),
        },
        OrgLocator::LineRef { line, .. } => OrgLocator::LineRef {
            file: doc_file.to_path_buf(),
            line: *line,
        },
    })
}

/// Escapes a node title for safe inclusion inside a Mermaid `N0["..."]` node label.
fn escape_mermaid_label(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for ch in title.chars() {
        match ch {
            '\\' => out.push_str("#92;"),
            '"' => out.push_str("#quot;"),
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            '\r' | '\n' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// Escapes a node title for safe inclusion inside a Graphviz DOT quoted string.
fn escape_dot_label(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for ch in title.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' | '\n' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}
