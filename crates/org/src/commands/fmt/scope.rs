// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scope and workspace ID index resolution for `org fmt` (`ORGFMT-WP-39`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use clap::Args;

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::config::{Config, IdScope};
use org_tools_core::document::OrgDocument;
use org_tools_core::export_md::normalize_workspace_path;
use org_tools_core::files::collect_org_files;
use org_tools_core::source::SourceFile;

/// Ancestor markers inspected when discovering a default workspace or vault root.
const VAULT_MARKERS: &[&str] = &[".org-tools.toml", "kb.toml", ".git", ".cache/org-cache.db"];

/// CLI arguments configuring resolution scope and explicit workspace / roam paths.
#[derive(Debug, Clone, Default, Args)]
pub(crate) struct ScopeArgs {
    /// Scope for ID link and duplicate resolution (vault, roam-dir, dir, file).
    #[arg(long, value_enum)]
    pub(crate) id_scope: Option<IdScope>,

    /// Explicit workspace / vault root directory override.
    #[arg(long, value_hint = clap::ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,

    /// Explicit org-roam directory name or path.
    #[arg(long, action = clap::ArgAction::Append)]
    pub(crate) roam_dir: Vec<String>,
}

/// Resolved ID and document scope context for linting and formatting.
pub(crate) struct ResolvedScope {
    /// The effective [`IdScope`] applied to this run.
    pub(crate) effective_scope: IdScope,
    /// Known `:ID:` values across the resolved scope (lowercased and original).
    pub(crate) scoped_ids: HashSet<String>,
    /// Map from lowercased `:ID:` to list of file paths declaring that `:ID:`.
    pub(crate) id_locations: HashMap<String, Vec<PathBuf>>,
    /// Pre-parsed documents for target files.
    pub(crate) workspace_docs: Vec<OrgDocument>,
}

/// Discovers the root directory of the vault containing `target_file`.
fn discover_vault_root(target_file: &Path, config: &Config) -> Option<PathBuf> {
    let norm = normalize_workspace_path(target_file);
    for ancestor in norm.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            break;
        }
        if VAULT_MARKERS.iter().any(|m| ancestor.join(m).exists()) {
            return Some(ancestor.to_path_buf());
        }
    }
    config
        .roam_directories()
        .into_iter()
        .find(|(_, roam_path)| norm.starts_with(roam_path))
        .map(|(_, roam_path)| roam_path)
}

/// Indexes all `:ID:` candidates from a cache into `scoped_ids` and `id_locations`.
fn index_cache_candidates(
    root: &Path,
    scoped_ids: &mut HashSet<String>,
    id_locations: &mut HashMap<String, Vec<PathBuf>>,
) {
    if let Ok((cache, _)) = WorkspaceCache::open_and_sync(root) {
        if let Ok(candidates) = cache.complete_ids("") {
            for cand in candidates {
                let norm = cand.id.to_ascii_lowercase();
                scoped_ids.insert(norm.clone());
                scoped_ids.insert(cand.id.clone());
                let full_path = root.join(&cand.rel_path);
                id_locations.entry(norm).or_default().push(full_path);
            }
        }
    }
}

/// Resolves IDs for [`IdScope::Vault`] against the workspace index and roam directories.
fn resolve_vault_scope(
    files: &[PathBuf],
    scope_args: &ScopeArgs,
    config: &Config,
    scoped_ids: &mut HashSet<String>,
    id_locations: &mut HashMap<String, Vec<PathBuf>>,
) -> bool {
    let root = if let Some(ref ws) = scope_args.workspace {
        Some(ws.clone())
    } else if let Some(first) = files.first() {
        discover_vault_root(first, config)
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        discover_vault_root(&cwd, config)
    };

    let Some(vault_root) = root else {
        return false;
    };

    index_cache_candidates(&vault_root, scoped_ids, id_locations);
    for (_, roam_path) in config.roam_directories() {
        if !roam_path.starts_with(&vault_root) {
            index_cache_candidates(&roam_path, scoped_ids, id_locations);
        }
    }
    true
}

