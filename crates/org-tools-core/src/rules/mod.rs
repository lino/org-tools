// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rule traits, context structs, and shared parsing utilities.

pub mod format;
pub mod heading;
pub mod lint;
pub mod list;
pub mod timestamp;

use crate::config::Config;
use crate::diagnostic::{Diagnostic, Fix};
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
/// Provides read-only access to the source text and active configuration.
pub struct LintContext<'a> {
    /// The source file being linted.
    pub source: &'a SourceFile,
    /// Active configuration for this run.
    pub config: &'a Config,
}

impl<'a> LintContext<'a> {
    /// Creates a new lint context.
    pub fn new(source: &'a SourceFile, config: &'a Config) -> Self {
        Self { source, config }
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
    /// Scans the source and returns diagnostics for all issues found.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic>;
}
