// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Incremental SQLite WAL + FTS5 workspace index (`.cache/org-cache.db`).
//!
//! Maintains a disposable five-table SQLite database (`files`, `nodes`, `links`,
//! `edna_edges`, and `entries_fts`) keyed by `(rel_path, mtime_ns, byte_len, content_hash)`
//! for `< 5 ms` warm `org roam`, `edna`, and BM25 `body:"..."` queries without
//! reading or writing Emacs's `org-roam.db`.

mod federated;
mod fts;
mod payload;
mod roam;
mod schema;
mod sha256;
mod sync;
#[cfg(test)]
mod tests;
mod walk;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::export_md::normalize_workspace_path;
use crate::files::join_slash_rel;
use crate::roam::{RoamBacklink, RoamIndex, RoamNode, RoamOutgoingLink};

pub use federated::{FederatedVault, RoamWorkspace};
pub use fts::{
    build_fts5_match_expr, count_phrase_occurrences, extract_entry_bodies,
    load_cached_fts_candidate_docs, load_docs_with_bodies, matches_body_phrase,
    rank_matched_indices_bm25, tokenize_unicode61, BodyLoadedDoc, CachedFtsPlan,
};

use fts::query_fts5_bm25;
use roam::{
    load_graph_cached, query_backlinks_cached, query_blocker_target_paths,
    query_outgoing_links_cached, resolve_nodes_cached,
};
use schema::{
    build_cache_status, configure_connection, list_table_names, open_in_memory_configured,
    query_table_counts, sidecar_path,
};
use sync::{
    apply_full_rebuild, collect_and_parse_all, invalidate_cached_paths, sync_discovered_files,
};
use walk::discover_org_files;

/// Default workspace-relative path for the SQLite workspace index.
pub const DEFAULT_CACHE_REL_PATH: &str = ".cache/org-cache.db";

/// Environment variable overriding the SQLite workspace database file path.
pub const CACHE_PATH_ENV_VAR: &str = "ORG_TOOLS_CACHE_PATH";

/// Timing and cache-efficiency metrics recorded during a cache operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CacheTelemetry {
    /// Wall-clock operation duration in microseconds.
    pub duration_us: u64,
    /// Number of `.org` files inspected on disk.
    pub files_scanned: usize,
    /// Number of `.org` files satisfied from cached metadata without re-parsing.
    pub cache_hits: usize,
    /// Number of `.org` files parsed and indexed into SQLite.
    pub cache_misses: usize,
}

/// Summary of the on-disk `.cache/org-cache.db` index returned by `org cache status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheStatus {
    /// Resolved path to the SQLite database file.
    pub db_path: String,
    /// Whether the SQLite database file currently exists on disk.
    pub exists: bool,
    /// Total byte size of the SQLite database and active WAL file on disk.
    pub size_bytes: u64,
    /// Row count in the `files` table.
    pub files_indexed: usize,
    /// Row count in the `nodes` table.
    pub nodes_count: usize,
    /// Row count in the `links` table.
    pub links_count: usize,
    /// Row count in the `edna_edges` table.
    pub edna_edges_count: usize,
    /// Row count in the `entries_fts` virtual table.
    pub fts_entries_count: usize,
    /// Execution telemetry for the status or rebuild operation.
    pub telemetry: CacheTelemetry,
}

/// A BM25-ranked heading entry match from the `entries_fts` FTS5 table.
#[derive(Debug, Clone, PartialEq)]
pub struct FtsHit {
    /// Workspace-relative `/` path of the file containing the matched heading.
    pub rel_path: String,
    /// Zero-based index into [`crate::document::OrgDocument::entries`].
    pub entry_idx: usize,
    /// One-based heading line number.
    pub heading_line: usize,
    /// Positive BM25 relevance score (higher is more relevant).
    pub score: f64,
}

/// Handle to an open SQLite WAL + FTS5 cache for a workspace root.
pub struct WorkspaceCache {
    /// Open SQLite database connection.
    pub(crate) conn: Connection,
    /// Normalized workspace root directory.
    workspace_root: PathBuf,
    /// Path to the `.cache/org-cache.db` file.
    db_path: PathBuf,
    /// Relative paths re-parsed during the most recent `open_and_sync` call.
    pub(crate) missed_rels: Vec<String>,
}

