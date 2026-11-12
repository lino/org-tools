// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lint rule, scope, and authoring hygiene configuration.

use clap::ValueEnum;
use serde::Deserialize;

/// Scope for resolving `[[id:...]]` links, `org-edna` IDs, and duplicate IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum IdScope {
    /// Resolve IDs against the entire workspace index and configured roam directories.
    #[default]
    Vault,
    /// Resolve IDs strictly against the owning `org-roam` directory.
    RoamDir,
    /// Resolve IDs strictly against `.org` files in the target directory.
    Dir,
    /// Resolve IDs strictly against declarations inside the current file.
    File,
}

impl std::fmt::Display for IdScope {
    /// Formats the scope variant as its canonical kebab-case CLI identifier.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vault => write!(f, "vault"),
            Self::RoamDir => write!(f, "roam-dir"),
            Self::Dir => write!(f, "dir"),
            Self::File => write!(f, "file"),
        }
    }
}

/// Zettelkasten hygiene configuration (`[lint.zettel]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ZettelLintConfig {
    /// Forbid active TODO keywords and checklists in concept notes (or all notes when true).
    pub forbid_tasks: bool,
}

/// Org authoring hygiene configuration (`[lint.authoring]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AuthoringLintConfig {
    /// Forbid `#+DATE:` and `#+AUTHOR:` boilerplate file headers.
    pub forbid_ephemeral_headers: bool,
    /// Maximum allowed character length of non-separator table cells (default 120 when enabled).
    pub max_table_cell_chars: Option<usize>,
}

/// Configuration for lint rules (`[lint]` section in `.org-tools.toml`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LintConfig {
    /// Rule names or IDs to disable (e.g., `["W001", "heading-level-gap"]`).
    pub disabled_rules: Vec<String>,
    /// Rule names or IDs to explicitly enable (e.g., `["W038", "zettel-task-hygiene"]`).
    #[serde(alias = "enabled_rules")]
    pub enable: Vec<String>,
    /// Scope for ID link and duplicate resolution (`vault`, `roam-dir`, `dir`, `file`).
    pub id_scope: IdScope,
    /// Zettelkasten hygiene settings (`[lint.zettel]`).
    pub zettel: ZettelLintConfig,
    /// Authoring hygiene settings (`[lint.authoring]`).
    pub authoring: AuthoringLintConfig,
}
