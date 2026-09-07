// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Persistent SQLite cache for `org-tools`.
//!
//! Provides fast indexing and $O(1)$ lookups for entry `:ID:` and `:CUSTOM_ID:` properties,
//! tags, and dependencies across large org workspaces. Supports automatic schema
//! migrations with fallback to cache recreation if the database is corrupt or not migratable.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::document::OrgDocument;
use crate::source::SourceFile;

/// Current schema version of the SQLite cache.
pub const CURRENT_SCHEMA_VERSION: i32 = 1;

/// A database schema migration step.
#[derive(Debug, Clone)]
pub struct Migration {
    /// Schema version before applying this migration.
    pub from_version: i32,
    /// Schema version after applying this migration.
    pub to_version: i32,
    /// Human-readable description of changes.
    pub description: &'static str,
    /// SQL statements executed in a transaction.
    pub sql: &'static str,
}

/// Registered database migrations in order of version.
pub const MIGRATIONS: &[Migration] = &[
    // Future migrations will be appended here.
];

/// Errors returned by the cache subsystem.
#[derive(Debug)]
pub enum CacheError {
    /// SQLite engine error.
    Sqlite(rusqlite::Error),
    /// File system I/O error.
    Io(std::io::Error),
    /// Migration failed and could not be applied.
    MigrationFailed(String),
    /// Invalid data encountered in cache.
    InvalidData(String),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::Sqlite(e) => write!(f, "sqlite error: {e}"),
            CacheError::Io(e) => write!(f, "io error: {e}"),
            CacheError::MigrationFailed(msg) => write!(f, "migration failed: {msg}"),
            CacheError::InvalidData(msg) => write!(f, "invalid cache data: {msg}"),
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CacheError::Sqlite(e) => Some(e),
            CacheError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for CacheError {
    fn from(err: rusqlite::Error) -> Self {
        CacheError::Sqlite(err)
    }
}

impl From<std::io::Error> for CacheError {
    fn from(err: std::io::Error) -> Self {
        CacheError::Io(err)
    }
}

/// Computes a fast, deterministic 64-bit FNV-1a hash of bytes.
pub fn compute_content_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Statistics gathered during a cache synchronization run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Number of files checked.
    pub total_files: usize,
    /// Files that did not change (mtime + size or content hash hit).
    pub cache_hits: usize,
    /// Files newly parsed and inserted into cache.
    pub updated_files: usize,
    /// Obsolete files pruned from cache.
    pub deleted_files: usize,
    /// Total headings currently in cache.
    pub total_entries: usize,
}

/// Overview statistics of the cache database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub file_count: usize,
    pub entry_count: usize,
    pub tag_count: usize,
    pub dependency_count: usize,
    pub schema_version: i32,
}

/// Cached representation of an org heading entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedEntry {
    pub file_path: String,
    pub entry_idx: usize,
    pub level: usize,
    pub keyword: Option<String>,
    pub priority: Option<char>,
    pub title: String,
    pub org_id: Option<String>,
    pub custom_id: Option<String>,
    pub heading_line: usize,
    pub content_end_line: usize,
    pub subtree_end_line: usize,
    pub scheduled: Option<String>,
    pub deadline: Option<String>,
    pub closed: Option<String>,
    pub tags: Vec<String>,
}

/// A connection to the SQLite cache database.
pub struct CacheDb {
    conn: Connection,
    path: Option<PathBuf>,
}

impl CacheDb {
    /// Opens or creates the SQLite cache at `db_path`.
    ///
    /// If the database file is corrupt, has an incompatible schema version,
    /// or fails during migration, this method falls back to deleting the database
    /// and recreating a clean, fresh cache.
    pub fn open_or_create(db_path: &Path) -> Result<Self, CacheError> {
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }

