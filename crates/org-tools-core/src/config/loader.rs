// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hierarchical `.org-tools.toml` path expansion, discovery, and layer merging.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::capture::CaptureTemplateLayer;

use super::{ConfigError, RoamDirectoriesSpec, ORG_TOOLS_CONFIG_ENV_VAR};

/// Expands `~/`, `$HOME/`, and `${HOME}/` prefixes and resolves relative paths against `base_dir`.
pub fn expand_config_path(raw: &str, base_dir: &Path) -> PathBuf {
    let trimmed = raw.trim();
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty());
    let expanded = if trimmed == "~" {
        home.unwrap_or_else(|| trimmed.to_string())
    } else if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("$HOME/"))
        .or_else(|| trimmed.strip_prefix("${HOME}/"))
    {
        home.map_or_else(|| trimmed.to_string(), |h| format!("{h}/{rest}"))
    } else {
        trimmed.to_string()
    };
    let path = PathBuf::from(&expanded);
    if path.is_absolute() || expanded.starts_with('~') {
        path
    } else {
        base_dir.join(expanded.strip_prefix("./").unwrap_or(&expanded))
    }
}

/// Derives a canonical vault name from `path`'s final path component.
pub(super) fn vault_name_from_path(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// Resolves a [`RoamDirectoriesSpec`] against `base_dir` into `(vault_name, PathBuf)` pairs.
pub(super) fn resolve_spec_directories(
    spec: &RoamDirectoriesSpec,
    base_dir: &Path,
) -> Vec<(String, PathBuf)> {
    match spec {
        RoamDirectoriesSpec::Named(map) => map
            .iter()
            .map(|(k, v)| (k.clone(), expand_config_path(v, base_dir)))
            .collect(),
        RoamDirectoriesSpec::List(list) => list
            .iter()
            .map(|raw| {
                let p = expand_config_path(raw, base_dir);
                (vault_name_from_path(&p, raw.trim()), p)
            })
            .collect(),
    }
}

/// Parses `ORG_ROAM_DIRECTORIES` (`name=/path` or `/path` separated by `,` or `:`).
pub(super) fn parse_env_roam_directories(raw: &str, base_dir: &Path) -> Vec<(String, PathBuf)> {
    raw.split([',', ':'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| match item.split_once('=') {
            Some((name, val)) => (name.trim().to_string(), expand_config_path(val, base_dir)),
            None => {
                let p = expand_config_path(item, base_dir);
                (vault_name_from_path(&p, item), p)
            }
        })
        .collect()
}

/// Recursively merges TOML tables while letting non-table values overwrite earlier layers.
fn deep_merge_toml_tables(dst: &mut toml::Table, src: toml::Table) {
    for (k, v) in src {
        if k == "directories" {
            dst.insert(k, v);
            continue;
        }
        match (dst.get_mut(&k), v) {
            (Some(toml::Value::Table(d_sub)), toml::Value::Table(s_sub)) => {
                deep_merge_toml_tables(d_sub, s_sub);
            }
            (_, val) => {
                dst.insert(k, val);
            }
        }
    }
}

/// State accumulated while merging hierarchical `.org-tools.toml` configuration layers.
#[derive(Default)]
pub(super) struct LayerMergeState {
    /// Merged raw TOML document table.
    pub(super) merged: toml::Table,
    /// Pre-resolved `(vault_name, path)` pairs.
    pub(super) resolved_dirs: Vec<(String, PathBuf)>,
    /// Base directory of the config file that last defined `default_directory`.
    pub(super) default_base_dir: Option<PathBuf>,
    /// Provenance `(layer, base_dir)` per `[capture.templates.<name>]` entry.
    pub(super) template_meta: BTreeMap<String, (CaptureTemplateLayer, PathBuf)>,
}

/// Merges a parsed TOML `overlay` from `config_path` into `state`.
pub(super) fn merge_config_layer(
    state: &mut LayerMergeState,
    overlay: toml::Table,
    config_path: &Path,
    layer: CaptureTemplateLayer,
) -> Result<(), ConfigError> {
    let base_dir = config_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    if let Some(toml::Value::Table(roam_tbl)) = overlay.get("roam") {
        if let Some(dir_val) = roam_tbl.get("directories") {
            let spec: RoamDirectoriesSpec =
                dir_val
                    .clone()
                    .try_into()
                    .map_err(|source| ConfigError::Parse {
                        path: config_path.to_path_buf(),
                        source,
                    })?;
            state.resolved_dirs = resolve_spec_directories(&spec, &base_dir);
        }
        if roam_tbl.contains_key("default_directory") {
            state.default_base_dir = Some(base_dir.clone());
        }
    }
    if let Some(toml::Value::Table(tpls)) = overlay.get("capture").and_then(|c| c.get("templates"))
    {
        for tpl_name in tpls.keys() {
            state
                .template_meta
                .insert(tpl_name.clone(), (layer, base_dir.clone()));
        }
    }
    deep_merge_toml_tables(&mut state.merged, overlay);
    Ok(())
}

/// Discovers the optional global user config file from `$ORG_TOOLS_CONFIG` or `$XDG_CONFIG_HOME`.
pub fn discover_global_config_path() -> Option<PathBuf> {
    if let Some(val) = std::env::var_os(ORG_TOOLS_CONFIG_ENV_VAR).filter(|v| !v.is_empty()) {
        let p = PathBuf::from(val);
        if p.is_file() {
            return Some(p);
        }
    }
    let candidate = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(|xdg| PathBuf::from(xdg).join("org-tools/config.toml"))
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".config/org-tools/config.toml"))
        })?;
    candidate.is_file().then_some(candidate)
}

/// Collects ordered `(config_path, layer)` pairs from `global_override` down to `start_dir`.
pub(super) fn collect_config_chain(
    start_dir: &Path,
    global_override: Option<&Path>,
) -> Vec<(PathBuf, CaptureTemplateLayer)> {
    let mut files = Vec::new();
    if let Some(g) = global_override.filter(|p| p.is_file()) {
        files.push((g.to_path_buf(), CaptureTemplateLayer::Global));
    }
    let mut ancestors = Vec::new();
    let mut cur = Some(if start_dir.is_file() {
        start_dir.parent().unwrap_or(start_dir)
    } else {
        start_dir
    });
    while let Some(dir) = cur {
        let candidate = dir.join(".org-tools.toml");
        if candidate.is_file() {
            ancestors.push(candidate);
        }
        cur = dir.parent();
    }
    ancestors.reverse();
    let count = ancestors.len();
    for (idx, path) in ancestors.into_iter().enumerate() {
        let layer = if count >= 2 && idx + 1 == count {
            CaptureTemplateLayer::Vault
        } else {
            CaptureTemplateLayer::Workspace
        };
        files.push((path, layer));
    }
    files
}
