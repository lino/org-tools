// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Org document record extraction and transactional SQLite row writers for `WorkspaceCache`.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{params, Connection, Transaction};

use crate::cache::fts::extract_entry_bodies;
use crate::cache::walk::{compute_canon_key, DiskOrgFile};
use crate::document::OrgDocument;
use crate::edna::extract_dependency_ids;
use crate::files::join_slash_rel;
use crate::roam::{
    extract_roam_links, extract_roam_nodes, is_roam_excluded, lookup_prop_ci, RoamLink, RoamNode,
};
use crate::source::SourceFile;

/// Extracted `org-edna` dependency row ready for `edna_edges` insertion.
pub(crate) struct EdnaEdgeRecord {
    /// Entry index (`-1` for level-0 file drawer, `>= 0` for headings).
    pub(crate) entry_idx: i64,
    /// Optional `:ID:` of the source node/entry.
    pub(crate) entry_id: Option<String>,
    /// One-based line number of the heading or file drawer.
    pub(crate) line: i64,
    /// Edge kind (`"blocker"` or `"trigger"`).
    pub(crate) kind: &'static str,
    /// Target `:ID:` referenced inside `ids(...)`.
    pub(crate) target_id: String,
    /// Raw property expression string.
    pub(crate) raw_expr: String,
}

/// Extracted FTS5 row ready for `entries_fts` insertion.
pub(crate) struct FtsEntryRecord {
    /// Zero-based entry index in `doc.entries` or -1 for file node.
    pub(crate) entry_idx: i64,
    /// One-based heading line number (or file properties line).
    pub(crate) heading_line: i64,
    /// Heading title or file title text.
    pub(crate) title: String,
    /// Comma/space-separated aliases.
    pub(crate) aliases: String,
    /// Space-separated tags.
    pub(crate) tags: String,
    /// Key=Value properties string.
    pub(crate) properties: String,
    /// Vulpea metadata key=value string.
    pub(crate) vulpea_meta: String,
    /// Clean body prose text.
    pub(crate) body: String,
}

/// Metadata-only update for a file whose content SHA-256 matched its prior digest.
pub(crate) struct FileTouch {
    /// Workspace-relative `/` path.
    pub(crate) rel_path: String,
    /// Updated `mtime_ns` timestamp.
    pub(crate) mtime_ns: i64,
    /// Updated byte length.
    pub(crate) byte_len: i64,
    /// Whether the entry is currently a symlink.
    pub(crate) is_symlink: bool,
    /// Canonical target path key.
    pub(crate) canon_key: String,
}

/// Extracted heading outline row ready for `headings` insertion.
pub(crate) struct HeadingOutlineRecord {
    /// Zero-based entry index in `doc.entries`.
    pub(crate) entry_idx: i64,
    /// One-based heading line number.
    pub(crate) heading_line: i64,
    /// Heading star level (`>= 1`).
    pub(crate) level: i64,
    /// Optional TODO keyword on the heading.
    pub(crate) todo_keyword: Option<String>,
    /// Clean heading title.
    pub(crate) title: String,
    /// Optional `:CUSTOM_ID:` value.
    pub(crate) custom_id: Option<String>,
    /// JSON array of heading tags.
    pub(crate) tags_json: String,
    /// JSON array of uppercase property drawer keys on the heading.
    pub(crate) prop_keys_json: String,
}

/// Parsed rows extracted from one `.org` file for transactional SQLite upsert.
pub(crate) struct ParsedFilePayload {
    /// File metadata key.
    pub(crate) disk: DiskOrgFile,
    /// Canonical target path key (`0` syscalls on regular files).
    pub(crate) canon_key: String,
    /// Lowercase hex SHA-256 content hash.
    pub(crate) content_hash: String,
    /// Whether an older row existed in `files` and must be replaced.
    pub(crate) existed: bool,
    /// JSON array of active file TODO keywords.
    pub(crate) todo_active_json: String,
    /// JSON array of done file TODO keywords.
    pub(crate) todo_done_json: String,
    /// JSON array of file-level `#+FILETAGS:`.
    pub(crate) filetags_json: String,
    /// JSON array of uppercase level-0 `:PROPERTIES:` keys.
    pub(crate) file_prop_keys_json: String,
    /// Non-excluded roam nodes in this file.
    pub(crate) nodes: Vec<RoamNode>,
    /// `:ROAM_EXCLUDE: t` `(id, title, line, level, todo_keyword)` tuples retained for `ids(...)` and completion.
    pub(crate) excluded_ids: Vec<(String, String, i64, i64, Option<String>)>,
    /// Heading outline and `:CUSTOM_ID:` rows in this file.
    pub(crate) headings: Vec<HeadingOutlineRecord>,
    /// Outgoing `[[id:...]]` links in this file.
    pub(crate) links: Vec<RoamLink>,
    /// `org-edna` `:BLOCKER:` and `:TRIGGER:` edges in this file.
    pub(crate) edna_edges: Vec<EdnaEdgeRecord>,
    /// Heading FTS5 rows in this file.
    pub(crate) fts_rows: Vec<FtsEntryRecord>,
}

