// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! SQLite-backed `org-roam` v2 node, backlink, outgoing-link, graph, and edna target queries.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, Row};

use crate::document::OrgDocument;
use crate::export_md::normalize_workspace_path;
use crate::files::join_slash_rel;
use crate::locator::{resolve_in_document, OrgLocator};
use crate::roam::{lookup_prop_ci, RoamBacklink, RoamIndex, RoamLink, RoamNode, RoamOutgoingLink};
use crate::source::SourceFile;

// region Row Decoders & Path Helpers

/// Formats `join_slash_rel(display_root, rel_path)` consistently with `WalkBuilder::new(display_root)`.
fn display_file_path(display_root: &Path, rel_path: &str) -> String {
    join_slash_rel(display_root, rel_path).display().to_string()
}

/// Decodes a JSON `Vec<String>` column, defaulting to an empty vector on corrupt metadata.
fn decode_str_vec(raw: String) -> Vec<String> {
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Maps a 12-column `nodes` row into a [`RoamNode`] rooted at `display_root`.
fn row_to_roam_node(row: &Row<'_>, display_root: &Path) -> rusqlite::Result<RoamNode> {
    let rel_path: String = row.get(1)?;
    Ok(RoamNode {
        id: row.get(0)?,
        file: display_file_path(display_root, &rel_path),
        id_seed: row.get(2)?,
        title: row.get(3)?,
        line: row.get::<_, i64>(4)? as usize,
        level: row.get::<_, i64>(5)? as usize,
        todo_keyword: row.get(6)?,
        priority: row.get(7)?,
        tags: decode_str_vec(row.get(8)?),
        olp: decode_str_vec(row.get(9)?),
        aliases: decode_str_vec(row.get(10)?),
        refs: decode_str_vec(row.get(11)?),
    })
}

/// Decodes a 6-column `links` row into a [`RoamLink`] rooted at `display_root`.
fn row_to_roam_link(r: &Row<'_>, display_root: &Path) -> rusqlite::Result<RoamLink> {
    let rel: String = r.get(0)?;
    Ok(RoamLink {
        source_file: display_file_path(display_root, &rel),
        source_id: r.get(1)?,
        target_id: r.get(2)?,
        description: r.get(3)?,
        line: r.get::<_, i64>(4)? as usize,
        context: r.get(5)?,
    })
}

/// Strips an ASCII case-insensitive `"id:"` prefix when present.
fn strip_id_prefix(input: &str) -> Option<&str> {
    input
        .get(..3)
        .is_some_and(|p| p.eq_ignore_ascii_case("id:"))
        .then(|| &input[3..])
}

// endregion

// region Node & Locator Resolution

/// Fetches all [`RoamNode`] rows matching `target_id` case-insensitively via `idx_nodes_id_lower`.
fn query_nodes_by_id(
    conn: &Connection,
    display_root: &Path,
    target_id: &str,
) -> Result<Vec<RoamNode>, String> {
    let needle = target_id.trim();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare_cached(
            "SELECT n.id, n.rel_path, n.id_seed, n.title, n.line, n.level, n.todo_keyword, n.priority,
                    n.tags_json, n.olp_json, n.aliases_json, n.refs_json
             FROM nodes n JOIN files f ON f.rel_path = n.rel_path
             WHERE n.roam_excluded = 0 AND f.is_roam_dup = 0 AND lower(n.id) = lower(?1)",
        )
        .map_err(|e| format!("failed to prepare node id lookup: {e}"))?;
    let rows = stmt
        .query_map(params![needle], |r| row_to_roam_node(r, display_root))
        .map_err(|e| format!("failed to execute node id lookup: {e}"))?;
    let mut nodes = Vec::new();
    for item in rows {
        nodes.push(item.map_err(|e| format!("node row decode error: {e}"))?);
    }
    nodes.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    Ok(nodes)
}

/// Rebinds `parsed` to `candidate_disp` and extracts the resolved node `:ID:` from `disk_path`.
fn resolve_id_in_single_file(
    parsed: &OrgLocator,
    disk_path: &Path,
    candidate_disp: PathBuf,
) -> Option<String> {
    let src = SourceFile::from_path(disk_path).ok()?;
    let mut doc = OrgDocument::from_source(&src);
    doc.file = candidate_disp.clone();
    let rebound = match parsed {
        OrgLocator::Id(id) => OrgLocator::Id(id.clone()),
        OrgLocator::CustomId { custom_id, .. } => OrgLocator::CustomId {
            file: candidate_disp,
            custom_id: custom_id.clone(),
        },
        OrgLocator::OutlinePath { path, .. } => OrgLocator::OutlinePath {
            file: candidate_disp,
            path: path.clone(),
        },
        OrgLocator::LineRef { line, .. } => OrgLocator::LineRef {
            file: candidate_disp,
            line: *line,
        },
    };
    let resolved = resolve_in_document(&rebound, &doc).ok()?;
    let raw_id = if resolved.is_file_node() {
        lookup_prop_ci(&doc.file_properties, "ID")?
    } else {
        lookup_prop_ci(&doc.entries.get(resolved.entry_index)?.properties, "ID")?
    };
    Some(raw_id.to_string())
}

/// Resolves a file-scoped `OrgLocator` by parsing only the single matching `.org` file on disk.
fn resolve_locator_node(
    conn: &Connection,
    workspace_root: &Path,
    display_root: &Path,
    raw_locator: &str,
) -> Result<Vec<RoamNode>, String> {
    let Ok(parsed) = OrgLocator::parse(raw_locator) else {
        return Ok(Vec::new());
    };
    let loc_file = match &parsed {
        OrgLocator::Id(id) => return query_nodes_by_id(conn, display_root, id),
        OrgLocator::CustomId { file, .. }
        | OrgLocator::OutlinePath { file, .. }
        | OrgLocator::LineRef { file, .. } => file,
    };

    let mut stmt = conn
        .prepare_cached("SELECT rel_path FROM files WHERE is_roam_dup = 0 ORDER BY rel_path ASC")
        .map_err(|e| format!("failed to prepare files lookup: {e}"))?;
    let rels = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| format!("failed to scan files table: {e}"))?;

    for item in rels {
        let rel = item.map_err(|e| e.to_string())?;
        let candidate_disp = join_slash_rel(display_root, &rel);
        let matches = Path::new(&rel) == loc_file
            || candidate_disp == *loc_file
            || candidate_disp.ends_with(loc_file)
            || normalize_workspace_path(&candidate_disp) == normalize_workspace_path(loc_file);
        if matches {
            let disk_path = join_slash_rel(workspace_root, &rel);
            if let Some(id) = resolve_id_in_single_file(&parsed, &disk_path, candidate_disp) {
                return query_nodes_by_id(conn, display_root, &id);
            }
        }
    }
    Ok(Vec::new())
}

