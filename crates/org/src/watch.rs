// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! File watcher for continuous SQLite cache synchronization.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use org_tools_core::cache::{default_cache_path, CacheDb};
use org_tools_core::files::collect_org_files;

/// Watches given paths and continuously updates the SQLite cache.
pub fn run_watch(
    paths: &[PathBuf],
    cache_path: Option<&Path>,
    initial_sync: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved_cache_path = cache_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| default_cache_path(Some(Path::new("."))));

    println!(
        "[watch] Opening SQLite cache at: {}",
        resolved_cache_path.display()
    );
    let mut db = CacheDb::open_or_create(&resolved_cache_path)?;

    if initial_sync {
        let files = collect_org_files(paths);
        let start = Instant::now();
        let stats = db.sync_files(&files)?;
        println!(
            "[watch] Initial sync complete in {:.2?}: {} files ({} updated, {} entries)",
            start.elapsed(),
            stats.total_files,
            stats.updated_files,
            stats.total_entries
        );
    }

    let (tx, rx) = channel();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        Config::default(),
    )?;

    for p in paths {
        let watch_target = if p.is_dir() {
            p
        } else if let Some(parent) = p.parent() {
            if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            }
        } else {
            Path::new(".")
        };
        watcher.watch(watch_target, RecursiveMode::Recursive)?;
        println!("[watch] Watching {} for changes...", watch_target.display());
    }

    println!("[watch] Monitoring org files. Press Ctrl-C to stop.");

    // Event loop with debouncing
    let debounce_duration = Duration::from_millis(200);

    loop {
        // Wait for first event
        let first_event = match rx.recv() {
            Ok(Ok(event)) => event,
            Ok(Err(e)) => {
                eprintln!("[watch] Watch error: {e}");
                continue;
            }
            Err(_) => break, // Channel closed
        };

        // Collect any burst events within debounce window
        let mut dirty_paths = HashSet::new();
        let deadline = Instant::now() + debounce_duration;

        fn handle_event(event: Event, dirty_paths: &mut HashSet<PathBuf>) {
            match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                    for path in event.paths {
                        if path.extension().is_some_and(|ext| ext == "org") {
                            dirty_paths.insert(path);
                        }
                    }
                }
                _ => {}
            }
        }

        handle_event(first_event, &mut dirty_paths);

        while let Some(timeout) = deadline.checked_duration_since(Instant::now()) {
            if timeout.is_zero() {
                break;
            }
            match rx.recv_timeout(timeout) {
                Ok(Ok(event)) => handle_event(event, &mut dirty_paths),
                Ok(Err(e)) => eprintln!("[watch] Watch error: {e}"),
                Err(_) => break,
            }
        }

        if !dirty_paths.is_empty() {
            let mut updated = 0;
            let mut removed = 0;
            for path in &dirty_paths {
                if path.exists() {
                    let files = std::slice::from_ref(path);
                    if let Ok(stats) = db.sync_files(files) {
                        if stats.updated_files > 0 {
                            updated += 1;
                        }
                    }
                } else if let Ok(true) = db.remove_file(path) {
                    removed += 1;
                }
            }
            if updated > 0 || removed > 0 {
                println!(
                    "[watch] Synced cache: {} updated, {} removed",
                    updated, removed
                );
            }
        }
    }

    Ok(())
}