        match Self::try_open(db_path) {
            Ok(db) => Ok(db),
            Err(e) => {
                eprintln!(
                    "[warn] SQLite cache at {} cannot be opened or migrated ({}). Recreating fresh cache...",
                    db_path.display(),
                    e
                );
                Self::recreate(db_path)
            }
        }
    }

    /// Opens an in-memory SQLite cache for ephemeral or testing use.
    pub fn open_in_memory() -> Result<Self, CacheError> {
        let mut conn = Connection::open_in_memory()?;
        Self::configure_pragmas(&conn)?;
        Self::apply_migrations(&mut conn)?;
        Ok(Self {
            conn,
            path: None,
        })
    }

    fn try_open(db_path: &Path) -> Result<Self, CacheError> {
        let mut conn = Connection::open(db_path)?;
        Self::configure_pragmas(&conn)?;
        Self::apply_migrations(&mut conn)?;
        Ok(Self {
            conn,
            path: Some(db_path.to_path_buf()),
        })
    }

    fn recreate(db_path: &Path) -> Result<Self, CacheError> {
        // Remove primary file and WAL/SHM sidecars if present
        let _ = fs::remove_file(db_path);
        let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
        let shm_path = PathBuf::from(format!("{}-shm", db_path.display()));
        let _ = fs::remove_file(wal_path);
        let _ = fs::remove_file(shm_path);

        Self::try_open(db_path)
    }

    fn configure_pragmas(conn: &Connection) -> Result<(), CacheError> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(())
    }

    fn apply_migrations(conn: &mut Connection) -> Result<(), CacheError> {
        let version: i32 = conn.query_row("PRAGMA user_version;", [], |row| row.get(0))?;

        if version == 0 {
            // Fresh database: initialize schema v1
            let tx = conn.transaction()?;
            Self::apply_v1_schema(&tx)?;
            tx.execute_batch(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION};"))?;
            tx.commit()?;
            return Ok(());
        }

        if version == CURRENT_SCHEMA_VERSION {
            return Ok(());
        }

        if version > CURRENT_SCHEMA_VERSION {
            return Err(CacheError::MigrationFailed(format!(
                "database schema version {version} is newer than supported version {CURRENT_SCHEMA_VERSION}"
            )));
        }

        // Apply registered incremental migrations in order
        let mut current = version;
        for migration in MIGRATIONS {
            if migration.from_version == current {
                let tx = conn.transaction()?;
                tx.execute_batch(migration.sql)?;
                current = migration.to_version;
                tx.execute_batch(&format!("PRAGMA user_version = {current};"))?;
                tx.commit()?;
            }
        }

        if current != CURRENT_SCHEMA_VERSION {
            return Err(CacheError::MigrationFailed(format!(
                "could not migrate database from version {version} to {CURRENT_SCHEMA_VERSION}"
            )));
        }

        Ok(())
    }

    fn apply_v1_schema(tx: &Transaction<'_>) -> Result<(), CacheError> {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY,
                path TEXT UNIQUE NOT NULL,
                size INTEGER NOT NULL,
                mtime_sec INTEGER NOT NULL,
                mtime_nsec INTEGER NOT NULL,
                hash TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY,
                file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                entry_idx INTEGER NOT NULL,
                level INTEGER NOT NULL,
                keyword TEXT,
                priority TEXT,
                title TEXT NOT NULL,
                org_id TEXT,
                custom_id TEXT,
                heading_line INTEGER NOT NULL,
                content_end_line INTEGER NOT NULL,
                subtree_end_line INTEGER NOT NULL,
                scheduled TEXT,
                deadline TEXT,
                closed TEXT
            );

            CREATE TABLE IF NOT EXISTS tags (
                entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
                tag TEXT NOT NULL COLLATE NOCASE,
                PRIMARY KEY (entry_id, tag)
            );

            CREATE TABLE IF NOT EXISTS dependencies (
                source_entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
                target_org_id TEXT NOT NULL,
                dep_kind TEXT NOT NULL,
                PRIMARY KEY (source_entry_id, target_org_id, dep_kind)
            );

            CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
            CREATE INDEX IF NOT EXISTS idx_entries_file_id ON entries(file_id);
            CREATE INDEX IF NOT EXISTS idx_entries_org_id ON entries(org_id) WHERE org_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_entries_custom_id ON entries(custom_id) WHERE custom_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_tags_tag ON tags(tag);
            CREATE INDEX IF NOT EXISTS idx_deps_target ON dependencies(target_org_id);",
        )?;
        Ok(())
    }

    /// Synchronizes a collection of file paths into the cache.
    ///
    /// Employs a fast two-tier check:
    /// 1. Stat (mtime + size) comparison against stored record.
    /// 2. If stat differs, content hash comparison.
    /// 3. If hash differs or file is new, re-parses with [`OrgDocument`] and updates entries.
    pub fn sync_files(&mut self, paths: &[PathBuf]) -> Result<SyncStats, CacheError> {
        let mut stats = SyncStats {
            total_files: paths.len(),
            ..Default::default()
        };

        for path in paths {
            let metadata = match fs::metadata(path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let size = metadata.len() as i64;
            let mtime = metadata
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default();
            let mtime_sec = mtime.as_secs() as i64;
            let mtime_nsec = mtime.subsec_nanos() as i64;

            let path_str = path.to_string_lossy().to_string();

            let existing_record: Option<(i64, i64, i64, i64, String)> = self
                .conn
                .query_row(
                    "SELECT id, size, mtime_sec, mtime_nsec, hash FROM files WHERE path = ?1",
                    params![&path_str],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .optional()?;

            if let Some((_file_id, old_size, old_mtime_sec, old_mtime_nsec, old_hash)) = existing_record {
                if old_size == size && old_mtime_sec == mtime_sec && old_mtime_nsec == mtime_nsec {
                    // Cache hit: stat matches completely
                    stats.cache_hits += 1;
                    continue;
                }

                // Stat changed: read content and compare hash
                let content = match fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let new_hash = compute_content_hash(content.as_bytes());

                if new_hash == old_hash {
                    // Content is identical (e.g. touch or unchanged save). Update stat only.
                    self.conn.execute(
                        "UPDATE files SET size = ?1, mtime_sec = ?2, mtime_nsec = ?3 WHERE path = ?4",
                        params![size, mtime_sec, mtime_nsec, &path_str],
                    )?;
                    stats.cache_hits += 1;
                    continue;
                }

                // Content changed: update document
                self.index_file_content(&path_str, size, mtime_sec, mtime_nsec, &new_hash, &content)?;
                stats.updated_files += 1;
            } else {
                // New file: read and index
                let content = match fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let new_hash = compute_content_hash(content.as_bytes());
                self.index_file_content(&path_str, size, mtime_sec, mtime_nsec, &new_hash, &content)?;
                stats.updated_files += 1;
            }
        }

        // Prune deleted files that are no longer in `paths`
        stats.deleted_files = self.prune_deleted_files(paths)?;

        stats.total_entries = self
            .conn
            .query_row("SELECT COUNT(*) FROM entries", [], |row| {
                row.get::<_, i64>(0).map(|c| c as usize)
            })
            .unwrap_or(0);

        Ok(stats)
    }

    /// Indexes a single file's content into the database within a transaction.
    pub fn index_file_content(
        &mut self,
        path_str: &str,
        size: i64,
        mtime_sec: i64,
        mtime_nsec: i64,
        hash: &str,
        content: &str,
    ) -> Result<(), CacheError> {
        let source = SourceFile::new(PathBuf::from(path_str), content.to_string());
        let doc = OrgDocument::from_source(&source);

        let tx = self.conn.transaction()?;

        // Insert or replace into files table
        tx.execute(
            "INSERT INTO files (path, size, mtime_sec, mtime_nsec, hash)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                size = excluded.size,
                mtime_sec = excluded.mtime_sec,
                mtime_nsec = excluded.mtime_nsec,
                hash = excluded.hash",
            params![path_str, size, mtime_sec, mtime_nsec, hash],
        )?;

        let file_id: i64 = tx.query_row(
            "SELECT id FROM files WHERE path = ?1",
            params![path_str],
            |row| row.get(0),
        )?;

        // Remove old entries for this file
        tx.execute("DELETE FROM entries WHERE file_id = ?1", params![file_id])?;

        // Insert new entries
        for (idx, entry) in doc.entries.iter().enumerate() {
            let priority_str = entry.priority.map(|c| c.to_string());
            let scheduled_str = entry.planning.scheduled.as_ref().map(|t| t.to_string());
            let deadline_str = entry.planning.deadline.as_ref().map(|t| t.to_string());
            let closed_str = entry.planning.closed.as_ref().map(|t| t.to_string());
            let org_id = entry.properties.get("ID").cloned();
            let custom_id = entry.properties.get("CUSTOM_ID").cloned();

            tx.execute(
                "INSERT INTO entries (
                    file_id, entry_idx, level, keyword, priority, title,
                    org_id, custom_id, heading_line, content_end_line,
                    subtree_end_line, scheduled, deadline, closed
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    file_id,
                    idx as i64,
                    entry.level as i64,
                    entry.keyword.as_deref(),
                    priority_str.as_deref(),
                    &entry.title,
                    org_id.as_deref(),
                    custom_id.as_deref(),
                    entry.heading_line as i64,
                    entry.content_end_line as i64,
                    entry.subtree_end_line as i64,
                    scheduled_str.as_deref(),
                    deadline_str.as_deref(),
                    closed_str.as_deref(),
                ],
            )?;

            let entry_id: i64 = tx.last_insert_rowid();

            // Insert tags
            for tag in &entry.tags {
                tx.execute(
                    "INSERT OR IGNORE INTO tags (entry_id, tag) VALUES (?1, ?2)",
                    params![entry_id, tag],
                )?;
            }

            // Insert dependencies (blockers / triggers)
            if let Some(blocker) = entry.properties.get("BLOCKER") {
                for token in blocker.split_whitespace() {
                    let cleaned = token.trim_matches('"');
                    if !cleaned.is_empty() && !cleaned.contains('(') {
                        tx.execute(
                            "INSERT OR IGNORE INTO dependencies (source_entry_id, target_org_id, dep_kind)
                             VALUES (?1, ?2, 'blocker')",
                            params![entry_id, cleaned],
                        )?;
                    }
                }
            }
            if let Some(trigger) = entry.properties.get("TRIGGER") {
                for token in trigger.split_whitespace() {
                    let cleaned = token.trim_matches('"');
                    if !cleaned.is_empty() && !cleaned.contains('(') {
                        tx.execute(
                            "INSERT OR IGNORE INTO dependencies (source_entry_id, target_org_id, dep_kind)
                             VALUES (?1, ?2, 'trigger')",
                            params![entry_id, cleaned],
                        )?;
                    }
                }
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Removes a file and all its entries from the cache.
    pub fn remove_file(&mut self, path: &Path) -> Result<bool, CacheError> {
        let path_str = path.to_string_lossy().to_string();
        let count = self
            .conn
            .execute("DELETE FROM files WHERE path = ?1", params![&path_str])?;
        Ok(count > 0)
    }

    /// Prunes files from the cache that are no longer in `active_paths`.
    pub fn prune_deleted_files(&mut self, active_paths: &[PathBuf]) -> Result<usize, CacheError> {
        let active_set: std::collections::HashSet<String> = active_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        let mut stmt = self.conn.prepare("SELECT id, path FROM files")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut to_delete = Vec::new();
        for row in rows {
            let (id, path) = row?;
            if !active_set.contains(&path) {
                to_delete.push(id);
            }
        }
        drop(stmt);

        let count = to_delete.len();
        if count > 0 {
            let tx = self.conn.transaction()?;
            for id in to_delete {
                tx.execute("DELETE FROM files WHERE id = ?1", params![id])?;
            }
            tx.commit()?;
        }

        Ok(count)
    }

    /// Looks up an entry by its `:ID:` property.
    pub fn find_id(&self, id: &str) -> Result<Option<CachedEntry>, CacheError> {
        self.query_single_entry(
            "SELECT f.path, e.id, e.entry_idx, e.level, e.keyword, e.priority, e.title,
                    e.org_id, e.custom_id, e.heading_line, e.content_end_line,
                    e.subtree_end_line, e.scheduled, e.deadline, e.closed
             FROM entries e
             JOIN files f ON f.id = e.file_id
             WHERE e.org_id = ?1",
            params![id],
        )
    }

    /// Looks up an entry by its `:CUSTOM_ID:` property.
    pub fn find_custom_id(&self, custom_id: &str) -> Result<Option<CachedEntry>, CacheError> {
        self.query_single_entry(
            "SELECT f.path, e.id, e.entry_idx, e.level, e.keyword, e.priority, e.title,
                    e.org_id, e.custom_id, e.heading_line, e.content_end_line,
                    e.subtree_end_line, e.scheduled, e.deadline, e.closed
             FROM entries e
             JOIN files f ON f.id = e.file_id
             WHERE e.custom_id = ?1",
            params![custom_id],
        )
    }

    fn query_single_entry(&self, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Option<CachedEntry>, CacheError> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;

        if let Some(row) = rows.next()? {
            let file_path: String = row.get(0)?;
            let entry_id: i64 = row.get(1)?;
            let entry_idx: i64 = row.get(2)?;
            let level: i64 = row.get(3)?;
            let keyword: Option<String> = row.get(4)?;
            let priority_str: Option<String> = row.get(5)?;
            let title: String = row.get(6)?;
            let org_id: Option<String> = row.get(7)?;
            let custom_id: Option<String> = row.get(8)?;
            let heading_line: i64 = row.get(9)?;
            let content_end_line: i64 = row.get(10)?;
            let subtree_end_line: i64 = row.get(11)?;
            let scheduled: Option<String> = row.get(12)?;
            let deadline: Option<String> = row.get(13)?;
            let closed: Option<String> = row.get(14)?;

            let priority = priority_str.and_then(|s| s.chars().next());

            let mut tag_stmt = self.conn.prepare("SELECT tag FROM tags WHERE entry_id = ?1")?;
            let tag_rows = tag_stmt.query_map(params![entry_id], |r| r.get::<_, String>(0))?;
            let mut tags = Vec::new();
            for t in tag_rows {
                tags.push(t?);
            }

            Ok(Some(CachedEntry {
                file_path,
                entry_idx: entry_idx as usize,
                level: level as usize,
                keyword,
                priority,
                title,
                org_id,
                custom_id,
                heading_line: heading_line as usize,
                content_end_line: content_end_line as usize,
                subtree_end_line: subtree_end_line as usize,
                scheduled,
                deadline,
                closed,
                tags,
            }))
        } else {
            Ok(None)
        }
    }

    /// Retrieves statistics about the database.
    pub fn stats(&self) -> Result<CacheStats, CacheError> {
        let file_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        let entry_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;
        let tag_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
        let dependency_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM dependencies", [], |r| r.get(0))?;
        let schema_version: i32 = self.conn.query_row("PRAGMA user_version;", [], |r| r.get(0))?;

        Ok(CacheStats {
            file_count: file_count as usize,
            entry_count: entry_count as usize,
            tag_count: tag_count as usize,
            dependency_count: dependency_count as usize,
            schema_version,
        })
    }

    /// Completely clears the cache tables.
    pub fn clear(&mut self) -> Result<(), CacheError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM files;", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Gets the database file path if backed by disk.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

/// Resolves the default cache file path for a workspace.
pub fn default_cache_path(workspace_root: Option<&Path>) -> PathBuf {
    if let Some(root) = workspace_root {
        let local_cache = root.join(".org-cache.db");
        if local_cache.exists() {
            return local_cache;
        }
    }

    if let Ok(xdg_cache) = std::env::var("XDG_CACHE_HOME") {
        PathBuf::from(xdg_cache).join("org-tools").join("cache.db")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".cache")
            .join("org-tools")
            .join("cache.db")
    } else if let Some(root) = workspace_root {
        root.join(".org-cache.db")
    } else {
        PathBuf::from(".org-cache.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_open_in_memory_and_schema_version() {
        let db = CacheDb::open_in_memory().unwrap();
        let stats = db.stats().unwrap();
        assert_eq!(stats.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(stats.file_count, 0);
        assert_eq!(stats.entry_count, 0);
    }

    #[test]
    fn test_sync_files_and_id_lookup() {
        let mut db = CacheDb::open_in_memory().unwrap();
        let dir = tempdir().unwrap();
        let file1 = dir.path().join("tasks.org");
        let content1 = "* TODO Buy groceries :errand:\n:PROPERTIES:\n:ID: task-123\n:END:\n\n* DONE Clean kitchen\n:PROPERTIES:\n:CUSTOM_ID: clean-kitchen\n:END:\n";
        fs::write(&file1, content1).unwrap();

        let stats = db.sync_files(std::slice::from_ref(&file1)).unwrap();
        assert_eq!(stats.total_files, 1);
        assert_eq!(stats.updated_files, 1);
        assert_eq!(stats.cache_hits, 0);
        assert_eq!(stats.total_entries, 2);

        // Cache hit on re-sync
        let stats2 = db.sync_files(std::slice::from_ref(&file1)).unwrap();
        assert_eq!(stats2.cache_hits, 1);
        assert_eq!(stats2.updated_files, 0);

        // Find by ID
        let found_id = db.find_id("task-123").unwrap().expect("should find task-123");
        assert_eq!(found_id.title, "Buy groceries");
        assert_eq!(found_id.keyword.as_deref(), Some("TODO"));
        assert_eq!(found_id.tags, vec!["errand"]);

        // Find by CUSTOM_ID
        let found_custom = db.find_custom_id("clean-kitchen").unwrap().expect("should find clean-kitchen");
        assert_eq!(found_custom.title, "Clean kitchen");
        assert_eq!(found_custom.keyword.as_deref(), Some("DONE"));

        // Miss
        assert!(db.find_id("non-existent").unwrap().is_none());
    }

    #[test]
    fn test_sync_updates_on_file_modification() {
        let mut db = CacheDb::open_in_memory().unwrap();
        let dir = tempdir().unwrap();
        let file1 = dir.path().join("notes.org");
        fs::write(&file1, "* Heading 1\n:PROPERTIES:\n:ID: id-1\n:END:\n").unwrap();

        db.sync_files(std::slice::from_ref(&file1)).unwrap();
        assert_eq!(db.find_id("id-1").unwrap().unwrap().title, "Heading 1");

        // Modify content
        fs::write(&file1, "* Heading Renamed\n:PROPERTIES:\n:ID: id-1\n:END:\n").unwrap();
        let stats = db.sync_files(std::slice::from_ref(&file1)).unwrap();
        assert_eq!(stats.updated_files, 1);
        assert_eq!(db.find_id("id-1").unwrap().unwrap().title, "Heading Renamed");
    }

    #[test]
    fn test_prune_deleted_files() {
        let mut db = CacheDb::open_in_memory().unwrap();
        let dir = tempdir().unwrap();
        let file1 = dir.path().join("a.org");
        let file2 = dir.path().join("b.org");
        fs::write(&file1, "* Task A\n:PROPERTIES:\n:ID: id-a\n:END:\n").unwrap();
        fs::write(&file2, "* Task B\n:PROPERTIES:\n:ID: id-b\n:END:\n").unwrap();

        db.sync_files(&[file1.clone(), file2.clone()]).unwrap();
        assert_eq!(db.stats().unwrap().file_count, 2);

        // Re-sync with only file1
        let stats = db.sync_files(std::slice::from_ref(&file1)).unwrap();
        assert_eq!(stats.deleted_files, 1);
        assert_eq!(db.stats().unwrap().file_count, 1);
        assert!(db.find_id("id-b").unwrap().is_none());
        assert!(db.find_id("id-a").unwrap().is_some());
    }

    #[test]
    fn test_fallback_to_recreate_on_corrupt_or_incompatible_db() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test_cache.db");

        // Create corrupt garbage file
        fs::write(&db_file, b"NOT_A_VALID_SQLITE_FILE_CORRUPT_DATA").unwrap();

        // open_or_create should detect corruption, recreate, and succeed!
        let db = CacheDb::open_or_create(&db_file).expect("should recreate corrupt db cleanly");
        let stats = db.stats().unwrap();
        assert_eq!(stats.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn test_fallback_to_recreate_on_incompatible_newer_version() {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test_future_cache.db");

        {
            let conn = Connection::open(&db_file).unwrap();
            conn.execute_batch("PRAGMA user_version = 999;").unwrap();
        }

        // open_or_create should detect version 999 > CURRENT_SCHEMA_VERSION, recreate, and succeed!
        let db = CacheDb::open_or_create(&db_file).expect("should recreate future db cleanly");
        let stats = db.stats().unwrap();
        assert_eq!(stats.schema_version, CURRENT_SCHEMA_VERSION);
    }
}
