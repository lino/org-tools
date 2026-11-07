// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Configuration loading and validation for `.org-tools.toml`.

use serde::Deserialize;
use std::path::{Path, PathBuf};

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

/// Default lead-time days before an unwarned `DEADLINE:` appears in the agenda.
pub const DEFAULT_DEADLINE_WARNING_DAYS: u32 = 14;

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
    /// Agenda view configuration.
    #[serde(default)]
    pub agenda: AgendaConfig,
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
    /// Returns default formatting settings matching Emacs org-mode defaults.
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

impl Config {
    /// Loads config from `.org-tools.toml` in `start_dir` or its ancestors.
    ///
    /// Returns [`Config::default`] when no file is found, or [`ConfigError`]
    /// when a configuration file exists on disk but cannot be read or parsed.
    pub fn load(start_dir: &Path) -> Result<Self, ConfigError> {
        let mut dir = start_dir;
        loop {
            let config_path = dir.join(".org-tools.toml");
            if config_path.is_file() {
                let contents =
                    std::fs::read_to_string(&config_path).map_err(|source| ConfigError::Read {
                        path: config_path.clone(),
                        source,
                    })?;
                let config =
                    toml::from_str::<Config>(&contents).map_err(|source| ConfigError::Parse {
                        path: config_path.clone(),
                        source,
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

    /// Checks whether a rule is disabled by kebab-case name or rule ID.
    pub fn is_rule_disabled(&self, id: &str, name: &str) -> bool {
        self.lint
            .disabled_rules
            .iter()
            .any(|r| r == name || r.eq_ignore_ascii_case(id))
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
    }

    /// Verifies deserializing `[format]` and `[lint]` settings from TOML.
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

    /// Verifies partial TOML files populate unspecified fields with defaults.
    #[test]
    fn partial_toml_uses_defaults() {
        let toml = r#"
[format]
blank_lines = true
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.format.blank_lines);
        assert!(!config.format.heading_blank_lines);
        assert!(config.format.trailing_whitespace);
    }

    /// Verifies `Config::load` returns defaults when no `.org-tools.toml` exists.
    #[test]
    fn load_default_when_no_file_found() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(dir.path()).unwrap();
        assert!(!config.format.blank_lines);
    }

    /// Verifies `Config::load` reads a valid `.org-tools.toml` file.
    #[test]
    fn load_valid_config_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join(".org-tools.toml");
        std::fs::write(&config_file, "[format]\nblank_lines = true\n").unwrap();

        let config = Config::load(dir.path()).unwrap();
        assert!(config.format.blank_lines);
    }

    /// Verifies `Config::load` returns `ConfigError::Parse` for malformed TOML.
    #[test]
    fn load_invalid_toml_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join(".org-tools.toml");
        std::fs::write(&config_file, "this is not valid toml = [[[").unwrap();

        let result = Config::load(dir.path());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigError::Parse { .. }));
    }

    /// Verifies `[agenda] deadline_warning_days` defaults to `14` and deserializes overrides.
    #[test]
    fn agenda_deadline_warning_days_default_and_override() {
        let default_cfg = Config::default();
        assert_eq!(
            default_cfg.agenda.deadline_warning_days,
            DEFAULT_DEADLINE_WARNING_DAYS
        );
        assert_eq!(default_cfg.agenda.deadline_warning_days, 14);

        let custom: Config = toml::from_str("[agenda]\ndeadline_warning_days = 7\n").unwrap();
        assert_eq!(custom.agenda.deadline_warning_days, 7);
    }
}