/// Resolves `query` against SQLite `nodes` by `id:`, `:ID:`, `title`, `ROAM_ALIASES`, or `OrgLocator`.
pub(crate) fn resolve_nodes_cached(
    conn: &Connection,
    workspace_root: &Path,
    display_root: &Path,
    query: &str,
    allow_locator: bool,
) -> Result<Vec<RoamNode>, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(id_only) = strip_id_prefix(trimmed) {
        return query_nodes_by_id(conn, display_root, id_only);
    }

    let mut stmt = conn
        .prepare_cached(
            "SELECT n.id, n.rel_path, n.id_seed, n.title, n.line, n.level, n.todo_keyword, n.priority,
                    n.tags_json, n.olp_json, n.aliases_json, n.refs_json
             FROM nodes n JOIN files f ON f.rel_path = n.rel_path
             WHERE n.roam_excluded = 0 AND f.is_roam_dup = 0
               AND (lower(n.id) = lower(?1) OR lower(n.title) = lower(?1) OR n.aliases_json != '[]')",
        )
        .map_err(|e| format!("failed to prepare node text query: {e}"))?;
    let rows = stmt
        .query_map(params![trimmed], |r| row_to_roam_node(r, display_root))
        .map_err(|e| format!("failed to execute node text query: {e}"))?;

    let mut direct = Vec::new();
    for item in rows {
        let node = item.map_err(|e| format!("node row decode error: {e}"))?;
        let matched = node.id.eq_ignore_ascii_case(trimmed)
            || node.title.eq_ignore_ascii_case(trimmed)
            || node.aliases.iter().any(|a| a.eq_ignore_ascii_case(trimmed));
        if matched {
            direct.push(node);
        }
    }
    direct.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    if !direct.is_empty() || !allow_locator || !trimmed.contains("::") {
        return Ok(direct);
    }
    resolve_locator_node(conn, workspace_root, display_root, trimmed)
}