/// Appends `blocker` and `trigger` edges from `raw_opt` into `out`.
fn push_edna_prop_edges(
    raw_opt: Option<&str>,
    entry_idx: i64,
    entry_id: &Option<String>,
    line: i64,
    kind: &'static str,
    out: &mut Vec<EdnaEdgeRecord>,
) {
    let Some(raw) = raw_opt.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    for dep_id in extract_dependency_ids(raw) {
        let clean = dep_id.strip_prefix("id:").unwrap_or(dep_id.as_str()).trim();
        if !clean.is_empty() {
            out.push(EdnaEdgeRecord {
                entry_idx,
                entry_id: entry_id.clone(),
                line,
                kind,
                target_id: clean.to_string(),
                raw_expr: raw.to_string(),
            });
        }
    }
}

/// Extracts `:ROAM_EXCLUDE: t` `:ID:` tuples `(id, title, line, level, todo_keyword)` from `doc`.
fn extract_excluded_ids(doc: &OrgDocument) -> Vec<(String, String, i64, i64, Option<String>)> {
    let mut out = Vec::new();
    if is_roam_excluded(&doc.file_properties) {
        if let Some(id) = lookup_prop_ci(&doc.file_properties, "ID")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            out.push((id.to_string(), doc.file_title_or_stem(), 1, 0, None));
        }
    }
    for e in &doc.entries {
        if is_roam_excluded(&e.properties) {
            if let Some(id) = lookup_prop_ci(&e.properties, "ID")
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                out.push((
                    id.to_string(),
                    e.title.clone(),
                    e.heading_line as i64,
                    e.level as i64,
                    e.keyword.clone(),
                ));
            }
        }
    }
    out
}

/// Extracts [`HeadingOutlineRecord`] rows for all headings in `doc`.
fn extract_heading_outlines(doc: &OrgDocument) -> Vec<HeadingOutlineRecord> {
    doc.entries
        .iter()
        .enumerate()
        .map(|(idx, e)| {
            let custom_id = lookup_prop_ci(&e.properties, "CUSTOM_ID")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let prop_keys: Vec<String> = e
                .properties
                .keys()
                .map(|k| k.to_ascii_uppercase())
                .collect();
            HeadingOutlineRecord {
                entry_idx: idx as i64,
                heading_line: e.heading_line as i64,
                level: e.level as i64,
                todo_keyword: e.keyword.clone(),
                title: e.title.clone(),
                custom_id,
                tags_json: json_slice_or_empty(&e.tags).into_owned(),
                prop_keys_json: json_slice_or_empty(&prop_keys).into_owned(),
            }
        })
        .collect()
}

/// Extracts all level-0 and heading `edna_edges` records from `doc`.
fn extract_doc_edna_edges(doc: &OrgDocument) -> Vec<EdnaEdgeRecord> {
    let mut edges = Vec::new();
    let file_id = lookup_prop_ci(&doc.file_properties, "ID")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let fl = doc.file_properties_line.unwrap_or(1) as i64;
    for (prop, kind) in [("BLOCKER", "blocker"), ("TRIGGER", "trigger")] {
        let raw = lookup_prop_ci(&doc.file_properties, prop)
            .or_else(|| lookup_prop_ci(&doc.default_properties, prop));
        push_edna_prop_edges(raw, -1, &file_id, fl, kind, &mut edges);
    }
    for (idx, entry) in doc.entries.iter().enumerate() {
        let eid = lookup_prop_ci(&entry.properties, "ID")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        for (prop, kind) in [("BLOCKER", "blocker"), ("TRIGGER", "trigger")] {
            push_edna_prop_edges(
                doc.property(idx, prop),
                idx as i64,
                &eid,
                entry.heading_line as i64,
                kind,
                &mut edges,
            );
        }
    }
    edges
}

