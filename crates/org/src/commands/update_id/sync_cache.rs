// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Inline write-through synchronization for mutated workspace files.

use std::path::{Path, PathBuf};

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::export_md::normalize_workspace_path;

/// Discovers the nearest enclosing workspace root for `file`.
fn discover_enclosing_root(file: &Path) -> Option<(PathBuf, bool)> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let norm_cwd = normalize_workspace_path(&cwd);
    let norm_file = normalize_workspace_path(file);

    // 1. Ancestor containing .cache/org-cache.db
    for ancestor in norm_file.ancestors().skip(1) {
        if WorkspaceCache::resolve_db_path(ancestor).is_file() {
            return Some((ancestor.to_path_buf(), true));
        }
    }

    // 2. Ancestor containing .git or .org-tools.toml
    for ancestor in norm_file.ancestors().skip(1) {
        if ancestor.join(".git").exists() || ancestor.join(".org-tools.toml").exists() {
            return Some((ancestor.to_path_buf(), false));
        }
    }

    // 3. File inside cwd (e.g. zettel/new_note.org)
    if norm_file.starts_with(&norm_cwd) && norm_cwd != Path::new("/") {
        return Some((norm_cwd, false));
    }

    None
}

/// Synchronizes `written_paths` into `.cache/org-cache.db`, initializing the cache if in a workspace.
pub(crate) fn sync_written_targets(written_paths: &[PathBuf]) -> bool {
    if written_paths.is_empty() {
        return false;
    }

    for path in written_paths {
        if let Some((root, existed)) = discover_enclosing_root(path) {
            if existed {
                if let Ok(synced) = WorkspaceCache::sync_written_files(&root, written_paths) {
                    if synced {
                        return true;
                    }
                }
            } else {
                // Initialize cache at workspace root
                if WorkspaceCache::open_and_sync(&root).is_ok() {
                    return true;
                }
            }
        }
    }

    false
}
