// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! SQLite WAL connection pragmas, DDL schema migration, and table statistics for `.cache/org-cache.db`.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::cache::{CacheStatus, CacheTelemetry, CachedHeadingCandidate, CachedIdCandidate};

/// Current schema version stored in SQLite `PRAGMA user_version`.
pub(crate) const SCHEMA_USER_VERSION: i64 = 4;

/// Per-connection session pragmas applied on every open handle.
const SESSION_PRAGMAS_SQL: &str = "
    PRAGMA foreign_keys = ON;
    PRAGMA synchronous = NORMAL;
    PRAGMA temp_store = MEMORY;
";

/// Schema DDL creating all workspace cache tables and indexes.
const SCHEMA_DDL_SQL: &str = "
    PRAGMA journal_mode = WAL;
    BEGIN;
    CREATE TABLE IF NOT EXISTS files (
        rel_path TEXT PRIMARY KEY,
        mtime_ns INTEGER NOT NULL,
        byte_len INTEGER NOT NULL,
        content_hash TEXT NOT NULL,
        is_symlink INTEGER NOT NULL DEFAULT 0,
        canon_key TEXT NOT NULL DEFAULT '',
        is_roam_dup INTEGER NOT NULL DEFAULT 0,
        todo_active_json TEXT NOT NULL DEFAULT '[]',
        todo_done_json TEXT NOT NULL DEFAULT '[]',
        filetags_json TEXT NOT NULL DEFAULT '[]',
        file_prop_keys_json TEXT NOT NULL DEFAULT '[]'
    );
    CREATE INDEX IF NOT EXISTS idx_files_symlink ON files(rel_path) WHERE is_symlink = 1;
    CREATE INDEX IF NOT EXISTS idx_files_roam_dup ON files(rel_path) WHERE is_roam_dup = 1;
    CREATE TABLE IF NOT EXISTS nodes (
        id TEXT NOT NULL,
        rel_path TEXT NOT NULL REFERENCES files(rel_path) ON DELETE CASCADE,
        id_seed TEXT,
        title TEXT NOT NULL,
        line INTEGER NOT NULL,
        level INTEGER NOT NULL,
        todo_keyword TEXT,
        priority TEXT,
        tags_json TEXT NOT NULL,
        olp_json TEXT NOT NULL,
        aliases_json TEXT NOT NULL,
        refs_json TEXT NOT NULL,
        roam_excluded INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS idx_nodes_id_lower ON nodes(lower(id));
    CREATE INDEX IF NOT EXISTS idx_nodes_title_lower ON nodes(lower(title));
    CREATE INDEX IF NOT EXISTS idx_nodes_rel_path ON nodes(rel_path);
    CREATE TABLE IF NOT EXISTS headings (
        rel_path TEXT NOT NULL REFERENCES files(rel_path) ON DELETE CASCADE,
        entry_idx INTEGER NOT NULL,
        heading_line INTEGER NOT NULL,
        level INTEGER NOT NULL,
        todo_keyword TEXT,
        title TEXT NOT NULL,
        custom_id TEXT,
        tags_json TEXT NOT NULL DEFAULT '[]',
        prop_keys_json TEXT NOT NULL DEFAULT '[]'
    );
    CREATE INDEX IF NOT EXISTS idx_headings_custom_id_lower ON headings(lower(custom_id)) WHERE custom_id IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_headings_rel_path ON headings(rel_path);
    CREATE TABLE IF NOT EXISTS links (
        rel_path TEXT NOT NULL REFERENCES files(rel_path) ON DELETE CASCADE,
        source_id TEXT,
        target_id TEXT NOT NULL,
        description TEXT,
        line INTEGER NOT NULL,
        context TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_links_target_lower ON links(lower(target_id));
    CREATE INDEX IF NOT EXISTS idx_links_source_lower ON links(lower(source_id));
    CREATE INDEX IF NOT EXISTS idx_links_rel_path ON links(rel_path);
    CREATE TABLE IF NOT EXISTS edna_edges (
        rel_path TEXT NOT NULL REFERENCES files(rel_path) ON DELETE CASCADE,
        entry_idx INTEGER NOT NULL,
        entry_id TEXT,
        line INTEGER NOT NULL,
        kind TEXT NOT NULL,
        target_id TEXT NOT NULL,
        raw_expr TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_edna_rel_path ON edna_edges(rel_path);
    CREATE INDEX IF NOT EXISTS idx_edna_entry_id_lower ON edna_edges(lower(entry_id));
    CREATE INDEX IF NOT EXISTS idx_edna_target_id_lower ON edna_edges(lower(target_id));
    CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
        rel_path UNINDEXED,
        entry_idx UNINDEXED,
        heading_line UNINDEXED,
        title,
        body,
        tokenize = 'unicode61 remove_diacritics 0'
    );
    PRAGMA user_version = 4;
    COMMIT;
";

/// Row counts across all five workspace cache tables.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TableCounts {
    /// Row count in `files`.
    pub(crate) files_indexed: usize,
    /// Row count in `nodes`.
    pub(crate) nodes_count: usize,
    /// Row count in `links`.
    pub(crate) links_count: usize,
    /// Row count in `edna_edges`.
    pub(crate) edna_edges_count: usize,
    /// Row count in `entries_fts`.
    pub(crate) fts_entries_count: usize,
}

/// Applies session pragmas and initializes the WAL + FTS5 schema when `user_version` differs.
pub(crate) fn configure_connection(conn: &Connection, force_ddl: bool) -> Result<(), String> {
    conn.execute_batch(SESSION_PRAGMAS_SQL)
        .map_err(|e| format!("failed to configure SQLite session pragmas: {e}"))?;

    let version: i64 = conn
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .map_err(|e| format!("failed to read SQLite user_version: {e}"))?;

    if version != 0 && version != SCHEMA_USER_VERSION {
        conn.execute_batch(
            "DROP TABLE IF EXISTS entries_fts;
             DROP TABLE IF EXISTS headings;
             DROP TABLE IF EXISTS nodes;
             DROP TABLE IF EXISTS links;
             DROP TABLE IF EXISTS edna_edges;
             DROP TABLE IF EXISTS files;",
        )
        .map_err(|e| format!("failed to reset outdated workspace SQLite schema: {e}"))?;
    }
    if force_ddl || version != SCHEMA_USER_VERSION {
        conn.execute_batch(SCHEMA_DDL_SQL)
            .map_err(|e| format!("failed to initialize workspace SQLite schema: {e}"))?;
    }
    Ok(())
}

/// Opens an ephemeral in-memory SQLite handle initialized with the workspace cache schema.
pub(crate) fn open_in_memory_configured() -> Result<Connection, String> {
    let conn = Connection::open_in_memory()
        .map_err(|e| format!("failed to open in-memory SQLite handle: {e}"))?;
    configure_connection(&conn, true)?;
    Ok(conn)
}

/// Deletes all rows across `entries_fts` and `files` (cascading to `nodes`, `links`, `edna_edges`).
pub(crate) fn clear_all_rows(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("DELETE FROM entries_fts; DELETE FROM files;")
        .map_err(|e| format!("failed to clear workspace cache rows: {e}"))
}

/// Queries row counts across `files`, `nodes`, `links`, `edna_edges`, and `entries_fts`.
pub(crate) fn query_table_counts(conn: &Connection) -> Result<TableCounts, String> {
    let sql = "
        SELECT
            (SELECT COUNT(*) FROM files),
            (SELECT COUNT(*) FROM nodes WHERE roam_excluded = 0),
            (SELECT COUNT(*) FROM links),
            (SELECT COUNT(*) FROM edna_edges),
            (SELECT COUNT(*) FROM entries_fts)
    ";
    conn.query_row(sql, [], |row| {
        Ok(TableCounts {
            files_indexed: row.get::<_, i64>(0)? as usize,
            nodes_count: row.get::<_, i64>(1)? as usize,
            links_count: row.get::<_, i64>(2)? as usize,
            edna_edges_count: row.get::<_, i64>(3)? as usize,
            fts_entries_count: row.get::<_, i64>(4)? as usize,
        })
    })
    .map_err(|e| format!("failed to query workspace cache table counts: {e}"))
}

/// Lists all table names present in `sqlite_master` sorted alphabetically.
pub(crate) fn list_table_names(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name ASC")
        .map_err(|e| format!("failed to query sqlite_master: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("failed to read sqlite_master rows: {e}"))?;
    let mut out = Vec::new();
    for item in rows {
        out.push(item.map_err(|e| format!("sqlite_master decode error: {e}"))?);
    }
    Ok(out)
}

/// Constructs a sibling SQLite sidecar path such as `<db_path>-wal` or `<db_path>-shm`.
pub(crate) fn sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut os: OsString = db_path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

/// Computes the combined on-disk byte size of `db_path` and any active `-wal` sidecar.
pub(crate) fn database_disk_bytes(db_path: &Path) -> u64 {
    let main_len = fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let wal_len = fs::metadata(sidecar_path(db_path, "-wal"))
        .map(|m| m.len())
        .unwrap_or(0);
    main_len.saturating_add(wal_len)
}

/// Builds a [`CacheStatus`] summary for `db_path` from optional [`TableCounts`] and `telemetry`.
pub(crate) fn build_cache_status(
    db_path: &Path,
    counts: Option<TableCounts>,
    telemetry: CacheTelemetry,
) -> CacheStatus {
    let exists = counts.is_some();
    let c = counts.unwrap_or_default();
    CacheStatus {
        db_path: db_path.display().to_string(),
        exists,
        size_bytes: if exists {
            database_disk_bytes(db_path)
        } else {
            0
        },
        files_indexed: c.files_indexed,
        nodes_count: c.nodes_count,
        links_count: c.links_count,
        edna_edges_count: c.edna_edges_count,
        fts_entries_count: c.fts_entries_count,
        telemetry,
    }
}

/// Adjusts `(scanned, hits, misses)` to exclude intra-workspace `is_roam_dup = 1` symlinked files.
pub(crate) fn adjust_roam_telemetry_counts(
    conn: &Connection,
    scanned: usize,
    hits: usize,
    misses: usize,
    missed_rels: &[String],
) -> Result<(usize, usize, usize), String> {
    let dup_total =
        conn.query_row(
            "SELECT COUNT(*) FROM files WHERE is_roam_dup = 1",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| format!("failed to count duplicate roam files: {e}"))? as usize;
    if dup_total == 0 {
        return Ok((scanned, hits, misses));
    }
    let mut dup_misses = 0usize;
    if !missed_rels.is_empty() {
        let mut stmt = conn
            .prepare_cached("SELECT is_roam_dup FROM files WHERE rel_path = ?1")
            .map_err(|e| format!("failed to prepare dup lookup: {e}"))?;
        for rel in missed_rels {
            if stmt.query_row([rel], |r| r.get::<_, i64>(0)).unwrap_or(0) != 0 {
                dup_misses += 1;
            }
        }
    }
    let dup_hits = dup_total.saturating_sub(dup_misses);
    Ok((
        scanned.saturating_sub(dup_total),
        hits.saturating_sub(dup_hits),
        misses.saturating_sub(dup_misses),
    ))
}

/// Queries `:ID:` completion candidates (including `:ROAM_EXCLUDE: t` entries) matching `query`.
pub(crate) fn query_completion_ids(
    conn: &Connection,
    query: &str,
) -> Result<Vec<CachedIdCandidate>, String> {
    let needle = query
        .trim()
        .strip_prefix("id:")
        .unwrap_or(query.trim())
        .to_ascii_lowercase();
    let sql = "
        SELECT n.id, n.id_seed, n.title, n.rel_path, n.line, n.level,
               n.todo_keyword, n.aliases_json, n.roam_excluded
        FROM nodes n
        JOIN files f ON f.rel_path = n.rel_path
        WHERE f.is_roam_dup = 0
        ORDER BY n.rel_path ASC, n.line ASC, n.id ASC
    ";
    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| format!("failed to prepare ID completion query: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            let aliases_j: String = r.get(7)?;
            let aliases: Vec<String> = serde_json::from_str(&aliases_j).unwrap_or_default();
            Ok(CachedIdCandidate {
                id: r.get(0)?,
                id_seed: r.get(1)?,
                title: r.get(2)?,
                rel_path: r.get(3)?,
                line: r.get::<_, i64>(4)? as usize,
                level: r.get::<_, i64>(5)? as usize,
                todo_keyword: r.get(6)?,
                aliases,
                roam_excluded: r.get::<_, i64>(8)? != 0,
            })
        })
        .map_err(|e| format!("failed to execute ID completion query: {e}"))?;
    let mut out = Vec::new();
    for item in rows {
        let cand = item.map_err(|e| format!("ID completion row decode error: {e}"))?;
        if needle.is_empty()
            || cand.id.to_ascii_lowercase().contains(&needle)
            || cand.title.to_ascii_lowercase().contains(&needle)
            || cand
                .id_seed
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(&needle))
            || cand
                .aliases
                .iter()
                .any(|a| a.to_ascii_lowercase().contains(&needle))
        {
            out.push(cand);
        }
    }
    Ok(out)
}

/// Queries heading outline and `:CUSTOM_ID:` candidates matching `query`.
pub(crate) fn query_completion_headings(
    conn: &Connection,
    query: &str,
) -> Result<Vec<CachedHeadingCandidate>, String> {
    let needle = query
        .trim()
        .strip_prefix('#')
        .unwrap_or(query.trim())
        .to_ascii_lowercase();
    let sql = "
        SELECT h.rel_path, h.heading_line, h.level, h.todo_keyword, h.title, h.custom_id
        FROM headings h
        JOIN files f ON f.rel_path = h.rel_path
        WHERE f.is_roam_dup = 0
        ORDER BY h.rel_path ASC, h.heading_line ASC
    ";
    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| format!("failed to prepare heading completion query: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(CachedHeadingCandidate {
                rel_path: r.get(0)?,
                heading_line: r.get::<_, i64>(1)? as usize,
                level: r.get::<_, i64>(2)? as usize,
                todo_keyword: r.get(3)?,
                title: r.get(4)?,
                custom_id: r.get(5)?,
            })
        })
        .map_err(|e| format!("failed to execute heading completion query: {e}"))?;
    let mut out = Vec::new();
    for item in rows {
        let cand = item.map_err(|e| format!("heading completion decode error: {e}"))?;
        if needle.is_empty()
            || cand.title.to_ascii_lowercase().contains(&needle)
            || cand.rel_path.to_ascii_lowercase().contains(&needle)
            || cand
                .custom_id
                .as_deref()
                .is_some_and(|c| c.to_ascii_lowercase().contains(&needle))
        {
            out.push(cand);
        }
    }
    Ok(out)
}

