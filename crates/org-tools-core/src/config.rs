// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Configuration loading and validation for `.org-tools.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::rules::heading::{parse_todo_spec, TodoKeywords};

// region Public Types & Constants

/// Default lead-time days before an unwarned `DEADLINE:` appears in the agenda.
pub const DEFAULT_DEADLINE_WARNING_DAYS: u32 = 14;

/// Environment variable overriding the global user configuration file path.
pub const ORG_TOOLS_CONFIG_ENV_VAR: &str = "ORG_TOOLS_CONFIG";

/// Environment variable overriding configured `org-roam` v2 directories.
pub const ORG_ROAM_DIRECTORIES_ENV_VAR: &str = "ORG_ROAM_DIRECTORIES";

/// Error encountered when loading configuration from `.org-tools.toml`.
#[derive(Debug)]
pub enum ConfigError {
    /// Failed to read the configuration file from disk.
    Read {
        /// Path to the unreadable `.org-tools.toml` file.
        path: PathBuf,
        /// Underlying filesystem I/O error.
        source: std::io::Error,
    },
    /// Failed to parse the TOML content.
    Parse {
        /// Path to the malformed `.org-tools.toml` file.
        path: PathBuf,
        /// Underlying TOML deserialization error.
        source: toml::de::Error,
    },
}

impl std::fmt::Display for ConfigError {
    /// Formats the configuration error with its file path and underlying cause.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Read { path, source } => {
                write!(f, "error reading {}: {}", path.display(), source)
            }
            ConfigError::Parse { path, source } => {
                write!(f, "error parsing {}: {}", path.display(), source)
            }
        }
    }
}

impl std::error::Error for ConfigError {
    /// Returns the underlying filesystem or TOML deserialization error.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Read { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
        }
    }
}

/// Configuration for org-tools, loaded hierarchically from `.org-tools.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Format rule configuration.
    pub format: FormatConfig,
    /// Lint rule configuration.
    pub lint: LintConfig,
    /// Agenda view configuration.
    pub agenda: AgendaConfig,
    /// Multi-directory `org-roam` v2 vault configuration.
    pub roam: RoamConfig,
    /// Fallback TODO keyword sequence configuration.
    pub todo: TodoConfig,
}

/// Configuration for agenda views (`[agenda]` section in `.org-tools.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AgendaConfig {
    /// Lead-time warning window in days for deadlines without an explicit `-Nd` suffix.
    pub deadline_warning_days: u32,
}

impl Default for AgendaConfig {
    /// Returns default agenda settings matching Emacs (`deadline_warning_days = 14`).
    fn default() -> Self {
        Self {
            deadline_warning_days: DEFAULT_DEADLINE_WARNING_DAYS,
        }
    }
}

/// Configuration for auto-fixable formatting rules (`F001`–`F005`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FormatConfig {
    /// Remove trailing whitespace from all lines.
    pub trailing_whitespace: bool,
    /// Collapse consecutive blank lines to at most `max_consecutive_blank_lines`.
    pub blank_lines: bool,
    /// Maximum consecutive blank lines allowed (only when `blank_lines` is true).
    pub max_consecutive_blank_lines: usize,
    /// Enforce blank lines before headings.
    pub heading_blank_lines: bool,
    /// Number of blank lines required before a heading (only when `heading_blank_lines` is true).
    pub heading_blank_lines_before: usize,
    /// Align table columns and normalize separators.
    pub table_format: bool,
    /// Align property values within PROPERTIES drawers.
    pub property_drawer_align: bool,
}

impl Default for FormatConfig {
    /// Returns default formatting settings matching Emacs org-mode defaults.
    fn default() -> Self {
        Self {
            trailing_whitespace: true,
            blank_lines: false,
            max_consecutive_blank_lines: 1,
            heading_blank_lines: false,
            heading_blank_lines_before: 1,
            table_format: true,
            property_drawer_align: true,
        }
    }
}

/// Configuration for lint rules.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LintConfig {
    /// Rule names or IDs to disable (e.g., `["W001", "heading-level-gap"]`).
    pub disabled_rules: Vec<String>,
}

