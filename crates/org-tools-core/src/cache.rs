// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Incremental SQLite WAL + FTS5 workspace index (`.cache/mabuse.db`).
//!
//! Maintains a disposable five-table SQLite database (`files`, `nodes`, `links`,
//! `edna_edges`, and `entries_fts`) keyed by `(rel_path, mtime_ns, byte_len, content_hash)`
//! for `< 5 ms` warm `org roam`, `edna`, and BM25 `body:"..."` queries without
//! reading or writing Emacs's `org-roam.db`.

mod fts;
mod payload;
mod roam;
mod schema;
mod sha256;
mod sync;
#[cfg(test)]
mod tests;
mod walk;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::export_md::normalize_workspace_path;
use crate::files::collect_org_files;
use crate::roam::{RoamBacklink, RoamIndex, RoamNode, RoamOutgoingLink};

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
    adjust_roam_telemetry_counts, build_cache_status, configure_connection, list_table_names,
    open_in_memory_configured, query_table_counts, sidecar_path,
};
use sync::{sync_discovered_files, sync_workspace};
use walk::discover_org_files;

/// Default workspace-relative path for the Mabuse SQLite index.
pub const DEFAULT_CACHE_REL_PATH: &str = ".cache/mabuse.db";

/// Environment variable overriding the Mabuse SQLite database file path.
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

/// Summary of the on-disk `.cache/mabuse.db` index returned by `org cache status`.
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

/// Handle to an open Mabuse SQLite WAL + FTS5 cache for a workspace root.
pub struct MabuseCache {
    /// Open SQLite database connection.
    conn: Connection,
    /// Normalized workspace root directory.
    workspace_root: PathBuf,
    /// Path to the `.cache/mabuse.db` file.
    db_path: PathBuf,
    /// Relative paths re-parsed during the most recent `open_and_sync` call.
    missed_rels: Vec<String>,
}

/// Unified `org-roam` v2 query backend backed either by [`MabuseCache`] or a direct [`RoamIndex`].
pub enum RoamWorkspace {
    /// Incremental SQLite + FTS5 cache bound to a single workspace directory.
    Cached {
        /// Open synchronized SQLite cache handle.
        cache: Box<MabuseCache>,
        /// Display root path supplied by the caller.
        display_root: PathBuf,
        /// Scan counters `(files_scanned, cache_hits, cache_misses)`.
        counts: (usize, usize, usize),
    },
    /// Uncached in-memory [`RoamIndex`] built directly from `.org` files.
    Direct(RoamIndex),
}

impl MabuseCache {
    /// Resolves the SQLite database path for `workspace_root`, honoring [`CACHE_PATH_ENV_VAR`].
    pub fn resolve_db_path(workspace_root: &Path) -> PathBuf {
        if let Some(val) = std::env::var_os(CACHE_PATH_ENV_VAR).filter(|v| !v.is_empty()) {
            return PathBuf::from(val);
        }
        workspace_root.join(DEFAULT_CACHE_REL_PATH)
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

    /// Opens `db_path` (creating parent directories if needed) and ensures WAL + FTS5 schema.
    fn open_rw(workspace_root: &Path, force_ddl: bool) -> Result<Self, String> {
        let norm_root = normalize_workspace_path(workspace_root);
        let db_path = Self::resolve_db_path(&norm_root);
        Self::open_rw_normalized(norm_root, db_path, force_ddl)
    }

    /// Inspects cache status at `workspace_root` without creating `.cache/mabuse.db` when absent.
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

    /// Rebuilds `.cache/mabuse.db` from scratch inside a single transaction.
    pub fn rebuild(workspace_root: &Path) -> Result<(Self, CacheStatus), String> {
        let mut cache = Self::open_rw(workspace_root, true)?;
        let telemetry = sync_workspace(&mut cache.conn, &cache.workspace_root, true)?;
        let counts = query_table_counts(&cache.conn)?;
        let status = build_cache_status(&cache.db_path, Some(counts), telemetry);
        Ok((cache, status))
    }

    /// Removes `.cache/mabuse.db` and any `-wal` / `-shm` sidecar files.
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

    /// Opens `.cache/mabuse.db` and incrementally synchronizes modified, added, and deleted `.org` files.
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

impl RoamWorkspace {
    /// Opens a synchronized [`MabuseCache`] on a single directory root, or builds a [`RoamIndex`].
    pub fn open(roots: &[PathBuf], no_cache: bool) -> Result<Self, String> {
        if !no_cache && roots.len() == 1 && roots[0].is_dir() {
            let display_root = roots[0].clone();
            let (cache, t) = MabuseCache::open_and_sync(&display_root)?;
            let counts = adjust_roam_telemetry_counts(
                &cache.conn,
                t.files_scanned,
                t.cache_hits,
                t.cache_misses,
                &cache.missed_rels,
            )?;
            return Ok(Self::Cached {
                cache: Box::new(cache),
                display_root,
                counts,
            });
        }
        let raw_files = collect_org_files(roots);
        let mut seen = HashSet::with_capacity(raw_files.len());
        let mut unique_files = Vec::with_capacity(raw_files.len());
        for file in raw_files {
            if seen.insert(normalize_workspace_path(&file)) {
                unique_files.push(file);
            }
        }
        Ok(Self::Direct(RoamIndex::from_files(&unique_files)?))
    }

    /// Returns `(files_scanned, cache_hits, cache_misses)` for telemetry output.
    pub fn counts(&self) -> (usize, usize, usize) {
        match self {
            Self::Cached { counts, .. } => *counts,
            Self::Direct(idx) => (idx.files_scanned, 0, idx.files_scanned),
        }
    }

    /// Resolves `query` into matching [`RoamNode`] values.
    pub fn resolve(&self, query: &str) -> Result<Vec<RoamNode>, String> {
        match self {
            Self::Cached {
                cache,
                display_root,
                ..
            } => cache.resolve_roam_nodes(display_root, query, true),
            Self::Direct(idx) => Ok(idx.resolve_nodes(query, true)),
        }
    }

    /// Fetches incoming [`RoamBacklink`] edges for `targets`.
    pub fn backlinks(&self, targets: &[RoamNode]) -> Result<Vec<RoamBacklink>, String> {
        match self {
            Self::Cached {
                cache,
                display_root,
                ..
            } => cache.roam_backlinks_for(display_root, targets),
            Self::Direct(idx) => Ok(idx.backlinks_for(targets)),
        }
    }

    /// Fetches outgoing [`RoamOutgoingLink`] edges for `sources`.
    pub fn outgoing(&self, sources: &[RoamNode]) -> Result<Vec<RoamOutgoingLink>, String> {
        match self {
            Self::Cached {
                cache,
                display_root,
                ..
            } => cache.roam_outgoing_links_for(display_root, sources),
            Self::Direct(idx) => Ok(idx.outgoing_links_for(sources)),
        }
    }

    /// Consumes the workspace backend into a full [`RoamIndex`] and its scan counters.
    pub fn into_graph(self) -> Result<(RoamIndex, (usize, usize, usize)), String> {
        let counts = self.counts();
        match self {
            Self::Cached {
                cache,
                display_root,
                ..
            } => Ok((cache.load_roam_graph(&display_root, counts.0)?, counts)),
            Self::Direct(idx) => Ok((idx, counts)),
        }
    }
}
