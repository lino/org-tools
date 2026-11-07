// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Incremental `(mtime_ns, byte_len, sha256)` cursor diffing and SQLite synchronization.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use rusqlite::{params, Connection};

use crate::cache::payload::{build_file_payload, commit_batch, FileTouch};
use crate::cache::schema::clear_all_rows;
use crate::cache::sha256::sha256_hex;
use crate::cache::walk::{compute_canon_key, discover_org_files, DiskOrgFile};
use crate::cache::CacheTelemetry;
use crate::export_md::normalize_workspace_path;

/// Sorted file-table diff tuple `(unchanged_hits, added_indices, changed_indices, deleted_rel_paths)`.
type FileDiffPlan = (usize, Vec<usize>, Vec<usize>, Vec<String>);

/// Compares one `(disk, stored)` pair and returns whether `(disk, stored)` cursors should advance.
#[allow(clippy::too_many_arguments)]
fn compare_file_pair(
    workspace_root: &Path,
    df: &DiskOrgFile,
    row: &rusqlite::Row<'_>,
    d_idx: usize,
    hits: &mut usize,
    added: &mut Vec<usize>,
    changed: &mut Vec<usize>,
    deleted: &mut Vec<String>,
) -> Result<(bool, bool), String> {
    let s_rel = row
        .get_ref(0)
        .map_err(|e| e.to_string())?
        .as_str()
        .map_err(|e| e.to_string())?;
    match df.rel_path.as_str().cmp(s_rel) {
        Ordering::Equal => {
            let s_mtime: i64 = row.get(1).map_err(|e| e.to_string())?;
            let s_len: i64 = row.get(2).map_err(|e| e.to_string())?;
            let s_sym: i64 = row.get(3).map_err(|e| e.to_string())?;
            let sym_ok = if !df.is_symlink && s_sym == 0 {
                true
            } else {
                let s_canon = row
                    .get_ref(4)
                    .map_err(|e| e.to_string())?
                    .as_str()
                    .map_err(|e| e.to_string())?;
                s_sym == i64::from(df.is_symlink)
                    && s_canon == compute_canon_key(workspace_root, df)
            };
            if s_mtime == df.mtime_ns && s_len == df.byte_len && sym_ok {
                *hits += 1;
            } else {
                changed.push(d_idx);
            }
            Ok((true, true))
        }
        Ordering::Less => {
            added.push(d_idx);
            Ok((true, false))
        }
        Ordering::Greater => {
            deleted.push(s_rel.to_string());
            Ok((false, true))
        }
    }
}

/// Compares sorted `disk_files` against the `files` PK index without allocating strings on warm hits.
fn diff_sorted_files(
    conn: &Connection,
    workspace_root: &Path,
    disk_files: &[DiskOrgFile],
) -> Result<FileDiffPlan, String> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT rel_path, mtime_ns, byte_len, is_symlink, canon_key
             FROM files ORDER BY rel_path ASC",
        )
        .map_err(|e| format!("failed to prepare files scan: {e}"))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| format!("failed to query files table: {e}"))?;

    let (mut hits, mut added, mut changed, mut deleted) = (0, Vec::new(), Vec::new(), Vec::new());
    let mut d_idx = 0usize;
    let mut cur_row = rows.next().map_err(|e| format!("files row error: {e}"))?;

    while let (Some(df), Some(row)) = (disk_files.get(d_idx), &cur_row) {
        let (adv_disk, adv_db) = compare_file_pair(
            workspace_root,
            df,
            row,
            d_idx,
            &mut hits,
            &mut added,
            &mut changed,
            &mut deleted,
        )?;
        if adv_disk {
            d_idx += 1;
        }
        if adv_db {
            cur_row = rows.next().map_err(|e| format!("files row error: {e}"))?;
        }
    }
    added.extend(d_idx..disk_files.len());
    while let Some(row) = cur_row {
        let s_rel = row
            .get_ref(0)
            .map_err(|e| e.to_string())?
            .as_str()
            .map_err(|e| e.to_string())?;
        deleted.push(s_rel.to_string());
        cur_row = rows.next().map_err(|e| format!("files row error: {e}"))?;
    }
    Ok((hits, added, changed, deleted))
}

/// Fetches stored `content_hash` values for candidate modified `rel_path` rows.
fn load_stored_hashes(
    conn: &Connection,
    disk_files: &[DiskOrgFile],
    changed_indices: &[usize],
) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::with_capacity(changed_indices.len());
    if changed_indices.is_empty() {
        return Ok(map);
    }
    let mut stmt = conn
        .prepare_cached("SELECT content_hash FROM files WHERE rel_path = ?1")
        .map_err(|e| format!("failed to prepare hash lookup: {e}"))?;
    for &idx in changed_indices {
        let rel = &disk_files[idx].rel_path;
        if let Ok(hash) = stmt.query_row(params![rel], |r| r.get::<_, String>(0)) {
            map.insert(rel.clone(), hash);
        }
    }
    Ok(map)
}

