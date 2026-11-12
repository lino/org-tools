// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scope filtering logic for `org roam` node queries.

use std::path::Path;

use org_tools_core::document::OrgDocument;
use org_tools_core::locator::FILE_NODE_ENTRY_INDEX;
use org_tools_core::roam::RoamNode;
use org_tools_core::scope::{resolve_effective_scope, ScopeQuery, ScopeRegistry};
use org_tools_core::source::SourceFile;

use super::roam::RoamScopeArgs;

/// Resolves the effective scan directory from workspace argument or positional paths.
pub(crate) fn resolve_roam_scan_dir(scope: &RoamScopeArgs) -> &Path {
    scope
        .workspace
        .as_deref()
        .or_else(|| {
            scope.paths.first().map(|p| {
                if p.is_dir() {
                    p.as_path()
                } else {
                    p.parent().unwrap_or(p)
                }
            })
        })
        .unwrap_or_else(|| Path::new("."))
}

/// Scope filter parameters for `org roam node`.
#[derive(Debug, Clone, Default)]
pub(crate) struct RoamNodeScopeFilter<'a> {
    /// Target scope expression constraint (e.g. "global", "landscape:core").
    pub(crate) filter_scope: Option<&'a str>,
    /// Filter nodes relevant to target project slug.
    pub(crate) for_project: Option<&'a str>,
    /// Filter nodes relevant to target project landscape slug.
    pub(crate) for_landscape: Option<&'a str>,
    /// Filter nodes matching target repository forge path or prefix.
    pub(crate) forge_path: Option<&'a str>,
    /// Exclude implicit global nodes when filtering by project, landscape, or forge.
    pub(crate) strict_scope: bool,
    /// Explicit path to scope and landscape configuration file.
    pub(crate) scope_config: Option<&'a Path>,
}

/// Filters resolved roam nodes according to scope, project, landscape, or forge constraints.
pub(crate) fn filter_nodes_by_scope(
    nodes: Vec<RoamNode>,
    filter: &RoamNodeScopeFilter<'_>,
    workspace_dir: &Path,
) -> Vec<RoamNode> {
    let scope_q = ScopeQuery {
        scope_expr: filter.filter_scope.map(str::to_string),
        for_project: filter.for_project.map(str::to_string),
        for_landscape: filter.for_landscape.map(str::to_string),
        forge_path: filter.forge_path.map(str::to_string),
        strict_scope: filter.strict_scope,
    };
    if scope_q.is_empty() {
        return nodes;
    }
    let reg = ScopeRegistry::load_for_dir(workspace_dir, filter.scope_config).unwrap_or_default();
    nodes
        .into_iter()
        .filter(|node| {
            let Ok(content) = std::fs::read_to_string(&node.file) else {
                return false;
            };
            let src = SourceFile::new(&node.file, content);
            let doc = OrgDocument::from_source(&src);
            let entry_idx = if node.level == 0 {
                FILE_NODE_ENTRY_INDEX
            } else {
                doc.find_by_id(&node.id).unwrap_or_else(|| {
                    doc.entries
                        .iter()
                        .position(|e| e.heading_line == node.line)
                        .unwrap_or(0)
                })
            };
            let raw_scope = resolve_effective_scope(&doc, entry_idx);
            scope_q.matches(&raw_scope, &node.tags, &reg)
        })
        .collect()
}
