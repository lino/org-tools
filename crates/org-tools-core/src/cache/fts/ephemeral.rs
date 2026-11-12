// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ephemeral in-memory SQLite FTS5 search across parsed Org documents.

use std::collections::HashMap;

use rusqlite::{params, Connection, Statement};

use super::BodyLoadedDoc;
use crate::document::{OrgDocument, OrgEntry};
use crate::locator::FILE_NODE_ENTRY_INDEX;

// region Types

/// An ephemeral search match with BM25 score and highlighted snippet.
#[derive(Debug, Clone)]
pub struct EphemeralFtsHit {
    /// Document index in the passed `docs` slice.
    pub doc_idx: usize,
    /// Entry index in `doc.entries` or [`FILE_NODE_ENTRY_INDEX`].
    pub entry_idx: usize,
    /// BM25 relevance score.
    pub rank: f64,
    /// Highlighted snippet text.
    pub snippet: String,
}

// endregion

// region Indexing & Query Helpers

/// Sanitizes whitespace tokens into a prefix-matching FTS5 OR expression.
fn build_fts5_query_expr(query_str: &str) -> Option<String> {
    let terms: Vec<String> = query_str
        .split_whitespace()
        .map(|t| {
            let clean = t.replace('"', "\"\"");
            format!("\"{clean}\"*")
        })
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

/// Initializes the ephemeral in-memory FTS5 virtual table.
fn init_ephemeral_fts_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE entries_fts USING fts5(
            doc_idx UNINDEXED, entry_idx UNINDEXED, title, aliases, tags, properties, vulpea_meta, body,
            tokenize = 'unicode61 remove_diacritics 0'
        );",
    )
    .map_err(|e| e.to_string())
}

