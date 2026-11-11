// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Configuration loading and validation for `.org-tools.toml`.

mod loader;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::capture::{CaptureConfig, CaptureTemplateLayer};
use crate::rules::heading::{parse_todo_spec, TodoKeywords};

use self::loader::{
    collect_config_chain, merge_config_layer, parse_env_roam_directories, resolve_spec_directories,
    vault_name_from_path, LayerMergeState,
};
pub use self::loader::{discover_global_config_path, expand_config_path};

// region Public Types & Constants

/// Default lead-time days before an unwarned `DEADLINE:` appears in the agenda.
pub const DEFAULT_DEADLINE_WARNING_DAYS: u32 = 14;
/// Default subdirectory for base GTD files under a roam vault.
pub const DEFAULT_ROAM_GTD_SUBDIR: &str = "gtd";
/// Default subdirectory for permanent Zettelkasten notes under a roam vault.
pub const DEFAULT_ROAM_ZETTEL_SUBDIR: &str = "zettel";
/// Default subdirectory for fleeting notes under a roam vault.
pub const DEFAULT_ROAM_FLEETING_SUBDIR: &str = "fleeting";
/// Default subdirectory for daily journal notes under a roam vault.
pub const DEFAULT_ROAM_DAILY_SUBDIR: &str = "daily";
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
                write!(f, "error reading {}: {source}", path.display())
            }
            ConfigError::Parse { path, source } => {
                write!(f, "error parsing {}: {source}", path.display())
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
    /// Capture template configuration (`[capture.templates.<name>]`).
    pub capture: CaptureConfig,
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RoamConfig {
    /// Named vault table or list of vault directory paths.
    pub directories: RoamDirectoriesSpec,
    /// Optional default vault name or directory path.
    pub default_directory: Option<String>,
    /// Relative subdirectory holding base GTD files (default `"gtd"`).
    pub gtd_subdir: String,
    /// Relative subdirectory holding permanent Zettelkasten notes (default `"zettel"`).
    pub zettel_subdir: String,
    /// Relative subdirectory holding fleeting notes (default `"fleeting"`).
    pub fleeting_subdir: String,
    /// Relative subdirectory holding daily notes (default `"daily"`).
    pub daily_subdir: String,
    /// Optional editor command override for interactive captures.
    pub editor: Option<String>,
    /// Whether captures and transformations append a log item to today's daily note.
    pub log_to_daily: bool,
    /// Pre-resolved `(vault_name, path)` pairs anchored to the defining config file's directory.
    #[serde(skip)]
    pub resolved_directories: Vec<(String, PathBuf)>,
    /// Base directory of the config file that last defined `default_directory`.
    #[serde(skip)]
    pub default_base_dir: Option<PathBuf>,
}

impl Default for RoamConfig {
    /// Returns default `org-roam` v2 vault settings (`gtd`, `zettel`, `fleeting`, `daily`).
    fn default() -> Self {
        Self {
            directories: RoamDirectoriesSpec::default(),
            default_directory: None,
            gtd_subdir: DEFAULT_ROAM_GTD_SUBDIR.to_string(),
            zettel_subdir: DEFAULT_ROAM_ZETTEL_SUBDIR.to_string(),
            fleeting_subdir: DEFAULT_ROAM_FLEETING_SUBDIR.to_string(),
            daily_subdir: DEFAULT_ROAM_DAILY_SUBDIR.to_string(),
            editor: None,
            log_to_daily: false,
            resolved_directories: Vec::new(),
            default_base_dir: None,
        }
    }
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
        (!self.todo.is_empty() || !self.done.is_empty()).then(|| TodoKeywords {
            sequences: vec![(self.todo.clone(), self.done.clone())],
            todo: self.todo.clone(),
            done: self.done.clone(),
            log_on_enter: std::collections::HashSet::new(),
            log_on_leave: std::collections::HashSet::new(),
        })
    }
}

// endregion

// region Config Implementation

impl Config {
    /// Loads configuration from `.org-tools.toml` in `start_dir` or its ancestors.
    pub fn load(start_dir: &Path) -> Result<Self, ConfigError> {
        Self::load_for_dir(start_dir)
    }

    /// Hierarchically loads and merges global, workspace, and per-directory `.org-tools.toml` files.
    pub fn load_for_dir(start_dir: &Path) -> Result<Self, ConfigError> {
        Self::load_for_dir_with_global(start_dir, discover_global_config_path().as_deref())
    }

    /// Hierarchically loads configuration for `start_dir` with an explicit `global_override` path.
    pub fn load_for_dir_with_global(
        start_dir: &Path,
        global_override: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        Self::load_from_chain(&collect_config_chain(start_dir, global_override))
    }

    /// Loads configuration for a specific `roam_dir` layered over `workspace_dir` and global config.
    pub fn load_for_roam_dir(workspace_dir: &Path, roam_dir: &Path) -> Result<Self, ConfigError> {
        Self::load_for_roam_dir_with_global(
            workspace_dir,
            roam_dir,
            discover_global_config_path().as_deref(),
        )
    }

    /// Loads configuration for `roam_dir` layered over `workspace_dir` and `global_override`.
    pub fn load_for_roam_dir_with_global(
        workspace_dir: &Path,
        roam_dir: &Path,
        global_override: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        let mut chain = collect_config_chain(workspace_dir, global_override);
        for (_, layer) in &mut chain {
            if *layer == CaptureTemplateLayer::Vault {
                *layer = CaptureTemplateLayer::Workspace;
            }
        }
        let vault_cfg = roam_dir.join(".org-tools.toml");
        if vault_cfg.is_file() {
            if let Some((_, l)) = chain.iter_mut().find(|(p, _)| p == &vault_cfg) {
                *l = CaptureTemplateLayer::Vault;
            } else {
                chain.push((vault_cfg, CaptureTemplateLayer::Vault));
            }
        }
        Self::load_from_chain(&chain)
    }

    /// Materializes a [`Config`] from an ordered `(path, layer)` chain.
    fn load_from_chain(chain: &[(PathBuf, CaptureTemplateLayer)]) -> Result<Self, ConfigError> {
        if chain.is_empty() {
            return Ok(Self::default());
        }
        let mut state = LayerMergeState::default();
        let mut last_path = chain[0].0.clone();
        for (path, layer) in chain {
            let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
                path: path.clone(),
                source,
            })?;
            let overlay =
                toml::from_str::<toml::Table>(&contents).map_err(|source| ConfigError::Parse {
                    path: path.clone(),
                    source,
                })?;
            merge_config_layer(&mut state, overlay, path, *layer)?;
            last_path = path.clone();
        }
        let mut cfg: Self = toml::Value::Table(state.merged)
            .try_into()
            .map_err(|source| ConfigError::Parse {
                path: last_path,
                source,
            })?;
        cfg.roam.resolved_directories = state.resolved_dirs;
        cfg.roam.default_base_dir = state.default_base_dir;
        for (name, (layer, base_dir)) in state.template_meta {
            if let Some(tpl) = cfg.capture.templates.get_mut(&name) {
                tpl.source_layer = layer;
                tpl.base_dir = Some(base_dir);
            }
        }
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
            let matched = dirs
                .iter()
                .find(|(_, p)| p == &expanded)
                .map(|(n, _)| n.clone())
                .or_else(|| Some(vault_name_from_path(&expanded, trimmed)));
            return Some((matched, expanded));
        }
        None
    }
}

// endregion