/// Collects distinct JSON string array elements across `sql` matching case-insensitive `prefix`.
pub(crate) fn query_distinct_json_tokens(
    conn: &Connection,
    sql: &str,
    prefix: &str,
    uppercase: bool,
) -> Result<Vec<String>, String> {
    let pfx = prefix.trim().to_ascii_lowercase();
    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| format!("failed to prepare token completion query: {e}"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| format!("failed to execute token completion query: {e}"))?;
    let mut seen = BTreeSet::new();
    for item in rows {
        let raw = item.map_err(|e| format!("token completion decode error: {e}"))?;
        if let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) {
            for tok in list {
                let norm = if uppercase {
                    tok.to_ascii_uppercase()
                } else {
                    tok
                };
                if pfx.is_empty() || norm.to_ascii_lowercase().starts_with(&pfx) {
                    seen.insert(norm);
                }
            }
        }
    }
    Ok(seen.into_iter().collect())
}

/// Queries distinct `(active, done)` TODO keywords across all non-duplicate workspace files.
pub(crate) fn query_completion_todos(
    conn: &Connection,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT todo_active_json, todo_done_json FROM files WHERE is_roam_dup = 0 ORDER BY rel_path ASC",
        )
        .map_err(|e| format!("failed to prepare TODO completion query: {e}"))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("failed to execute TODO completion query: {e}"))?;
    let (mut active, mut done) = (Vec::new(), Vec::new());
    for item in rows {
        let (act_j, done_j) = item.map_err(|e| format!("TODO completion decode error: {e}"))?;
        for kw in serde_json::from_str::<Vec<String>>(&act_j).unwrap_or_default() {
            if !active.contains(&kw) {
                active.push(kw);
            }
        }
        for kw in serde_json::from_str::<Vec<String>>(&done_j).unwrap_or_default() {
            if !done.contains(&kw) {
                done.push(kw);
            }
        }
    }
    Ok((active, done))
}