impl WorkspaceCache {
    /// Resolves the SQLite database path for `workspace_root`, honoring [`CACHE_PATH_ENV_VAR`].
    pub fn resolve_db_path(workspace_root: &Path) -> PathBuf {
        if let Some(val) = std::env::var_os(CACHE_PATH_ENV_VAR).filter(|v| !v.is_empty()) {
            return PathBuf::from(val);
        }
        join_slash_rel(workspace_root, DEFAULT_CACHE_REL_PATH)
    }

    /// Opens `db_path` for an already-normalized `norm_root` and ensures WAL + FTS5 schema.
    fn open_rw_normalized(
        norm_root: PathBuf,
        db_path: PathBuf,
        force_ddl: bool,
    ) -> Result<Self, String> {
        if force_ddl || !db_path.is_file() {
            if let Some(parent) = db_path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!("failed to create cache directory {}: {e}", parent.display())
                })?;
            }
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&db_path, flags)
            .map_err(|e| format!("failed to open {}: {e}", db_path.display()))?;
        configure_connection(&conn, force_ddl)?;
        Ok(Self {
            conn,
            workspace_root: norm_root,
            db_path,
            missed_rels: Vec::new(),
        })
    }

    /// Inspects cache status at `workspace_root` without creating `.cache/org-cache.db` when absent.
    pub fn status(workspace_root: &Path) -> Result<CacheStatus, String> {
        let started = Instant::now();
        let norm_root = normalize_workspace_path(workspace_root);
        let db_path = Self::resolve_db_path(&norm_root);
        if !db_path.is_file() {
            let tel = CacheTelemetry {
                duration_us: started.elapsed().as_micros() as u64,
                ..CacheTelemetry::default()
            };
            return Ok(build_cache_status(&db_path, None, tel));
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&db_path, flags)
            .map_err(|e| format!("failed to open {}: {e}", db_path.display()))?;
        let counts = query_table_counts(&conn)?;
        let tel = CacheTelemetry {
            duration_us: started.elapsed().as_micros() as u64,
            ..CacheTelemetry::default()
        };
        Ok(build_cache_status(&db_path, Some(counts), tel))
    }

    /// Rebuilds `.cache/org-cache.db` from scratch inside a single transaction.
    pub fn rebuild(workspace_root: &Path) -> Result<(Self, CacheStatus), String> {
        let started = Instant::now();
        let norm_root = normalize_workspace_path(workspace_root);
        let db_path = Self::resolve_db_path(&norm_root);
        let existed = db_path.is_file();
        let (upserts_res, db_res) = std::thread::scope(|s| {
            let parser = s.spawn(|| collect_and_parse_all(&norm_root));
            let db = Self::open_rw_normalized(norm_root.clone(), db_path, true);
            (parser.join().unwrap(), db)
        });
        let mut cache = db_res?;
        let telemetry = apply_full_rebuild(&mut cache.conn, existed, upserts_res?, started)?;
        let counts = query_table_counts(&cache.conn)?;
        let status = build_cache_status(&cache.db_path, Some(counts), telemetry);
        Ok((cache, status))
    }

    /// Removes `.cache/org-cache.db` and any `-wal` / `-shm` sidecar files.
    pub fn clear(workspace_root: &Path) -> Result<(PathBuf, bool), String> {
        let norm_root = normalize_workspace_path(workspace_root);
        let db_path = Self::resolve_db_path(&norm_root);
        let mut removed = false;
        for candidate in [
            db_path.clone(),
            sidecar_path(&db_path, "-wal"),
            sidecar_path(&db_path, "-shm"),
        ] {
            if candidate.exists() {
                fs::remove_file(&candidate)
                    .map_err(|e| format!("failed to remove {}: {e}", candidate.display()))?;
                removed = true;
            }
        }
        Ok((db_path, removed))
    }

    /// Opens `.cache/org-cache.db` and incrementally synchronizes modified, added, and deleted `.org` files.
    pub fn open_and_sync(workspace_root: &Path) -> Result<(Self, CacheTelemetry), String> {
        let started = Instant::now();
        let norm_root = normalize_workspace_path(workspace_root);
        let db_path = Self::resolve_db_path(&norm_root);
        if !db_path.is_file() {
            let disk_files = discover_org_files(&norm_root)?;
            if disk_files.is_empty() {
                let conn = open_in_memory_configured()?;
                let tel = CacheTelemetry {
                    duration_us: started.elapsed().as_micros() as u64,
                    ..CacheTelemetry::default()
                };
                let empty = Self {
                    conn,
                    workspace_root: norm_root,
                    db_path,
                    missed_rels: Vec::new(),
                };
                return Ok((empty, tel));
            }
            let mut cache = Self::open_rw_normalized(norm_root, db_path, false)?;
            let (tel, missed) = sync_discovered_files(
                &mut cache.conn,
                &cache.workspace_root,
                &disk_files,
                started,
            )?;
            cache.missed_rels = missed;
            return Ok((cache, tel));
        }
        let (disk_res, db_res) = std::thread::scope(|s| {
            let walker = s.spawn(|| discover_org_files(&norm_root));
            let db = Self::open_rw_normalized(norm_root.clone(), db_path, false);
            (walker.join().unwrap(), db)
        });
        let mut cache = db_res?;
        let (tel, missed) =
            sync_discovered_files(&mut cache.conn, &cache.workspace_root, &disk_res?, started)?;
        cache.missed_rels = missed;
        Ok((cache, tel))
    }

    /// Incrementally re-synchronizes this open SQLite handle against workspace disk changes.
    pub fn refresh(&mut self) -> Result<CacheTelemetry, String> {
        let started = Instant::now();
        let disk_files = discover_org_files(&self.workspace_root)?;
        if !self.db_path.is_file() && !disk_files.is_empty() {
            let fresh =
                Self::open_rw_normalized(self.workspace_root.clone(), self.db_path.clone(), false)?;
            self.conn = fresh.conn;
        }
        let (tel, missed) =
            sync_discovered_files(&mut self.conn, &self.workspace_root, &disk_files, started)?;
        self.missed_rels = missed;
        Ok(tel)
    }

    /// Marks `paths` stale in `files` so the next `refresh()` re-hashes them regardless of `mtime`.
    pub fn invalidate_paths(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        invalidate_cached_paths(&self.conn, &self.workspace_root, paths)
    }

    /// Returns the workspace root directory bound to this cache handle.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Returns the SQLite database file path bound to this cache handle.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Lists user table names present in `sqlite_master`.
    pub fn table_names(&self) -> Result<Vec<String>, String> {
        list_table_names(&self.conn)
    }

    /// Resolves `org-roam` v2 nodes by `id:`, `:ID:`, title, alias, or `OrgLocator`.
    pub fn resolve_roam_nodes(
        &self,
        display_root: &Path,
        query: &str,
        allow_locator: bool,
    ) -> Result<Vec<RoamNode>, String> {
        resolve_nodes_cached(
            &self.conn,
            &self.workspace_root,
            display_root,
            query,
            allow_locator,
        )
    }

    /// Queries incoming `[[id:...]]` backlinks for `targets` from SQLite.
    pub fn roam_backlinks_for(
        &self,
        display_root: &Path,
        targets: &[RoamNode],
    ) -> Result<Vec<RoamBacklink>, String> {
        query_backlinks_cached(&self.conn, display_root, targets)
    }

    /// Queries outgoing `[[id:...]]` links for `sources` from SQLite.
    pub fn roam_outgoing_links_for(
        &self,
        display_root: &Path,
        sources: &[RoamNode],
    ) -> Result<Vec<RoamOutgoingLink>, String> {
        query_outgoing_links_cached(&self.conn, display_root, sources)
    }

    /// Reconstructs a workspace [`RoamIndex`] from SQLite `nodes` and `links` tables.
    pub fn load_roam_graph(
        &self,
        display_root: &Path,
        files_scanned: usize,
    ) -> Result<RoamIndex, String> {
        load_graph_cached(&self.conn, display_root, files_scanned)
    }

    /// Executes an FTS5 BM25 search for `phrases` (`AND`-joined) ordered best-first.
    pub fn search_fts_bm25(&self, phrases: &[String]) -> Result<Vec<FtsHit>, String> {
        query_fts5_bm25(&self.conn, phrases)
    }

    /// Resolves relative file paths containing `:BLOCKER:` `ids(...)` targets referenced by `rel_paths`.
    pub fn blocker_target_rel_paths(&self, rel_paths: &[String]) -> Result<Vec<String>, String> {
        query_blocker_target_paths(&self.conn, rel_paths)
    }
}
