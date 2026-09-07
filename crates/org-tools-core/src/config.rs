// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Error encountered when loading configuration from `.org-tools.toml`.
#[derive(Debug)]
pub enum ConfigError {
    /// Failed to read the configuration file.
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Failed to parse the TOML content.
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl std::fmt::Display for ConfigError {
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
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Read { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
        }
    }
}

/// Configuration for org-tools, loaded from `.org-tools.toml`.
///
/// Defaults match Emacs org-mode behavior: no enforcement of blank line
/// rules, no heading spacing enforcement. Users can opt into opinionated
/// formatting by enabling these rules explicitly.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Format rule configuration.
    pub format: FormatConfig,
    /// Lint rule configuration.
    pub lint: LintConfig,
    /// Persistent SQLite cache configuration.
    pub cache: CacheConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FormatConfig {
    /// Remove trailing whitespace from all lines.
    pub trailing_whitespace: bool,

    /// Collapse consecutive blank lines to at most `max_consecutive_blank_lines`.
    /// Emacs default: false (org-mode does not enforce blank line limits).
    pub blank_lines: bool,

    /// Maximum consecutive blank lines allowed (only when `blank_lines` is true).
    pub max_consecutive_blank_lines: usize,

    /// Enforce blank lines before headings.
    /// Emacs default: false (org-mode does not enforce heading spacing).
    pub heading_blank_lines: bool,

    /// Number of blank lines required before a heading (only when `heading_blank_lines` is true).
    pub heading_blank_lines_before: usize,

    /// Align table columns and normalize separators.
    pub table_format: bool,

    /// Align property values within PROPERTIES drawers.
    pub property_drawer_align: bool,
}

/// Configuration for lint rules.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LintConfig {
    /// Rule names or IDs to disable (e.g., `["W001", "heading-level-gap"]`).
    pub disabled_rules: Vec<String>,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            trailing_whitespace: true,
            // Emacs does not enforce blank line limits — disabled by default.
            blank_lines: false,
            max_consecutive_blank_lines: 1,
            // Emacs does not enforce heading spacing — disabled by default.
            heading_blank_lines: false,
            heading_blank_lines_before: 1,
            table_format: true,
            property_drawer_align: true,
        }
    }
}

fn default_threshold_ms() -> u64 {
    500
}

/// Cache configuration loaded from `[cache]` section in `.org-tools.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Enable persistent SQLite cache for ID resolution, search, and indexing.
    /// Default: false (disabled).
    pub enabled: bool,
    /// Path to the SQLite cache database file.
    /// If None, defaults to `.org-cache.db` in workspace root or `$XDG_CACHE_HOME/org-tools/cache.db`.
    pub path: Option<String>,
    /// Latency threshold in milliseconds after which slow queries suggest enabling cache.
    /// Default: 500 ms. Set to 0 to disable suggestions.
    pub threshold_ms: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: None,
            threshold_ms: default_threshold_ms(),
        }
    }
}

impl Config {
    /// Load config from `.org-tools.toml` in the given directory or its ancestors.
    /// Returns default config if no file is found, or an error if a config file is present but invalid.
    pub fn load(start_dir: &Path) -> Result<Self, ConfigError> {
        let mut dir = start_dir;
        loop {
            let config_path = dir.join(".org-tools.toml");
            if config_path.is_file() {
                let contents = std::fs::read_to_string(&config_path).map_err(|source| {
                    ConfigError::Read {
                        path: config_path.clone(),
                        source,
                    }
                })?;
                let config = toml::from_str::<Config>(&contents).map_err(|source| {
                    ConfigError::Parse {
                        path: config_path.clone(),
                        source,
                    }
                })?;
                return Ok(config);
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break,
            }
        }
        Ok(Self::default())
    }

    /// Check if a rule is disabled by name or ID.
    /// Accepts both the kebab-case name (e.g., "heading-level-gap")
    /// and the rule ID (e.g., "W001").
    pub fn is_rule_disabled(&self, id: &str, name: &str) -> bool {
        self.lint
            .disabled_rules
            .iter()
            .any(|r| r == name || r.eq_ignore_ascii_case(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_emacs() {
        let config = Config::default();
        // Emacs does not enforce blank line limits or heading spacing.
        assert!(!config.format.blank_lines);
        assert!(!config.format.heading_blank_lines);
        // But trailing whitespace and table formatting are on.
        assert!(config.format.trailing_whitespace);
        assert!(config.format.table_format);
        assert!(config.format.property_drawer_align);
    }

    #[test]
    fn parse_toml_config() {
        let toml = r#"
[format]
blank_lines = true
heading_blank_lines = true
heading_blank_lines_before = 2

[lint]
disabled_rules = ["heading-level-gap"]
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.format.blank_lines);
        assert!(config.format.heading_blank_lines);
        assert_eq!(config.format.heading_blank_lines_before, 2);
        assert!(config.is_rule_disabled("W001", "heading-level-gap"));
        assert!(!config.is_rule_disabled("E001", "unclosed-block"));
    }

    #[test]
    fn partial_toml_uses_defaults() {
        let toml = r#"
[format]
blank_lines = true
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.format.blank_lines);
        // Other fields use defaults.
        assert!(!config.format.heading_blank_lines);
        assert!(config.format.trailing_whitespace);
    }

    #[test]
    fn load_default_when_no_file_found() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(dir.path()).unwrap();
        assert!(!config.format.blank_lines);
    }

    #[test]
    fn load_valid_config_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join(".org-tools.toml");
        std::fs::write(&config_file, "[format]\nblank_lines = true\n").unwrap();

        let config = Config::load(dir.path()).unwrap();
        assert!(config.format.blank_lines);
    }

    #[test]
    fn load_invalid_toml_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join(".org-tools.toml");
        std::fs::write(&config_file, "this is not valid toml = [[[").unwrap();

        let result = Config::load(dir.path());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigError::Parse { .. }));
    }

    #[test]
    fn load_cache_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join(".org-tools.toml");
        std::fs::write(
            &config_file,
            "[cache]\nenabled = true\npath = \"/tmp/my-cache.db\"\nthreshold_ms = 250\n",
        )
        .unwrap();

        let config = Config::load(dir.path()).unwrap();
        assert!(config.cache.enabled);
        assert_eq!(config.cache.path.as_deref(), Some("/tmp/my-cache.db"));
        assert_eq!(config.cache.threshold_ms, 250);
    }
}