/// Raw `[roam.directories]` table or `[roam] directories = [...]` array in `.org-tools.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum RoamDirectoriesSpec {
    /// Named vault table (`work = "~/org/work"`, `private = "~/org/private"`).
    Named(BTreeMap<String, String>),
    /// Positional list of vault directory paths (`["~/org/work", "~/org/private"]`).
    List(Vec<String>),
}

impl Default for RoamDirectoriesSpec {
    /// Returns an empty named vault table by default.
    fn default() -> Self {
        Self::Named(BTreeMap::new())
    }
}

/// Multi-directory `org-roam` v2 vault configuration (`[roam]` in `.org-tools.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RoamConfig {
    /// Named vault table or list of vault directory paths.
    pub directories: RoamDirectoriesSpec,
    /// Optional default vault name or directory path.
    pub default_directory: Option<String>,
    /// Pre-resolved `(vault_name, path)` pairs anchored to the defining config file's directory.
    #[serde(skip)]
    pub resolved_directories: Vec<(String, PathBuf)>,
    /// Base directory of the config file that last defined `default_directory`.
    #[serde(skip)]
    pub default_base_dir: Option<PathBuf>,
}

/// Fallback TODO sequence configuration (`[todo]` in `.org-tools.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TodoConfig {
    /// Additive `#+TODO:` sequence specifications (e.g., `["TODO NEXT | DONE"]`).
    pub sequences: Vec<String>,
    /// Optional alias list of TODO sequence strings.
    pub keywords: Vec<String>,
    /// Optional explicit active ("not done") keywords.
    pub todo: Vec<String>,
    /// Optional explicit completed ("done") keywords.
    pub done: Vec<String>,
}

impl TodoConfig {
    /// Converts configured fallback sequences into [`TodoKeywords`], or `None` if unconfigured.
    pub fn to_todo_keywords(&self) -> Option<TodoKeywords> {
        if !self.sequences.is_empty() {
            return Some(TodoKeywords::from_specs(&self.sequences));
        }
        if !self.keywords.is_empty() {
            return Some(parse_todo_spec(&self.keywords.join(" ")));
        }
        if !self.todo.is_empty() || !self.done.is_empty() {
            return Some(TodoKeywords {
                sequences: vec![(self.todo.clone(), self.done.clone())],
                todo: self.todo.clone(),
                done: self.done.clone(),
                log_on_enter: std::collections::HashSet::new(),
                log_on_leave: std::collections::HashSet::new(),
            });
        }
        None
    }
}

// endregion

// region Path Expansion & Hierarchical Merging

