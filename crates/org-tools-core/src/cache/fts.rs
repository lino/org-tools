// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Entry body extraction, `unicode61` phrase matching, FTS5 query builder, and BM25 scorer.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::cache::{FtsHit, WorkspaceCache};
use crate::document::OrgDocument;
use crate::files::join_slash_rel;
use crate::roam::lookup_prop_ci;
use crate::source::SourceFile;

// region Constants & Types

thread_local! {
    /// Per-thread in-memory SQLite handle hosting `temp.q_fts` and `temp.q_vocab` for `unicode61`.
    static VOCAB_CONN: RefCell<Option<Connection>> = const { RefCell::new(None) };
}

/// DDL initializing the ephemeral `fts5vocab` `'instance'` tokenizer table.
const VOCAB_INIT_SQL: &str = "
    CREATE VIRTUAL TABLE temp.q_fts USING fts5(x, tokenize = 'unicode61 remove_diacritics 0');
    CREATE VIRTUAL TABLE temp.q_vocab USING fts5vocab(temp, 'q_fts', 'instance');
";

/// A parsed [`OrgDocument`] paired with pre-extracted clean body prose for every entry.
pub struct BodyLoadedDoc {
    /// Parsed Org document structure.
    pub doc: OrgDocument,
    /// Clean body prose indexed parallel to `doc.entries`.
    pub bodies: Vec<String>,
}

// endregion

// region Entry Body & Unicode61 Helpers

/// Returns `true` when `trimmed` starts with an Org planning keyword.
fn is_planning_line(trimmed: &str) -> bool {
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

/// Advances past an optional `:PROPERTIES: ... :END:` drawer closed before `end`.
fn skip_property_drawer(lines: &[&str], start: usize, end: usize) -> usize {
    if start >= end || !lines[start].trim().eq_ignore_ascii_case(":PROPERTIES:") {
        return start;
    }
    let mut idx = start + 1;
    while idx < end {
        if lines[idx].trim().eq_ignore_ascii_case(":END:") {
            return idx + 1;
        }
        idx += 1;
    }
    start
}

/// Advances past a `:LOGBOOK: ... :END:` drawer or `#+begin_comment ... #+end_comment` block.
fn skip_excluded_span(lines: &[&str], idx: usize, end: usize, close_marker: &str) -> usize {
    let mut cur = idx + 1;
    while cur < end {
        if lines[cur].trim().eq_ignore_ascii_case(close_marker) {
            return cur + 1;
        }
        cur += 1;
    }
    end
}

/// Returns `true` when `trimmed` is a `#+begin_...` or `#+end_...` block delimiter line.
fn is_block_delimiter(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("#+begin_") || lower.starts_with("#+end_")
}

/// Extracts clean body prose for a single heading slice `lines[start..end]`.
fn extract_slice_body(lines: &[&str], mut idx: usize, end: usize) -> String {
    if idx < end && is_planning_line(lines[idx].trim()) {
        idx += 1;
    }
    idx = skip_property_drawer(lines, idx, end);

    let mut kept = Vec::new();
    while idx < end {
        let raw = lines[idx].strip_suffix('\r').unwrap_or(lines[idx]);
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case(":LOGBOOK:") {
            idx = skip_excluded_span(lines, idx, end, ":END:");
            continue;
        }
        if trimmed
            .get(..15)
            .is_some_and(|p| p.eq_ignore_ascii_case("#+begin_comment"))
        {
            idx = skip_excluded_span(lines, idx, end, "#+end_comment");
            continue;
        }
        if !is_block_delimiter(trimmed) && !trimmed.starts_with("CLOCK:") {
            kept.push(raw);
        }
        idx += 1;
    }
    kept.join("\n")
}

/// Extracts clean body text for every heading in `doc` using a single split of `content`.
pub fn extract_entry_bodies(content: &str, doc: &OrgDocument) -> Vec<String> {
    if doc.entries.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = content.split('\n').collect();
    doc.entries
        .iter()
        .map(|entry| {
            let start = entry.heading_line.min(lines.len());
            let end = entry.content_end_line.saturating_sub(1).min(lines.len());
            extract_slice_body(&lines, start, end)
        })
        .collect()
}

/// Tokenizes `text` via SQLite's built-in `fts5vocab` `'instance'` table over `unicode61 remove_diacritics 0`.
fn tokenize_via_fts5vocab(text: &str) -> Vec<String> {
    VOCAB_CONN.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let conn = Connection::open_in_memory().expect("open in-memory fts5vocab SQLite");
            conn.execute_batch(VOCAB_INIT_SQL)
                .expect("initialize temp.q_fts and temp.q_vocab");
            *slot = Some(conn);
        }
        let conn = slot.as_ref().expect("vocab connection initialized");
        conn.execute("DELETE FROM temp.q_fts WHERE rowid = 1", [])
            .expect("clear temp.q_fts row");
        conn.execute(
            "INSERT INTO temp.q_fts(rowid, x) VALUES (1, ?1)",
            params![text],
        )
        .expect("insert text into temp.q_fts");
        let mut stmt = conn
            .prepare_cached("SELECT term FROM temp.q_vocab ORDER BY doc ASC, col ASC, offset ASC")
            .expect("prepare temp.q_vocab scan");
        stmt.query_map([], |r| r.get::<_, String>(0))
            .expect("query temp.q_vocab terms")
            .filter_map(Result::ok)
            .collect()
    })
}

