// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fast workspace `.org` directory walker with parallel subdir statting and `.gitignore` fallback.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use ignore::WalkBuilder;

use crate::export_md::normalize_workspace_path;

/// Minimum top-level subdirectory count before fanning out scoped parallel `read_dir` workers.
const PARALLEL_SUBDIR_THRESHOLD: usize = 4;

/// Maximum scoped worker threads used for parallel directory metadata scanning.
const MAX_SCAN_WORKERS: usize = 3;

/// On-disk `.org` file metadata collected during a directory scan.
#[derive(Debug, Clone)]
pub(crate) struct DiskOrgFile {
    /// Forward-slash relative path beneath `workspace_root`.
    pub(crate) rel_path: String,
    /// Last-modified timestamp in nanoseconds since `UNIX_EPOCH`.
    pub(crate) mtime_ns: i64,
    /// File length in bytes.
    pub(crate) byte_len: i64,
    /// Whether this entry on disk is a symbolic link.
    pub(crate) is_symlink: bool,
}

/// Computes the canonical target path string for `df` (`0` syscalls when `!df.is_symlink`).
pub(crate) fn compute_canon_key(workspace_root: &Path, df: &DiskOrgFile) -> String {
    let joined = workspace_root.join(&df.rel_path);
    if df.is_symlink {
        normalize_workspace_path(&joined)
            .to_string_lossy()
            .into_owned()
    } else {
        joined.to_string_lossy().into_owned()
    }
}

/// Extracts `(mtime_ns, byte_len)` from `fs::Metadata`.
fn extract_meta_stamp(meta: &fs::Metadata) -> (i64, i64) {
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    (mtime_ns, meta.len() as i64)
}

/// Formats `rel` as a forward-slash UTF-8 relative path string.
fn rel_path_to_slash(rel: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for comp in rel.components() {
        if let Component::Normal(seg) = comp {
            parts.push(seg.to_str()?);
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Joins `prefix` and `name` with `/` without `std::fmt` overhead.
fn join_rel(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        return name.to_string();
    }
    let mut out = String::with_capacity(prefix.len() + 1 + name.len());
    out.push_str(prefix);
    out.push('/');
    out.push_str(name);
    out
}

/// Classifies a single `fs::DirEntry` during `scan_root_fast`, returning `Ok(false)` on nested VCS/ignore.
fn visit_fast_entry(
    entry: fs::DirEntry,
    prefix: &str,
    subdirs: &mut Vec<(PathBuf, String)>,
    out: &mut Vec<DiskOrgFile>,
) -> Result<bool, String> {
    let os_name = entry.file_name();
    let Some(name) = os_name.to_str() else {
        return Ok(true);
    };
    if name == ".git" || name == ".ignore" {
        return Ok(false);
    }
    if name.starts_with('.') {
        return Ok(true);
    }
    let ft = entry
        .file_type()
        .map_err(|e| format!("file_type error on {}: {e}", entry.path().display()))?;
    if ft.is_dir() {
        subdirs.push((entry.path(), join_rel(prefix, name)));
    } else if name.ends_with(".org") {
        let is_symlink = ft.is_symlink();
        let Some(meta) = (if is_symlink {
            fs::metadata(entry.path()).ok()
        } else {
            entry.metadata().ok()
        }) else {
            return Ok(true);
        };
        if meta.is_file() {
            let (mtime_ns, byte_len) = extract_meta_stamp(&meta);
            out.push(DiskOrgFile {
                rel_path: join_rel(prefix, name),
                mtime_ns,
                byte_len,
                is_symlink,
            });
        }
    }
    Ok(true)
}

/// Recursively scans `dir` sequentially without `.gitignore` rules, returning `Ok(false)` on ignore files.
fn scan_dir_seq(dir: &Path, prefix: &str, out: &mut Vec<DiskOrgFile>) -> Result<bool, String> {
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("failed to read directory {}: {e}", dir.display()))?;
    let mut subdirs = Vec::new();
    for item in entries {
        let entry = item.map_err(|e| format!("directory entry error in {}: {e}", dir.display()))?;
        if !visit_fast_entry(entry, prefix, &mut subdirs, out)? {
            return Ok(false);
        }
    }
    for (sub_path, sub_rel) in subdirs {
        if !scan_dir_seq(&sub_path, &sub_rel, out)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Scans `workspace_root` using scoped parallel workers when `>= 4` subdirectories exist.
fn scan_root_fast(workspace_root: &Path, out: &mut Vec<DiskOrgFile>) -> Result<bool, String> {
    let entries = fs::read_dir(workspace_root)
        .map_err(|e| format!("failed to read directory {}: {e}", workspace_root.display()))?;
    let mut subdirs = Vec::new();
    for item in entries {
        let entry = item.map_err(|e| format!("directory entry error: {e}"))?;
        if !visit_fast_entry(entry, "", &mut subdirs, out)? {
            return Ok(false);
        }
    }
    if subdirs.len() < PARALLEL_SUBDIR_THRESHOLD {
        for (sub_path, sub_rel) in subdirs {
            if !scan_dir_seq(&sub_path, &sub_rel, out)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    let chunk_size = subdirs.len().div_ceil(MAX_SCAN_WORKERS);
    let results: Vec<Result<Option<Vec<DiskOrgFile>>, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = subdirs
            .chunks(chunk_size)
            .map(|chunk| {
                s.spawn(move || {
                    let mut local = Vec::new();
                    for (sub_path, sub_rel) in chunk {
                        if !scan_dir_seq(sub_path, sub_rel, &mut local)? {
                            return Ok(None);
                        }
                    }
                    Ok(Some(local))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for res in results {
        let Some(local) = res? else {
            return Ok(false);
        };
        out.extend(local);
    }
    Ok(true)
}

/// Returns `true` when `workspace_root` or any ancestor directory contains `.git` or `.ignore`.
fn has_ancestor_vcs_or_ignore(workspace_root: &Path) -> bool {
    workspace_root
        .ancestors()
        .any(|dir| dir.join(".git").exists() || dir.join(".ignore").exists())
}

/// Walks `workspace_root` via `WalkBuilder::new` matching `files::collect_org_files`, skipping `.cache`.
fn scan_dir_with_ignore(workspace_root: &Path) -> Result<Vec<DiskOrgFile>, String> {
    let mut builder = WalkBuilder::new(workspace_root);
    builder.filter_entry(|e| e.depth() == 0 || e.file_name() != ".cache");
    let mut out = Vec::new();
    for entry in builder.build().flatten() {
        let path = entry.path();
        if path == workspace_root || !path.extension().is_some_and(|e| e == "org") {
            continue;
        }
        let is_symlink = entry.path_is_symlink();
        let Some(meta) = (if is_symlink {
            fs::metadata(path).ok()
        } else {
            entry.metadata().ok()
        }) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(rel) = path.strip_prefix(workspace_root) else {
            continue;
        };
        if let Some(rel_path) = rel_path_to_slash(rel) {
            let (mtime_ns, byte_len) = extract_meta_stamp(&meta);
            out.push(DiskOrgFile {
                rel_path,
                mtime_ns,
                byte_len,
                is_symlink,
            });
        }
    }
    Ok(out)
}

/// Discovers all `.org` files under `workspace_root` sorted by `rel_path`.
pub(crate) fn discover_org_files(workspace_root: &Path) -> Result<Vec<DiskOrgFile>, String> {
    let mut files = Vec::new();
    if has_ancestor_vcs_or_ignore(workspace_root) || !scan_root_fast(workspace_root, &mut files)? {
        files = scan_dir_with_ignore(workspace_root)?;
    }
    files.sort_unstable_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(files)
}