/// Extracts all FTS5 index records (level-0 file node if present, plus all heading entries).
fn extract_fts_rows(
    source: &SourceFile,
    doc: &OrgDocument,
    bodies: &[String],
) -> Vec<FtsEntryRecord> {
    let mut rows = Vec::with_capacity(doc.entries.len() + 1);
    if !doc.file_properties.is_empty() || doc.file_id().is_some() {
        let lines: Vec<&str> = source.content.lines().collect();
        let end = doc
            .entries
            .first()
            .map(|e| e.heading_line.saturating_sub(1))
            .unwrap_or(lines.len());
        let file_body = crate::cache::fts::extract_slice_body(&lines, 0, end);
        let aliases = doc
            .file_properties
            .get("ROAM_ALIASES")
            .cloned()
            .unwrap_or_default();
        let tags = doc.filetags.join(" ");
        let props = doc
            .file_properties
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ");
        let meta = doc
            .vulpea_meta
            .iter()
            .flat_map(|(k, vs)| vs.iter().map(move |v| format!("{k}={v}")))
            .collect::<Vec<_>>()
            .join(" ");

        rows.push(FtsEntryRecord {
            entry_idx: -1,
            heading_line: doc.file_properties_line.unwrap_or(1) as i64,
            title: doc.file_title_or_stem(),
            aliases,
            tags,
            properties: props,
            vulpea_meta: meta,
            body: file_body,
        });
    }

    for (idx, entry) in doc.entries.iter().enumerate() {
        let aliases = entry
            .properties
            .get("ROAM_ALIASES")
            .cloned()
            .unwrap_or_default();
        let tags = doc.effective_tags(idx).join(" ");
        let props = entry
            .properties
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ");
        let meta = entry
            .vulpea_meta
            .iter()
            .flat_map(|(k, vs)| vs.iter().map(move |v| format!("{k}={v}")))
            .collect::<Vec<_>>()
            .join(" ");
        let body = if idx < bodies.len() {
            bodies[idx].clone()
        } else {
            String::new()
        };

        rows.push(FtsEntryRecord {
            entry_idx: idx as i64,
            heading_line: entry.heading_line as i64,
            title: entry.title.clone(),
            aliases,
            tags,
            properties: props,
            vulpea_meta: meta,
            body,
        });
    }
    rows
}

/// Parses raw file bytes into a [`ParsedFilePayload`] ready for SQLite insertion.
pub(crate) fn build_file_payload(
    workspace_root: &Path,
    disk: DiskOrgFile,
    bytes: Vec<u8>,
    content_hash: String,
    existed: bool,
) -> ParsedFilePayload {
    let content = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).into_owned());
    let source = SourceFile::new(join_slash_rel(workspace_root, &disk.rel_path), content);
    let doc = OrgDocument::from_source(&source);
    let nodes = extract_roam_nodes(&doc);
    let excluded_ids = extract_excluded_ids(&doc);
    let headings = extract_heading_outlines(&doc);
    let links = extract_roam_links(&source, &doc);
    let edna_edges = extract_doc_edna_edges(&doc);
    let bodies = extract_entry_bodies(&source.content, &doc);
    let fts_rows = extract_fts_rows(&source, &doc, &bodies);
    let file_props: Vec<String> = doc
        .file_properties
        .keys()
        .map(|k| k.to_ascii_uppercase())
        .collect();
    let canon_key = compute_canon_key(workspace_root, &disk);
    ParsedFilePayload {
        disk,
        canon_key,
        content_hash,
        existed,
        todo_active_json: json_slice_or_empty(&doc.todo_keywords.todo).into_owned(),
        todo_done_json: json_slice_or_empty(&doc.todo_keywords.done).into_owned(),
        filetags_json: json_slice_or_empty(&doc.filetags).into_owned(),
        file_prop_keys_json: json_slice_or_empty(&file_props).into_owned(),
        nodes,
        excluded_ids,
        headings,
        links,
        edna_edges,
        fts_rows,
    }
}

/// Serializes `items` as a JSON array string, borrowing `"[]"` without allocation when empty.
fn json_slice_or_empty(items: &[String]) -> std::borrow::Cow<'static, str> {
    if items.is_empty() {
        std::borrow::Cow::Borrowed("[]")
    } else {
        std::borrow::Cow::Owned(serde_json::to_string(items).unwrap_or_else(|_| "[]".into()))
    }
}