/// Reads `df` bytes from `workspace_root` and computes its lowercase SHA-256 hex digest.
fn read_and_hash_disk_file(
    workspace_root: &Path,
    df: &DiskOrgFile,
) -> Result<(Vec<u8>, String), String> {
    let disk_path = workspace_root.join(&df.rel_path);
    let bytes =
        fs::read(&disk_path).map_err(|e| format!("error reading {}: {e}", disk_path.display()))?;
    let hash = sha256_hex(&bytes);
    Ok((bytes, hash))
}

/// Synchronizes `conn` against pre-discovered `disk_files`, returning telemetry and missed `rel_path`s.
pub(crate) fn sync_discovered_files(
    conn: &mut Connection,
    workspace_root: &Path,
    disk_files: &[DiskOrgFile],
    started: Instant,
) -> Result<(CacheTelemetry, Vec<String>), String> {
    let (mut hits, added, changed, deleted) = diff_sorted_files(conn, workspace_root, disk_files)?;
    if added.is_empty() && changed.is_empty() && deleted.is_empty() {
        return Ok((
            CacheTelemetry {
                duration_us: started.elapsed().as_micros() as u64,
                files_scanned: disk_files.len(),
                cache_hits: hits,
                cache_misses: 0,
            },
            Vec::new(),
        ));
    }
    let old_hashes = load_stored_hashes(conn, disk_files, &changed)?;
    let (mut touches, mut upserts, mut misses) = (
        Vec::new(),
        Vec::with_capacity(added.len() + changed.len()),
        0,
    );
    for idx in changed {
        let df = disk_files[idx].clone();
        let (bytes, hash) = read_and_hash_disk_file(workspace_root, &df)?;
        if old_hashes.get(&df.rel_path) == Some(&hash) {
            hits += 1;
            let canon_key = compute_canon_key(workspace_root, &df);
            touches.push(FileTouch {
                rel_path: df.rel_path,
                mtime_ns: df.mtime_ns,
                byte_len: df.byte_len,
                is_symlink: df.is_symlink,
                canon_key,
            });
        } else {
            misses += 1;
            upserts.push(build_file_payload(workspace_root, df, bytes, hash, true));
        }
    }
    for idx in added {
        let df = disk_files[idx].clone();
        let (bytes, hash) = read_and_hash_disk_file(workspace_root, &df)?;
        misses += 1;
        upserts.push(build_file_payload(workspace_root, df, bytes, hash, false));
    }

    let missed_rels: Vec<String> = upserts.iter().map(|u| u.disk.rel_path.clone()).collect();
    commit_batch(conn, &deleted, &touches, &upserts)?;
    Ok((
        CacheTelemetry {
            duration_us: started.elapsed().as_micros() as u64,
            files_scanned: disk_files.len(),
            cache_hits: hits,
            cache_misses: misses,
        },
        missed_rels,
    ))
}

/// Synchronizes `conn` with `workspace_root` incrementally (or completely when `full_rebuild` is `true`).
pub(crate) fn sync_workspace(
    conn: &mut Connection,
    workspace_root: &Path,
    full_rebuild: bool,
) -> Result<CacheTelemetry, String> {
    let started = Instant::now();
    if full_rebuild {
        clear_all_rows(conn)?;
    }
    let disk_files = discover_org_files(workspace_root)?;
    sync_discovered_files(conn, workspace_root, &disk_files, started).map(|(tel, _)| tel)
}

/// Converts `path` into a forward-slash relative path under `workspace_root` when possible.
fn slash_rel_under_workspace(workspace_root: &Path, path: &Path) -> Option<String> {
    let norm_path = normalize_workspace_path(path);
    let rel = if path.is_relative() {
        path
    } else if let Ok(r) = path.strip_prefix(workspace_root) {
        r
    } else {
        norm_path.strip_prefix(workspace_root).ok()?
    };
    let mut parts = Vec::new();
    for comp in rel.components() {
        if let Component::Normal(seg) = comp {
            parts.push(seg.to_str()?);
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Marks `paths` with `mtime_ns = -1` in `files` so `refresh()` re-hashes them on coarse-tick FS.
pub(crate) fn invalidate_cached_paths(
    conn: &Connection,
    workspace_root: &Path,
    paths: &[PathBuf],
) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare_cached("UPDATE files SET mtime_ns = -1 WHERE rel_path = ?1 OR canon_key = ?2")
        .map_err(|e| format!("failed to prepare cache invalidation: {e}"))?;
    for path in paths {
        let joined = if path.is_relative() {
            workspace_root.join(path)
        } else {
            path.clone()
        };
        let canon = normalize_workspace_path(&joined)
            .to_string_lossy()
            .into_owned();
        let rel = slash_rel_under_workspace(workspace_root, path).unwrap_or_default();
        stmt.execute(params![rel, canon])
            .map_err(|e| format!("failed to invalidate {}: {e}", path.display()))?;
    }
    Ok(())
}