// endregion

// region Backlinks & Outgoing Links

/// Looks up the primary `(title, rel_path)` of a node `:ID:` ordered by `(display_file, line)`.
fn lookup_first_node_meta(
    conn: &Connection,
    display_root: &Path,
    node_id: &str,
) -> Result<Option<(String, String)>, String> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT n.title, n.rel_path, n.line
             FROM nodes n JOIN files f ON f.rel_path = n.rel_path
             WHERE n.roam_excluded = 0 AND f.is_roam_dup = 0 AND lower(n.id) = lower(?1)",
        )
        .map_err(|e| format!("failed to prepare node meta lookup: {e}"))?;
    let rows = stmt
        .query_map(params![node_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| format!("failed to query node meta: {e}"))?;
    let mut best: Option<(String, String, i64)> = None;
    for item in rows {
        let cur = item.map_err(|e| e.to_string())?;
        if best.as_ref().is_none_or(|b| {
            (display_file_path(display_root, &cur.1), cur.2)
                < (display_file_path(display_root, &b.1), b.2)
        }) {
            best = Some(cur);
        }
    }
    Ok(best.map(|(t, rp, _)| (t, rp)))
}

/// Queries incoming [`RoamBacklink`] edges for `targets` via `idx_links_target_lower`.
pub(crate) fn query_backlinks_cached(
    conn: &Connection,
    display_root: &Path,
    targets: &[RoamNode],
) -> Result<Vec<RoamBacklink>, String> {
    let mut unique_ids = BTreeSet::new();
    for t in targets {
        unique_ids.insert(t.id.to_ascii_lowercase());
    }
    let mut out = Vec::new();
    let mut stmt = conn
        .prepare_cached(
            "SELECT l.rel_path, l.source_id, l.target_id, l.description, l.line, l.context
             FROM links l JOIN files f ON f.rel_path = l.rel_path
             WHERE f.is_roam_dup = 0 AND lower(l.target_id) = ?1",
        )
        .map_err(|e| format!("failed to prepare backlinks query: {e}"))?;

    for lower_id in unique_ids {
        let rows = stmt
            .query_map(params![lower_id], |r| row_to_roam_link(r, display_root))
            .map_err(|e| format!("failed to execute backlinks query: {e}"))?;
        for item in rows {
            let link = item.map_err(|e| e.to_string())?;
            let source_title = match link.source_id.as_deref() {
                Some(sid) => lookup_first_node_meta(conn, display_root, sid)?.map(|(t, _)| t),
                None => None,
            };
            out.push(RoamBacklink {
                source_id: link.source_id,
                source_title,
                source_file: link.source_file,
                target_id: link.target_id,
                line: link.line,
                description: link.description,
                context: link.context,
            });
        }
    }
    out.sort_by(|a, b| (&a.source_file, a.line).cmp(&(&b.source_file, b.line)));
    Ok(out)
}