/// Inserts [`RoamNode`] and excluded `:ID:` rows for `rel` inside `tx`.
fn insert_nodes_in_tx(
    tx: &Transaction<'_>,
    rel: &str,
    p: &ParsedFilePayload,
) -> Result<(), String> {
    if p.nodes.is_empty() && p.excluded_ids.is_empty() {
        return Ok(());
    }
    let sql = "INSERT INTO nodes (id, rel_path, id_seed, title, line, level, todo_keyword, priority, tags_json, olp_json, aliases_json, refs_json, roam_excluded)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)";
    let mut stmt = tx
        .prepare_cached(sql)
        .map_err(|e| format!("nodes prepare failed: {e}"))?;
    for n in &p.nodes {
        let (t, o, a, r) = (
            json_slice_or_empty(&n.tags),
            json_slice_or_empty(&n.olp),
            json_slice_or_empty(&n.aliases),
            json_slice_or_empty(&n.refs),
        );
        stmt.execute(params![
            n.id,
            rel,
            n.id_seed,
            n.title,
            n.line as i64,
            n.level as i64,
            n.todo_keyword,
            n.priority,
            t.as_ref(),
            o.as_ref(),
            a.as_ref(),
            r.as_ref(),
            0i64
        ])
        .map_err(|e| format!("nodes insert failed for {rel}: {e}"))?;
    }
    let none = Option::<String>::None;
    for (id, title, line, level, kw) in &p.excluded_ids {
        stmt.execute(params![
            id, rel, &none, title, line, level, kw, &none, "[]", "[]", "[]", "[]", 1i64
        ])
        .map_err(|e| format!("excluded node insert failed for {rel}: {e}"))?;
    }
    Ok(())
}

/// Inserts [`HeadingOutlineRecord`] rows for `rel` inside `tx`.
fn insert_headings_in_tx(
    tx: &Transaction<'_>,
    rel: &str,
    headings: &[HeadingOutlineRecord],
) -> Result<(), String> {
    if headings.is_empty() {
        return Ok(());
    }
    let sql = "INSERT INTO headings (rel_path, entry_idx, heading_line, level, todo_keyword, title, custom_id, tags_json, prop_keys_json)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";
    let mut s = tx
        .prepare_cached(sql)
        .map_err(|e| format!("headings prepare failed: {e}"))?;
    for h in headings {
        s.execute(params![
            rel,
            h.entry_idx,
            h.heading_line,
            h.level,
            h.todo_keyword,
            h.title,
            h.custom_id,
            h.tags_json,
            h.prop_keys_json
        ])
        .map_err(|e| format!("headings insert failed for {rel}: {e}"))?;
    }
    Ok(())
}