/// Expands `~/`, `$HOME/`, and `${HOME}/` prefixes and resolves relative paths against `base_dir`.
pub fn expand_config_path(raw: &str, base_dir: &Path) -> PathBuf {
    let trimmed = raw.trim();
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty());
    let expanded = if trimmed == "~" {
        home.unwrap_or_else(|| trimmed.to_string())
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home.map_or_else(|| trimmed.to_string(), |h| format!("{h}/{rest}"))
    } else if let Some(rest) = trimmed
        .strip_prefix("$HOME/")
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
fn vault_name_from_path(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// Resolves a [`RoamDirectoriesSpec`] against `base_dir` into `(vault_name, PathBuf)` pairs.
fn resolve_spec_directories(spec: &RoamDirectoriesSpec, base_dir: &Path) -> Vec<(String, PathBuf)> {
    match spec {
        RoamDirectoriesSpec::Named(map) => map
            .iter()
            .map(|(k, v)| (k.clone(), expand_config_path(v, base_dir)))
            .collect(),
        RoamDirectoriesSpec::List(list) => list
            .iter()
            .map(|raw| {
                let resolved = expand_config_path(raw, base_dir);
                (vault_name_from_path(&resolved, raw.trim()), resolved)
            })
            .collect(),
    }
}

/// Parses `ORG_ROAM_DIRECTORIES` (`name=/path` or `/path` separated by `,` or `:`).
fn parse_env_roam_directories(raw: &str, base_dir: &Path) -> Vec<(String, PathBuf)> {
    raw.split([',', ':'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| {
            if let Some((name, val)) = item.split_once('=') {
                (name.trim().to_string(), expand_config_path(val, base_dir))
            } else {
                let p = expand_config_path(item, base_dir);
                (vault_name_from_path(&p, item), p)
            }
        })
        .collect()
}

/// Merges a parsed TOML `overlay` table from `config_path` into `merged` and updates roam anchors.
fn merge_config_layer(
    merged: &mut toml::Table,
    overlay: toml::Table,
    config_path: &Path,
    resolved_dirs: &mut Vec<(String, PathBuf)>,
    default_base_dir: &mut Option<PathBuf>,
) -> Result<(), ConfigError> {
    let base_dir = config_path.parent().unwrap_or(Path::new("."));
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
            *resolved_dirs = resolve_spec_directories(&spec, base_dir);
        }
        if roam_tbl.contains_key("default_directory") {
            *default_base_dir = Some(base_dir.to_path_buf());
        }
    }
    for (sec_key, sec_val) in overlay {
        match (merged.get_mut(&sec_key), sec_val) {
            (Some(toml::Value::Table(dst)), toml::Value::Table(src)) => {
                for (k, v) in src {
                    dst.insert(k, v);
                }
            }
            (_, val) => {
                merged.insert(sec_key, val);
            }
        }
    }
    Ok(())
}

/// Discovers the optional global user config file from `$ORG_TOOLS_CONFIG` or `$XDG_CONFIG_HOME`.
fn discover_global_config_path() -> Option<PathBuf> {
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

/// Collects ordered `.org-tools.toml` files from `global_override` down to `start_dir`.
fn collect_config_chain(start_dir: &Path, global_override: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(g) = global_override.filter(|p| p.is_file()) {
        files.push(g.to_path_buf());
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
    files.extend(ancestors);
    files
}

// endregion

// region Config Implementation & Tests

impl Config {
    /// Loads configuration from `.org-tools.toml` in `start_dir` or its ancestors.
    pub fn load(start_dir: &Path) -> Result<Self, ConfigError> {
        Self::load_for_dir(start_dir)
    }

    /// Hierarchically loads and merges global, workspace, and per-directory `.org-tools.toml` files.
    pub fn load_for_dir(start_dir: &Path) -> Result<Self, ConfigError> {
        let global = discover_global_config_path();
        Self::load_for_dir_with_global(start_dir, global.as_deref())
    }

    /// Hierarchically loads configuration for `start_dir` with an explicit `global_override` path.
    pub fn load_for_dir_with_global(
        start_dir: &Path,
        global_override: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        let files = collect_config_chain(start_dir, global_override);
        if files.is_empty() {
            return Ok(Self::default());
        }
        let mut merged = toml::Table::new();
        let mut resolved_dirs = Vec::new();
        let mut default_base_dir = None;
        let mut last_path = files[0].clone();
        for path in files {
            let contents = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
                path: path.clone(),
                source,
            })?;
            let overlay =
                toml::from_str::<toml::Table>(&contents).map_err(|source| ConfigError::Parse {
                    path: path.clone(),
                    source,
                })?;
            merge_config_layer(
                &mut merged,
                overlay,
                &path,
                &mut resolved_dirs,
                &mut default_base_dir,
            )?;
            last_path = path;
        }
        let mut cfg: Self =
            toml::Value::Table(merged)
                .try_into()
                .map_err(|source| ConfigError::Parse {
                    path: last_path,
                    source,
                })?;
        cfg.roam.resolved_directories = resolved_dirs;
        cfg.roam.default_base_dir = default_base_dir;
        Ok(cfg)
    }

    /// Checks whether a rule is disabled by kebab-case name or rule ID.
    pub fn is_rule_disabled(&self, id: &str, name: &str) -> bool {
        self.lint
            .disabled_rules
            .iter()
            .any(|r| r == name || r.eq_ignore_ascii_case(id))
    }

    /// Returns resolved `(vault_name, directory_path)` pairs, honoring `ORG_ROAM_DIRECTORIES`.
    pub fn roam_directories(&self) -> Vec<(String, PathBuf)> {
        if let Some(env_val) = std::env::var(ORG_ROAM_DIRECTORIES_ENV_VAR)
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            let base = self
                .roam
                .default_base_dir
                .as_deref()
                .unwrap_or(Path::new("."));
            return parse_env_roam_directories(&env_val, base);
        }
        if !self.roam.resolved_directories.is_empty() {
            return self.roam.resolved_directories.clone();
        }
        resolve_spec_directories(&self.roam.directories, Path::new("."))
    }

    /// Resolves the configured `default_directory` (or first configured vault) into `(name, path)`.
    pub fn default_roam_dir(&self) -> Option<(String, PathBuf)> {
        let dirs = self.roam_directories();
        if let Some(def) = self.roam.default_directory.as_deref() {
            if let Some(found) = dirs.iter().find(|(n, _)| n == def) {
                return Some(found.clone());
            }
            let base = self
                .roam
                .default_base_dir
                .as_deref()
                .unwrap_or(Path::new("."));
            let p = expand_config_path(def, base);
            return Some((vault_name_from_path(&p, def), p));
        }
        dirs.into_iter().next()
    }

    /// Resolves a `--roam-dir <name|path>` selector against configured vaults or existing disk directories.
    pub fn resolve_roam_dir(&self, selector: &str) -> Option<(Option<String>, PathBuf)> {
        let trimmed = selector.trim();
        if trimmed.is_empty() {
            return None;
        }
        let dirs = self.roam_directories();
        if let Some((name, path)) = dirs.iter().find(|(n, _)| n == trimmed) {
            return Some((Some(name.clone()), path.clone()));
        }
        let expanded = expand_config_path(trimmed, Path::new("."));
        if expanded.is_dir() {
            let matched_name = dirs
                .iter()
                .find(|(_, p)| p == &expanded)
                .map(|(n, _)| n.clone())
                .or_else(|| Some(vault_name_from_path(&expanded, trimmed)));
            return Some((matched_name, expanded));
        }
        None
    }
}

