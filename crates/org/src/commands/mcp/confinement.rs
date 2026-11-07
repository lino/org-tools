// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Workspace boundary confinement, symlink escape guards, and `.org` locator rewriting.

use std::fs;
use std::path::{Component, Path, PathBuf};

use org_tools_core::export_md::{normalize_workspace_path, WORKSPACE_ENV_VAR};
use org_tools_core::locator::OrgLocator;

/// Ancestor markers inspected when discovering a default workspace root from `cwd`.
const WORKSPACE_MARKERS: &[&str] = &[".git", "kb.toml", ".org-tools.toml", ".fritz.toml"];

/// Discovers a default workspace directory via `--workspace`, `ORG_TOOLS_WORKSPACE`, or markers.
pub(super) fn resolve_startup_workspace(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(ws) = explicit {
        if !ws.is_dir() {
            return Err(format!("workspace directory not found: {}", ws.display()));
        }
        return Ok(ws.to_path_buf());
    }
    if let Some(env_ws) = std::env::var_os(WORKSPACE_ENV_VAR).filter(|v| !v.is_empty()) {
        let env_path = PathBuf::from(env_ws);
        if !env_path.is_dir() {
            return Err(format!(
                "workspace directory from {WORKSPACE_ENV_VAR} not found: {}",
                env_path.display()
            ));
        }
        return Ok(env_path);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let norm = normalize_workspace_path(&cwd);
    let mut cur: Option<&Path> = Some(&norm);
    while let Some(dir) = cur {
        if WORKSPACE_MARKERS.iter().any(|m| dir.join(m).exists()) {
            return Ok(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    Ok(cwd)
}

/// Returns `true` when `path` carries a case-insensitive `.org` file extension.
pub(super) fn is_org_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("org"))
}

/// Resolves the canonical filesystem root for `norm_ws`.
pub(super) fn canonical_workspace_root(norm_ws: &Path) -> Result<PathBuf, String> {
    norm_ws.canonicalize().map_err(|e| {
        format!(
            "failed to canonicalize workspace {}: {e}",
            norm_ws.display()
        )
    })
}

/// Verifies that existing `candidate` is an `.org` file whose canonical target stays under `canon_ws`.
pub(super) fn verify_existing_confined_org_file(
    candidate: &Path,
    canon_ws: &Path,
    label: &str,
) -> Result<PathBuf, String> {
    if !is_org_extension(candidate) {
        return Err(format!("only .org files are supported: {label}"));
    }
    if !candidate.is_file() {
        return Err(format!("file not found: {label}"));
    }
    let file_canon = candidate
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize {label}: {e}"))?;
    if !file_canon.starts_with(canon_ws) {
        return Err(format!("path escapes workspace boundary: {label}"));
    }
    if !is_org_extension(&file_canon) {
        return Err(format!("symlink target is not an .org file: {label}"));
    }
    Ok(file_canon)
}

/// Resolves `raw_path` relative to `display_ws` and confines its canonical target within `canon_ws`.
pub(super) fn resolve_confined_org_path(
    raw_path: &str,
    display_ws: &Path,
    canon_ws: &Path,
) -> Result<PathBuf, String> {
    let input = Path::new(raw_path);
    if input
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(format!(
            "path traversal outside workspace is not allowed: {raw_path}"
        ));
    }
    if !is_org_extension(input) {
        return Err(format!("only .org files are supported: {raw_path}"));
    }
    let candidate = if input.is_absolute() {
        if !normalize_workspace_path(input).starts_with(canon_ws) {
            return Err(format!("path escapes workspace boundary: {raw_path}"));
        }
        input.to_path_buf()
    } else {
        display_ws.join(input)
    };
    verify_existing_confined_org_file(&candidate, canon_ws, raw_path)?;
    Ok(candidate)
}

/// Rewrites any file-scoped [`OrgLocator`] relative to `display_ws` after enforcing `canon_ws` confinement.
pub(super) fn confine_and_rewrite_locator(
    locator: &OrgLocator,
    display_ws: &Path,
    canon_ws: &Path,
) -> Result<OrgLocator, String> {
    match locator {
        OrgLocator::Id(id) => Ok(OrgLocator::Id(id.clone())),
        OrgLocator::CustomId { file, custom_id } => {
            let resolved =
                resolve_confined_org_path(&file.to_string_lossy(), display_ws, canon_ws)?;
            Ok(OrgLocator::CustomId {
                file: resolved,
                custom_id: custom_id.clone(),
            })
        }
        OrgLocator::OutlinePath { file, path } => {
            let resolved =
                resolve_confined_org_path(&file.to_string_lossy(), display_ws, canon_ws)?;
            Ok(OrgLocator::OutlinePath {
                file: resolved,
                path: path.clone(),
            })
        }
        OrgLocator::LineRef { file, line } => {
            let resolved =
                resolve_confined_org_path(&file.to_string_lossy(), display_ws, canon_ws)?;
            Ok(OrgLocator::LineRef {
                file: resolved,
                line: *line,
            })
        }
    }
}

/// Verifies a write/cascade target path (existing or newly created archive `.org` file) stays inside `canon_ws`.
pub(super) fn verify_confined_mutation_target(
    path: &Path,
    canon_ws: &Path,
) -> Result<PathBuf, String> {
    let label = path.display().to_string();
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!(
            "mutation target traversal outside workspace is not allowed: {label}"
        ));
    }
    if !is_org_extension(path) {
        return Err(format!(
            "mutation target must be an .org file, got: {label}"
        ));
    }
    if path.exists() || fs::symlink_metadata(path).is_ok() {
        return verify_existing_confined_org_file(path, canon_ws, &label);
    }
    let Some(existing_anc) = path.ancestors().skip(1).find(|anc| anc.exists()) else {
        return Err(format!("mutation target has no existing parent: {label}"));
    };
    let anc_canon = existing_anc
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize parent of {label}: {e}"))?;
    if !anc_canon.starts_with(canon_ws) {
        return Err(format!(
            "mutation target escapes workspace boundary: {label}"
        ));
    }
    Ok(normalize_workspace_path(path))
}