/// Queries outgoing [`RoamOutgoingLink`] edges for `sources` via `idx_links_source_lower`.
pub(crate) fn query_outgoing_links_cached(
    conn: &Connection,
    display_root: &Path,
    sources: &[RoamNode],
) -> Result<Vec<RoamOutgoingLink>, String> {
    let mut unique_ids = BTreeSet::new();
    for s in sources {
        unique_ids.insert(s.id.to_ascii_lowercase());
    }
    let mut out = Vec::new();
    let mut stmt = conn
        .prepare_cached(
            "SELECT l.rel_path, l.source_id, l.target_id, l.description, l.line, l.context
             FROM links l JOIN files f ON f.rel_path = l.rel_path
             WHERE f.is_roam_dup = 0 AND lower(l.source_id) = ?1",
        )
        .map_err(|e| format!("failed to prepare outgoing links query: {e}"))?;

    for lower_id in unique_ids {
        let rows = stmt
            .query_map(params![lower_id], |r| row_to_roam_link(r, display_root))
            .map_err(|e| format!("failed to execute outgoing links query: {e}"))?;
        for item in rows {
            let link = item.map_err(|e| e.to_string())?;
            let Some(source_id) = link.source_id else {
                continue;
            };
            let target_meta = lookup_first_node_meta(conn, display_root, &link.target_id)?;
            out.push(RoamOutgoingLink {
                source_id,
                source_file: link.source_file,
                target_id: link.target_id,
                target_title: target_meta.as_ref().map(|(t, _)| t.clone()),
                target_file: target_meta
                    .as_ref()
                    .map(|(_, rp)| display_file_path(display_root, rp)),
                resolved: target_meta.is_some(),
                line: link.line,
                description: link.description,
                context: link.context,
            });
        }
    }
    out.sort_by(|a, b| (&a.source_file, a.line).cmp(&(&b.source_file, b.line)));
    Ok(out)
}

// endregion

// region Graph & Edna Lookups

/// Loads all [`RoamNode`] and [`RoamLink`] rows into a [`RoamIndex`] without parsing `.org` files.
pub(crate) fn load_graph_cached(
    conn: &Connection,
    display_root: &Path,
    files_scanned: usize,
) -> Result<RoamIndex, String> {
    let mut n_stmt = conn
        .prepare_cached(
            "SELECT n.id, n.rel_path, n.id_seed, n.title, n.line, n.level, n.todo_keyword, n.priority,
                    n.tags_json, n.olp_json, n.aliases_json, n.refs_json
             FROM nodes n JOIN files f ON f.rel_path = n.rel_path
             WHERE n.roam_excluded = 0 AND f.is_roam_dup = 0",
        )
        .map_err(|e| format!("failed to prepare graph nodes query: {e}"))?;
    let n_rows = n_stmt
        .query_map([], |r| row_to_roam_node(r, display_root))
        .map_err(|e| format!("failed to scan graph nodes: {e}"))?;
    let mut nodes = Vec::new();
    for item in n_rows {
        nodes.push(item.map_err(|e| e.to_string())?);
    }
    nodes.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    let mut l_stmt = conn
        .prepare_cached(
            "SELECT l.rel_path, l.source_id, l.target_id, l.description, l.line, l.context
             FROM links l JOIN files f ON f.rel_path = l.rel_path
             WHERE f.is_roam_dup = 0",
        )
        .map_err(|e| format!("failed to prepare graph links query: {e}"))?;
    let l_rows = l_stmt
        .query_map([], |r| row_to_roam_link(r, display_root))
        .map_err(|e| format!("failed to scan graph links: {e}"))?;
    let mut links = Vec::new();
    for item in l_rows {
        links.push(item.map_err(|e| e.to_string())?);
    }
    links.sort_by(|a, b| (&a.source_file, a.line).cmp(&(&b.source_file, b.line)));

    Ok(RoamIndex {
        nodes,
        links,
        docs: Vec::new(),
        files_scanned,
    })
}

/// Resolves relative file paths containing `ids(...)` `:BLOCKER:` targets referenced by `rel_paths`.
pub(crate) fn query_blocker_target_paths(
    conn: &Connection,
    rel_paths: &[String],
) -> Result<Vec<String>, String> {
    if rel_paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare_cached(
            "SELECT DISTINCT n.rel_path
             FROM edna_edges e
             JOIN nodes n ON lower(n.id) = lower(e.target_id)
             WHERE e.rel_path = ?1 AND e.kind = 'blocker'",
        )
        .map_err(|e| format!("failed to prepare edna blocker join: {e}"))?;
    let mut set = BTreeSet::new();
    for rel in rel_paths {
        let rows = stmt
            .query_map(params![rel], |r| r.get::<_, String>(0))
            .map_err(|e| format!("failed to execute edna blocker join: {e}"))?;
        for item in rows {
            set.insert(item.map_err(|e| e.to_string())?);
        }
    }
    Ok(set.into_iter().collect())
}

// endregion
