// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Federated and single-vault `RoamWorkspace` query facade over `WorkspaceCache` and `RoamIndex`.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::export_md::normalize_workspace_path;
use crate::files::collect_org_files;
use crate::roam::{RoamBacklink, RoamIndex, RoamNode, RoamOutgoingLink};

use super::schema::adjust_roam_telemetry_counts;
use super::WorkspaceCache;

/// A synchronized SQLite vault member inside [`RoamWorkspace::Federated`].
pub struct FederatedVault {
    /// Vault name or directory identifier attributed to [`RoamNode::roam_dir`].
    pub name: String,
    /// Open synchronized SQLite cache handle (`<roam_dir>/.cache/org-cache.db`).
    pub cache: WorkspaceCache,
    /// Display root path supplied by the caller.
    pub display_root: PathBuf,
    /// Scan counters `(files_scanned, cache_hits, cache_misses)`.
    pub counts: (usize, usize, usize),
}

/// Unified `org-roam` v2 query backend backed by [`WorkspaceCache`], federated vaults, or [`RoamIndex`].
pub enum RoamWorkspace {
    /// Incremental SQLite + FTS5 cache bound to a single workspace directory.
    Cached {
        /// Open synchronized SQLite cache handle.
        cache: Box<WorkspaceCache>,
        /// Display root path supplied by the caller.
        display_root: PathBuf,
        /// Scan counters `(files_scanned, cache_hits, cache_misses)`.
        counts: (usize, usize, usize),
    },
    /// Federated multi-vault SQLite backend across multiple active `org-roam` directories.
    Federated {
        /// Synchronized per-vault SQLite cache handles.
        vaults: Vec<FederatedVault>,
        /// Aggregate scan counters `(files_scanned, cache_hits, cache_misses)`.
        counts: (usize, usize, usize),
    },
    /// Uncached in-memory [`RoamIndex`] built directly from `.org` files.
    Direct(RoamIndex),
}

impl RoamWorkspace {
    /// Opens a single-vault, federated multi-vault, or uncached [`RoamWorkspace`].
    pub fn open(roots: &[PathBuf], no_cache: bool) -> Result<Self, String> {
        if !no_cache && roots.len() == 1 && roots[0].is_dir() {
            let display_root = roots[0].clone();
            let (cache, t) = WorkspaceCache::open_and_sync(&display_root)?;
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
        if !no_cache && roots.len() >= 2 && roots.iter().all(|r| r.is_dir()) {
            let named: Vec<(String, PathBuf)> = roots
                .iter()
                .map(|r| {
                    let name = r
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();
                    (name, r.clone())
                })
                .collect();
            return Self::open_named(&named, false);
        }
        Self::open_direct(roots)
    }

    /// Opens a federated [`RoamWorkspace::Federated`] across named `(vault_name, root)` pairs.
    pub fn open_named(named_roots: &[(String, PathBuf)], no_cache: bool) -> Result<Self, String> {
        if no_cache {
            let paths: Vec<PathBuf> = named_roots.iter().map(|(_, p)| p.clone()).collect();
            return Self::open_direct(&paths);
        }
        let mut vaults = Vec::with_capacity(named_roots.len());
        let mut total = (0usize, 0usize, 0usize);
        for (name, display_root) in named_roots {
            let (cache, t) = WorkspaceCache::open_and_sync(display_root)?;
            let c = adjust_roam_telemetry_counts(
                &cache.conn,
                t.files_scanned,
                t.cache_hits,
                t.cache_misses,
                &cache.missed_rels,
            )?;
            total = (total.0 + c.0, total.1 + c.1, total.2 + c.2);
            vaults.push(FederatedVault {
                name: name.clone(),
                cache,
                display_root: display_root.clone(),
                counts: c,
            });
        }
        Ok(Self::Federated {
            vaults,
            counts: total,
        })
    }

    /// Builds an uncached [`RoamWorkspace::Direct`] from `roots`.
    fn open_direct(roots: &[PathBuf]) -> Result<Self, String> {
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
            Self::Cached { counts, .. } | Self::Federated { counts, .. } => *counts,
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
            Self::Federated { vaults, .. } => {
                let mut out = Vec::new();
                for v in vaults {
                    for mut n in v.cache.resolve_roam_nodes(&v.display_root, query, true)? {
                        n.roam_dir = Some(v.name.clone());
                        out.push(n);
                    }
                }
                out.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
                Ok(out)
            }
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
            Self::Federated { vaults, .. } => {
                let mut out = Vec::new();
                for v in vaults {
                    out.extend(v.cache.roam_backlinks_for(&v.display_root, targets)?);
                }
                out.sort_by(|a, b| (&a.source_file, a.line).cmp(&(&b.source_file, b.line)));
                Ok(out)
            }
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
            Self::Federated { vaults, .. } => {
                let mut out = Vec::new();
                for v in vaults {
                    out.extend(v.cache.roam_outgoing_links_for(&v.display_root, sources)?);
                }
                for link in &mut out {
                    if !link.resolved {
                        if let Some(found) = self
                            .resolve(&format!("id:{}", link.target_id))?
                            .into_iter()
                            .next()
                        {
                            link.target_title = Some(found.title);
                            link.target_file = Some(found.file);
                            link.resolved = true;
                        }
                    }
                }
                out.sort_by(|a, b| (&a.source_file, a.line).cmp(&(&b.source_file, b.line)));
                Ok(out)
            }
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
            Self::Federated { vaults, .. } => {
                let mut nodes = Vec::new();
                let mut links = Vec::new();
                for v in vaults {
                    let g = v.cache.load_roam_graph(&v.display_root, v.counts.0)?;
                    for mut n in g.nodes {
                        n.roam_dir = Some(v.name.clone());
                        nodes.push(n);
                    }
                    links.extend(g.links);
                }
                nodes.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
                links.sort_by(|a, b| (&a.source_file, a.line).cmp(&(&b.source_file, b.line)));
                Ok((
                    RoamIndex {
                        nodes,
                        links,
                        docs: Vec::new(),
                        files_scanned: counts.0,
                    },
                    counts,
                ))
            }
            Self::Direct(idx) => Ok((idx, counts)),
        }
    }
}