/// Tokenizes `text` into exact SQLite FTS5 `unicode61 remove_diacritics 0` tokens.
pub fn tokenize_unicode61(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    if text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b' ') {
        return text
            .split_ascii_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
    }
    tokenize_via_fts5vocab(text)
}

/// Counts contiguous `phrase` occurrences inside `haystack`.
pub fn count_phrase_occurrences(haystack: &[String], phrase: &[String]) -> usize {
    if phrase.is_empty() || haystack.len() < phrase.len() {
        return 0;
    }
    haystack
        .windows(phrase.len())
        .filter(|w| *w == phrase)
        .count()
}

/// Returns `true` when `title` or `body` contains `needle` as a `unicode61` token/phrase.
pub fn matches_body_phrase(title: &str, body: &str, needle: &str) -> bool {
    let phrase = tokenize_unicode61(needle);
    if phrase.is_empty() {
        return false;
    }
    count_phrase_occurrences(&tokenize_unicode61(title), &phrase) > 0
        || count_phrase_occurrences(&tokenize_unicode61(body), &phrase) > 0
}

// endregion

// region FTS5 Query & BM25 Scorer

/// Formats non-empty `unicode61` tokens from `raw` as a double-quoted FTS5 phrase clause.
fn format_fts5_phrase(raw: &str) -> Option<String> {
    let tokens = tokenize_unicode61(raw);
    if tokens.is_empty() {
        return None;
    }
    let escaped: Vec<String> = tokens.into_iter().map(|t| t.replace('"', "\"\"")).collect();
    Some(format!("\"{}\"", escaped.join(" ")))
}

/// Sanitizes user `phrases` into a safe FTS5 `MATCH` expression free of operator injection.
pub fn build_fts5_match_expr(phrases: &[String]) -> Option<String> {
    if phrases.is_empty() {
        return None;
    }
    let mut clauses = Vec::with_capacity(phrases.len());
    for raw in phrases {
        clauses.push(format_fts5_phrase(raw)?);
    }
    Some(clauses.join(" AND "))
}

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

/// Runs an indexed SQLite FTS5 BM25 query for `phrases` ordered best-match-first.
pub(crate) fn query_fts5_bm25(
    conn: &Connection,
    phrases: &[String],
) -> Result<Vec<FtsHit>, String> {
    let Some(match_expr) = build_fts5_match_expr(phrases) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare_cached(
            "SELECT rel_path, entry_idx, heading_line, bm25(entries_fts)
             FROM entries_fts
             WHERE entries_fts MATCH ?1
             ORDER BY bm25(entries_fts) ASC, rel_path ASC, entry_idx ASC",
        )
        .map_err(|e| format!("failed to prepare FTS5 BM25 query: {e}"))?;

    let rows = stmt
        .query_map(params![match_expr], |row| {
            let raw_bm25: f64 = row.get(3)?;
            Ok(FtsHit {
                rel_path: row.get(0)?,
                entry_idx: row.get::<_, i64>(1)? as usize,
                heading_line: row.get::<_, i64>(2)? as usize,
                score: -raw_bm25,
            })
        })
        .map_err(|e| format!("failed to execute FTS5 BM25 query: {e}"))?;

    let mut out = Vec::new();
    for item in rows {
        out.push(item.map_err(|e| format!("failed to decode FTS5 row: {e}"))?);
    }
    Ok(out)
}