/// Resolves IDs for [`IdScope::RoamDir`] against explicit or owning roam directories.
fn resolve_roam_dir_scope(
    files: &[PathBuf],
    scope_args: &ScopeArgs,
    config: &Config,
    scoped_ids: &mut HashSet<String>,
    id_locations: &mut HashMap<String, Vec<PathBuf>>,
) {
    let mut dirs_to_index: Vec<PathBuf> = Vec::new();

    if !scope_args.roam_dir.is_empty() {
        for selector in &scope_args.roam_dir {
            if let Some((_, resolved)) = config.resolve_roam_dir(selector) {
                dirs_to_index.push(resolved);
            } else {
                let p = PathBuf::from(selector);
                if p.is_dir() {
                    dirs_to_index.push(p);
                }
            }
        }
    } else {
        for file in files {
            let norm = normalize_workspace_path(file);
            for (_, roam_path) in config.roam_directories() {
                if norm.starts_with(&roam_path) && !dirs_to_index.contains(&roam_path) {
                    dirs_to_index.push(roam_path);
                }
            }
        }
    }

    if dirs_to_index.is_empty() {
        resolve_dir_scope(files, scoped_ids, id_locations);
        return;
    }

    for dir in dirs_to_index {
        index_cache_candidates(&dir, scoped_ids, id_locations);
    }
}

/// Resolves IDs for [`IdScope::Dir`] against all `.org` files in the target directory.
fn resolve_dir_scope(
    files: &[PathBuf],
    scoped_ids: &mut HashSet<String>,
    id_locations: &mut HashMap<String, Vec<PathBuf>>,
) {
    let dir = if let Some(first) = files.first() {
        if first.is_dir() {
            first.clone()
        } else {
            first.parent().unwrap_or(Path::new(".")).to_path_buf()
        }
    } else {
        PathBuf::from(".")
    };

    let org_files = collect_org_files(&[dir]);
    for path in org_files {
        if let Ok(src) = SourceFile::from_path(&path) {
            let doc = OrgDocument::from_source(&src);
            if let Some(fid) = doc.file_id() {
                let norm = fid.to_ascii_lowercase();
                scoped_ids.insert(norm.clone());
                scoped_ids.insert(fid.to_string());
                id_locations.entry(norm).or_default().push(path.clone());
            }
            for entry in &doc.entries {
                if let Some((_, id)) = entry
                    .properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("ID"))
                {
                    let norm = id.to_ascii_lowercase();
                    scoped_ids.insert(norm.clone());
                    scoped_ids.insert(id.clone());
                    id_locations.entry(norm).or_default().push(path.clone());
                }
            }
        }
    }
}

/// Resolves the effective ID and document scope for `files` under `scope_args` and `config`.
pub(crate) fn resolve_fmt_scope(
    files: &[PathBuf],
    scope_args: &ScopeArgs,
    config: &Config,
) -> ResolvedScope {
    let effective_scope = scope_args.id_scope.unwrap_or(config.lint.id_scope);
    let mut scoped_ids = HashSet::new();
    let mut id_locations = HashMap::new();

    match effective_scope {
        IdScope::File => {}
        IdScope::RoamDir => {
            resolve_roam_dir_scope(
                files,
                scope_args,
                config,
                &mut scoped_ids,
                &mut id_locations,
            );
        }
        IdScope::Dir => {
            resolve_dir_scope(files, &mut scoped_ids, &mut id_locations);
        }
        IdScope::Vault => {
            let found_vault = resolve_vault_scope(
                files,
                scope_args,
                config,
                &mut scoped_ids,
                &mut id_locations,
            );
            if !found_vault {
                resolve_dir_scope(files, &mut scoped_ids, &mut id_locations);
            }
        }
    }

    let workspace_docs: Vec<OrgDocument> = files
        .iter()
        .filter_map(|p| SourceFile::from_path(p).ok())
        .map(|src| OrgDocument::from_source(&src))
        .collect();

    ResolvedScope {
        effective_scope,
        scoped_ids,
        id_locations,
        workspace_docs,
    }
}
