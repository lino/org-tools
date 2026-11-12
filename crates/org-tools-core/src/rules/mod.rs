// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rule traits, context structs, and shared parsing utilities.

pub mod format;
pub mod heading;
pub mod lint;
pub mod list;
pub mod timestamp;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::config::Config;
use crate::diagnostic::{Diagnostic, Fix};
use crate::document::OrgDocument;
use crate::source::SourceFile;

/// Shared context passed to [`FormatRule::format`].
///
/// Provides read-only access to the source text and active configuration.
pub struct FormatContext<'a> {
    /// The source file being formatted.
    pub source: &'a SourceFile,
    /// Active configuration for this run.
    pub config: &'a Config,
}

impl<'a> FormatContext<'a> {
    /// Creates a new format context.
    pub fn new(source: &'a SourceFile, config: &'a Config) -> Self {
        Self { source, config }
    }
}

/// Shared context passed to [`LintRule::check`].
///
/// Provides read-only access to the source text, active configuration, and
/// optional pre-parsed workspace documents or indexed ID sets for cross-file validation.
pub struct LintContext<'a> {
    /// The source file being linted.
    pub source: &'a SourceFile,
    /// Active configuration for this run.
    pub config: &'a Config,
    /// Optional slice of parsed workspace documents for cross-file lints.
    pub workspace_docs: Option<&'a [OrgDocument]>,
    /// Optional set of known `:ID:` values across the resolved scope (lowercased).
    pub scoped_ids: Option<&'a HashSet<String>>,
    /// Optional map of known `:ID:` to defining file paths for duplicate detection.
    pub id_locations: Option<&'a HashMap<String, Vec<PathBuf>>>,
}

impl<'a> LintContext<'a> {
    /// Creates a new single-file lint context without extra workspace documents.
    pub fn new(source: &'a SourceFile, config: &'a Config) -> Self {
        Self {
            source,
            config,
            workspace_docs: None,
            scoped_ids: None,
            id_locations: None,
        }
    }

    /// Creates a new lint context backed by `workspace_docs` for cross-file checks.
    pub fn with_workspace(
        source: &'a SourceFile,
        config: &'a Config,
        workspace_docs: &'a [OrgDocument],
    ) -> Self {
        Self {
            source,
            config,
            workspace_docs: Some(workspace_docs),
            scoped_ids: None,
            id_locations: None,
        }
    }

    /// Creates a new lint context backed by precomputed scope identifiers and locations.
    pub fn with_scope(
        source: &'a SourceFile,
        config: &'a Config,
        workspace_docs: Option<&'a [OrgDocument]>,
        scoped_ids: Option<&'a HashSet<String>>,
        id_locations: Option<&'a HashMap<String, Vec<PathBuf>>>,
    ) -> Self {
        Self {
            source,
            config,
            workspace_docs,
            scoped_ids,
            id_locations,
        }
    }
}

/// A rule that produces [`Fix`] values to auto-correct formatting issues.
///
/// Rule IDs use the scheme `F` + 3-digit number (e.g., `F001`).
/// Implementations must be stateless and thread-safe.
pub trait FormatRule: Send + Sync {
    /// Unique rule identifier (e.g., `"F001"`).
    fn id(&self) -> &'static str;
    /// Kebab-case rule name (e.g., `"trailing-whitespace"`).
    fn name(&self) -> &'static str;
    /// Human-readable one-line description of what this rule does.
    fn description(&self) -> &'static str;
    /// Scans the source and returns fixes for all formatting issues found.
    fn format(&self, ctx: &FormatContext) -> Vec<Fix>;
}

/// A rule that inspects org source and reports [`Diagnostic`] values.
///
/// Rule IDs use the scheme:
/// - `E` + 3-digit number for errors (structural issues)
/// - `W` + 3-digit number for warnings (style/correctness)
/// - `I` + 3-digit number for info-level suggestions
///
/// Implementations must be stateless and thread-safe.
pub trait LintRule: Send + Sync {
    /// Unique rule identifier (e.g., `"W001"`).
    fn id(&self) -> &'static str;
    /// Kebab-case rule name (e.g., `"heading-level-gap"`).
    fn name(&self) -> &'static str;
    /// Human-readable one-line description of what this rule checks.
    fn description(&self) -> &'static str;
    /// Returns `true` if this rule is active under the given configuration.
    fn is_enabled(&self, config: &Config) -> bool {
        !config.is_rule_disabled(self.id(), self.name())
    }
    /// Scans the source and returns diagnostics for all issues found.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic>;
}