// endregion

// region Candidate Loading & Ranking

/// Parses `.org` `files` into [`BodyLoadedDoc`] items preserving input order.
pub fn load_docs_with_bodies(files: &[PathBuf]) -> Vec<BodyLoadedDoc> {
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        if let Ok(source) = SourceFile::from_path(file) {
            let doc = OrgDocument::from_source(&source);
            let bodies = extract_entry_bodies(&source.content, &doc);
            out.push(BodyLoadedDoc { doc, bodies });
        }
    }
    out
}

/// Returns `true` when `doc` carries a `:BLOCKER:` using non-ID cross-file finders (`match(`, `file(`, or `olp(`).
fn doc_has_dynamic_cross_file_blocker(doc: &OrgDocument) -> bool {
    let uses_dyn = |s: &str| {
        let lower = s.to_ascii_lowercase();
        lower.contains("match(") || lower.contains("file(") || lower.contains("olp(")
    };
    if lookup_prop_ci(&doc.file_properties, "BLOCKER").is_some_and(uses_dyn)
        || lookup_prop_ci(&doc.default_properties, "BLOCKER").is_some_and(uses_dyn)
    {
        return true;
    }
    (0..doc.entries.len())
        .filter_map(|idx| doc.property(idx, "BLOCKER"))
        .any(uses_dyn)
}

/// Candidate FTS5 BM25 hits paired with loaded `.org` documents (`None` when dynamic blockers require full scan).
pub type CachedFtsPlan = Option<(Vec<FtsHit>, Vec<BodyLoadedDoc>)>;

/// Loads only FTS5-matched `.org` files (plus `:BLOCKER:` `ids(...)` target files), or `None` on dynamic blockers.
pub fn load_cached_fts_candidate_docs(
    cache: &WorkspaceCache,
    display_root: &Path,
    phrases: &[String],
) -> Result<CachedFtsPlan, String> {
    let mut hits = cache.search_fts_bm25(phrases)?;
    if hits.is_empty() {
        return Ok(Some((Vec::new(), Vec::new())));
    }
    let hit_rels: Vec<String> = hits.iter().map(|h| h.rel_path.clone()).collect();
    let blocker_rels = cache.blocker_target_rel_paths(&hit_rels)?;
    let mut rel_set: BTreeSet<String> = hit_rels.into_iter().collect();
    rel_set.extend(blocker_rels);

    let mut loaded = Vec::with_capacity(rel_set.len());
    for rel in rel_set {
        let disk_path = join_slash_rel(cache.workspace_root(), &rel);
        let Ok(raw_src) = SourceFile::from_path(&disk_path) else {
            continue;
        };
        let display_src = SourceFile::new(join_slash_rel(display_root, &rel), raw_src.content);
        let doc = OrgDocument::from_source(&display_src);
        if doc_has_dynamic_cross_file_blocker(&doc) {
            return Ok(None);
        }
        let bodies = extract_entry_bodies(&display_src.content, &doc);
        loaded.push(BodyLoadedDoc { doc, bodies });
    }
    loaded.sort_by(|a, b| a.doc.file.cmp(&b.doc.file));
    hits.sort_by(|a, b| {
        b.score.total_cmp(&a.score).then_with(|| {
            (join_slash_rel(display_root, &a.rel_path), a.entry_idx)
                .cmp(&(join_slash_rel(display_root, &b.rel_path), b.entry_idx))
        })
    });
    Ok(Some((hits, loaded)))
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
        .filter_map(|p| format_fts5_phrase(p))
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