/// Unit tests for `.org-tools.toml` configuration loading and defaults.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies default configuration matches Emacs org-mode defaults.
    #[test]
    fn default_config_matches_emacs() {
        let config = Config::default();
        assert!(!config.format.blank_lines);
        assert!(!config.format.heading_blank_lines);
        assert!(config.format.trailing_whitespace);
        assert!(config.format.table_format);
        assert!(config.format.property_drawer_align);
        assert_eq!(
            config.agenda.deadline_warning_days,
            DEFAULT_DEADLINE_WARNING_DAYS
        );
    }

    /// Verifies deserializing `[format]` and `[lint]` settings from TOML.
    #[test]
    fn parse_toml_config() {
        let toml = "[format]\nblank_lines = true\nheading_blank_lines = true\nheading_blank_lines_before = 2\n[lint]\ndisabled_rules = [\"heading-level-gap\"]\n";
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.format.blank_lines);
        assert!(config.format.heading_blank_lines);
        assert_eq!(config.format.heading_blank_lines_before, 2);
        assert!(config.is_rule_disabled("W001", "heading-level-gap"));
    }

    /// Verifies `Config::load` reads valid `.org-tools.toml` and errors on malformed TOML.
    #[test]
    fn load_valid_and_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!Config::load(dir.path()).unwrap().format.blank_lines);
        let config_file = dir.path().join(".org-tools.toml");
        std::fs::write(&config_file, "[format]\nblank_lines = true\n").unwrap();
        assert!(Config::load(dir.path()).unwrap().format.blank_lines);
        std::fs::write(&config_file, "this is not valid toml = [[[").unwrap();
        assert!(matches!(
            Config::load(dir.path()).unwrap_err(),
            ConfigError::Parse { .. }
        ));
    }
}

// endregion
