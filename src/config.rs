use serde::Deserialize;
use std::path::Path;

/// Configuration for orgfmt, loaded from `.orgfmt.toml`.
///
/// Defaults match Emacs org-mode behavior: no enforcement of blank line
/// rules, no heading spacing enforcement. Users can opt into opinionated
/// formatting by enabling these rules explicitly.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub format: FormatConfig,
    pub lint: LintConfig,
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LintConfig {
    /// List of rule names or IDs to disable.
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

impl Config {
    /// Load config from `.orgfmt.toml` in the given directory or its ancestors.
    /// Returns default config if no file is found.
    pub fn load(start_dir: &Path) -> Self {
        let mut dir = start_dir;
        loop {
            let config_path = dir.join(".orgfmt.toml");
            if config_path.is_file() {
                match std::fs::read_to_string(&config_path) {
                    Ok(contents) => match toml::from_str::<Config>(&contents) {
                        Ok(config) => return config,
                        Err(e) => {
                            eprintln!("orgfmt: error parsing {}: {}", config_path.display(), e);
                            return Self::default();
                        }
                    },
                    Err(e) => {
                        eprintln!("orgfmt: error reading {}: {}", config_path.display(), e);
                        return Self::default();
                    }
                }
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break,
            }
        }
        Self::default()
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
}