/// Inserts a level-0 file node into the ephemeral FTS5 index.
fn insert_file_node_fts(
    ins: &mut Statement<'_>,
    doc_idx: usize,
    doc: &OrgDocument,
    lines: &[&str],
) -> Result<(), String> {
    if doc.file_properties.is_empty() && doc.file_id().is_none() {
        return Ok(());
    }
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
    let end = doc
        .entries
        .first()
        .map(|e| e.heading_line.saturating_sub(1))
        .unwrap_or(lines.len());
    let body = super::extract_slice_body(lines, 0, end);
    ins.execute(params![
        doc_idx as i64,
        FILE_NODE_ENTRY_INDEX as i64,
        doc.file_title_or_stem(),
        aliases,
        tags,
        props,
        meta,
        body
    ])
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Inserts an entry into the ephemeral FTS5 index.
fn insert_entry_fts(
    ins: &mut Statement<'_>,
    doc_idx: usize,
    entry_idx: usize,
    entry: &OrgEntry,
    doc: &OrgDocument,
    lines: &[&str],
) -> Result<(), String> {
    let aliases = entry
        .properties
        .get("ROAM_ALIASES")
        .cloned()
        .unwrap_or_default();
    let tags = doc.effective_tags(entry_idx).join(" ");
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
    let start = entry.heading_line.min(lines.len());
    let end = entry.content_end_line.saturating_sub(1).min(lines.len());
    let body = if start < end {
        super::extract_slice_body(lines, start, end)
    } else {
        String::new()
    };
    ins.execute(params![
        doc_idx as i64,
        entry_idx as i64,
        entry.title,
        aliases,
        tags,
        props,
        meta,
        body
    ])
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Queries ranked FTS5 results from the in-memory SQLite table.
fn query_ephemeral_ranked(
    conn: &Connection,
    match_expr: &str,
    limit: Option<usize>,
) -> Result<Vec<EphemeralFtsHit>, String> {
    let sql = format!(
        "SELECT doc_idx, entry_idx, bm25(entries_fts) as rank, snippet(entries_fts, -1, '<b>', '</b>', '...', 10) as snip
         FROM entries_fts
         WHERE entries_fts MATCH ?1
         ORDER BY rank ASC{}",
        limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default()
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![match_expr], |row| {
            let raw_rank: f64 = row.get(2)?;
            Ok(EphemeralFtsHit {
                doc_idx: row.get::<_, i64>(0)? as usize,
                entry_idx: row.get::<_, i64>(1)? as usize,
                rank: -raw_rank,
                snippet: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut results = Vec::new();
    for r in rows {
        results.push(r.map_err(|e| e.to_string())?);
    }
    Ok(results)
}

// endregion

// region Public Entrypoint

/// Executes an in-memory SQLite FTS5 search across documents with BM25 ranking.
pub fn execute_ephemeral_fts5_search(
    query_str: &str,
    docs: &[OrgDocument],
    limit: Option<usize>,
) -> Result<Vec<EphemeralFtsHit>, String> {
    let Some(match_expr) = build_fts5_query_expr(query_str) else {
        return Ok(Vec::new());
    };

    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    init_ephemeral_fts_table(&conn)?;

    let mut ins = conn
        .prepare(
            "INSERT INTO entries_fts (doc_idx, entry_idx, title, aliases, tags, properties, vulpea_meta, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .map_err(|e| e.to_string())?;

    for (di, doc) in docs.iter().enumerate() {
        let content = std::fs::read_to_string(&doc.file).unwrap_or_default();
        let lines: Vec<&str> = content.lines().collect();
        insert_file_node_fts(&mut ins, di, doc, &lines)?;
        for (ei, entry) in doc.entries.iter().enumerate() {
            insert_entry_fts(&mut ins, di, ei, entry, doc, &lines)?;
        }
    }
    drop(ins);

    query_ephemeral_ranked(&conn, &match_expr, limit)
}

// endregion

// region In-Memory BM25 Scoring & Ranking

/// Populates an ephemeral in-memory `entries_fts` table from `loaded` documents.
fn populate_ephemeral_fts5(conn: &Connection, loaded: &[BodyLoadedDoc]) -> Result<(), String> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE entries_fts USING fts5(
            doc_idx UNINDEXED, entry_idx UNINDEXED, title, body,
            tokenize = 'unicode61 remove_diacritics 0'
        );",
    )
    .map_err(|e| e.to_string())?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    {
        let mut ins = tx
            .prepare(
                "INSERT INTO entries_fts (doc_idx, entry_idx, title, body) VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(|e| e.to_string())?;
        for (di, item) in loaded.iter().enumerate() {
            for (ei, entry) in item.doc.entries.iter().enumerate() {
                ins.execute(params![di as i64, ei as i64, entry.title, item.bodies[ei]])
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Computes exact SQLite C `bm25(entries_fts)` scores on an ephemeral in-memory FTS5 table.
fn score_in_memory_fts5_bm25(
    loaded: &[BodyLoadedDoc],
    or_match_expr: &str,
) -> HashMap<(usize, usize), f64> {
    let mut scores = HashMap::new();
    let Ok(conn) = Connection::open_in_memory() else {
        return scores;
    };
    if populate_ephemeral_fts5(&conn, loaded).is_err() {
        return scores;
    }
    let sql = "SELECT doc_idx, entry_idx, bm25(entries_fts)
               FROM entries_fts WHERE entries_fts MATCH ?1";
    if let Ok(mut stmt) = conn.prepare(sql) {
        if let Ok(rows) = stmt.query_map(params![or_match_expr], |r| {
            Ok((
                r.get::<_, i64>(0)? as usize,
                r.get::<_, i64>(1)? as usize,
                -r.get::<_, f64>(2)?,
            ))
        }) {
            for (di, ei, s) in rows.flatten() {
                scores.insert((di, ei), s);
            }
        }
    }
    scores
}

/// Ranks `(doc_idx, entry_idx)` pairs by SQLite FTS5 Okapi BM25 score and `(file, entry_idx)` tie-breaker.
pub fn rank_matched_indices_bm25(
    loaded: &[BodyLoadedDoc],
    matched: &[(usize, usize)],
    phrases: &[String],
) -> Vec<(usize, usize)> {
    if matched.len() <= 1 {
        return matched.to_vec();
    }
    let clauses: Vec<String> = phrases
        .iter()
        .filter_map(|p| super::format_fts5_phrase(p))
        .collect();
    let mut ranked = matched.to_vec();
    if clauses.is_empty() {
        ranked.sort_by(|&(da, ea), &(db, eb)| {
            (&loaded[da].doc.file, ea).cmp(&(&loaded[db].doc.file, eb))
        });
        return ranked;
    }
    let scores = score_in_memory_fts5_bm25(loaded, &clauses.join(" OR "));
    ranked.sort_by(|&(da, ea), &(db, eb)| {
        let sa = scores.get(&(da, ea)).copied().unwrap_or(0.0);
        let sb = scores.get(&(db, eb)).copied().unwrap_or(0.0);
        sb.total_cmp(&sa)
            .then_with(|| (&loaded[da].doc.file, ea).cmp(&(&loaded[db].doc.file, eb)))
    });
    ranked
}

// endregion