/// Inserts a single [`ParsedFilePayload`] across all workspace cache tables inside `tx`.
fn write_payload_in_tx(tx: &Transaction<'_>, p: &ParsedFilePayload) -> Result<(), String> {
    let rel = p.disk.rel_path.as_str();
    if p.existed {
        tx.prepare_cached("DELETE FROM entries_fts WHERE rel_path = ?1")
            .and_then(|mut s| s.execute(params![rel]))
            .map_err(|e| format!("FTS5 delete failed for {rel}: {e}"))?;
        tx.prepare_cached("DELETE FROM files WHERE rel_path = ?1")
            .and_then(|mut s| s.execute(params![rel]))
            .map_err(|e| format!("files delete failed for {rel}: {e}"))?;
    }
    tx.prepare_cached(
        "INSERT INTO files (rel_path, mtime_ns, byte_len, content_hash, is_symlink, canon_key, is_roam_dup, todo_active_json, todo_done_json, filetags_json, file_prop_keys_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, ?9, ?10)",
    )
    .and_then(|mut s| {
        s.execute(params![
            rel, p.disk.mtime_ns, p.disk.byte_len, p.content_hash, i64::from(p.disk.is_symlink),
            p.canon_key, p.todo_active_json, p.todo_done_json, p.filetags_json, p.file_prop_keys_json
        ])
    })
    .map_err(|e| format!("files insert failed for {rel}: {e}"))?;

    insert_nodes_in_tx(tx, rel, p)?;
    insert_headings_in_tx(tx, rel, &p.headings)?;
    if !p.links.is_empty() {
        let mut s = tx
            .prepare_cached("INSERT INTO links (rel_path, source_id, target_id, description, line, context) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
            .map_err(|e| format!("links prepare failed: {e}"))?;
        for l in &p.links {
            s.execute(params![
                rel,
                l.source_id,
                l.target_id,
                l.description,
                l.line as i64,
                l.context
            ])
            .map_err(|e| format!("links insert failed for {rel}: {e}"))?;
        }
    }
    if !p.edna_edges.is_empty() {
        let mut s = tx
            .prepare_cached("INSERT INTO edna_edges (rel_path, entry_idx, entry_id, line, kind, target_id, raw_expr) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
            .map_err(|e| format!("edna_edges prepare failed: {e}"))?;
        for e in &p.edna_edges {
            s.execute(params![
                rel,
                e.entry_idx,
                e.entry_id,
                e.line,
                e.kind,
                e.target_id,
                e.raw_expr
            ])
            .map_err(|e| format!("edna_edges insert failed for {rel}: {e}"))?;
        }
    }
    if !p.fts_rows.is_empty() {
        let mut s = tx
            .prepare_cached(
                "INSERT INTO entries_fts (rel_path, entry_idx, heading_line, title, aliases, tags, properties, vulpea_meta, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .map_err(|e| format!("entries_fts prepare failed: {e}"))?;
        for f in &p.fts_rows {
            s.execute(params![
                rel,
                f.entry_idx,
                f.heading_line,
                f.title,
                f.aliases,
                f.tags,
                f.properties,
                f.vulpea_meta,
                f.body
            ])
            .map_err(|e| format!("entries_fts insert failed for {rel}: {e}"))?;
        }
    }
    Ok(())
}

/// Recomputes `files.is_roam_dup` in `Path::cmp` order whenever symlinks or prior duplicates exist.
fn recompute_roam_dups_in_tx(tx: &Transaction<'_>) -> Result<(), String> {
    let has_sym_or_dup: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM files WHERE is_symlink = 1)
                 OR EXISTS(SELECT 1 FROM files WHERE is_roam_dup = 1)",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("failed to probe symlink duplicates: {e}"))?;
    if !has_sym_or_dup {
        return Ok(());
    }
    let mut rows: Vec<(String, String, i64)> = Vec::new();
    {
        let mut stmt = tx
            .prepare_cached("SELECT rel_path, canon_key, is_roam_dup FROM files")
            .map_err(|e| format!("failed to prepare files dup scan: {e}"))?;
        let mapped = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| format!("failed to query files dup scan: {e}"))?;
        for item in mapped {
            rows.push(item.map_err(|e| format!("files dup decode error: {e}"))?);
        }
    }
    rows.sort_by(|a, b| Path::new(&a.0).cmp(Path::new(&b.0)));
    let mut seen = HashSet::with_capacity(rows.len());
    let mut upd = tx
        .prepare_cached("UPDATE files SET is_roam_dup = ?2 WHERE rel_path = ?1")
        .map_err(|e| format!("failed to prepare is_roam_dup update: {e}"))?;
    for (rel, canon, old_dup) in rows {
        let new_dup = i64::from(!seen.insert(canon));
        if new_dup != old_dup {
            upd.execute(params![rel, new_dup])
                .map_err(|e| format!("failed to update is_roam_dup for {rel}: {e}"))?;
        }
    }
    Ok(())
}

/// Applies deletions, metadata touches, and full file re-indexes inside a single SQLite transaction.
pub(crate) fn commit_batch(
    conn: &mut Connection,
    deleted: &[String],
    touches: &[FileTouch],
    upserts: &[ParsedFilePayload],
) -> Result<(), String> {
    let pure_insert =
        deleted.is_empty() && touches.is_empty() && upserts.iter().all(|p| !p.existed);
    if pure_insert {
        let _ = conn.execute_batch("PRAGMA foreign_keys = OFF; PRAGMA synchronous = OFF;");
    }
    let res = (|| -> Result<(), String> {
        let tx = conn
            .transaction()
            .map_err(|e| format!("failed to begin SQLite write transaction: {e}"))?;
        for rel in deleted {
            tx.prepare_cached("DELETE FROM entries_fts WHERE rel_path = ?1")
                .and_then(|mut s| s.execute(params![rel]))
                .map_err(|e| format!("FTS5 delete failed for {rel}: {e}"))?;
            tx.prepare_cached("DELETE FROM files WHERE rel_path = ?1")
                .and_then(|mut s| s.execute(params![rel]))
                .map_err(|e| format!("files delete failed for {rel}: {e}"))?;
        }
        for t in touches {
            tx.prepare_cached("UPDATE files SET mtime_ns = ?2, byte_len = ?3, is_symlink = ?4, canon_key = ?5 WHERE rel_path = ?1")
                .and_then(|mut s| s.execute(params![t.rel_path, t.mtime_ns, t.byte_len, i64::from(t.is_symlink), t.canon_key]))
                .map_err(|e| format!("metadata touch failed for {}: {e}", t.rel_path))?;
        }
        for payload in upserts {
            write_payload_in_tx(&tx, payload)?;
        }
        recompute_roam_dups_in_tx(&tx)?;
        tx.commit()
            .map_err(|e| format!("failed to commit workspace cache transaction: {e}"))
    })();
    if pure_insert {
        let _ = conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;");
    }
    res
}
