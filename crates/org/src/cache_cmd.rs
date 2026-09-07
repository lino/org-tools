// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommands for managing the persistent SQLite cache.

use std::path::{Path, PathBuf};
use std::time::Instant;

use org_tools_core::cache::{default_cache_path, CacheDb};
use org_tools_core::files::collect_org_files;

/// Synchronizes the SQLite cache immediately with workspace files.
pub fn run_sync(paths: &[PathBuf], cache_path: Option<&Path>, reindex: bool, clear: bool) -> i32 {
    let resolved_path = cache_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| default_cache_path(Some(Path::new("."))));

    let mut db = match CacheDb::open_or_create(&resolved_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("org: failed to open SQLite cache: {e}");
            return 2;
        }
    };

    if clear {
        if let Err(e) = db.clear() {
            eprintln!("org: failed to clear cache: {e}");
            return 2;
        }
        println!("Cleared SQLite cache at: {}", resolved_path.display());
        return 0;
    }

    if reindex {
        if let Err(e) = db.clear() {
            eprintln!("org: failed to clear cache for reindexing: {e}");
            return 2;
        }
    }

    let files = collect_org_files(paths);
    let start = Instant::now();
    let stats = match db.sync_files(&files) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("org: cache sync failed: {e}");
            return 2;
        }
    };
    let elapsed = start.elapsed();

    println!("SQLite cache synchronized in {:.2?}:", elapsed);
    println!("  Files scanned:   {}", stats.total_files);
    println!("  Cache hits:      {}", stats.cache_hits);
    println!("  Files updated:   {}", stats.updated_files);
    println!("  Files pruned:    {}", stats.deleted_files);
    println!("  Total entries:   {}", stats.total_entries);
    println!("Cache location:    {}", resolved_path.display());

    0
}
