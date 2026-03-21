// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

/// A byte range in the source text, representing a contiguous region to replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl Span {
    /// Creates a new span from `start` (inclusive) to `end` (exclusive).
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// Severity level for a diagnostic message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational suggestion (I-series rules).
    Info,
    /// Correctness or style issue (W-series rules).
    Warning,
    /// Structural problem that likely breaks parsing (E-series rules).
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
        }
    }
}

/// A suggested fix: replace the bytes covered by `span` with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// The byte range in the original source to replace.
    pub span: Span,
    /// The replacement text (may be empty for deletions).
    pub replacement: String,
}

impl Fix {
    /// Creates a new fix that replaces `span` with `replacement`.
    pub fn new(span: Span, replacement: String) -> Self {
        Self { span, replacement }
    }
}

/// A diagnostic message produced by a lint or format rule.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Path to the source file that produced this diagnostic.
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column number.
    pub column: usize,
    /// Severity level of the diagnostic.
    pub severity: Severity,
    /// Unique rule identifier (e.g., `"W001"`, `"E001"`, `"F001"`).
    pub rule_id: &'static str,
    /// Kebab-case rule name (e.g., `"heading-level-gap"`).
    pub rule: &'static str,
    /// Human-readable description of the issue.
    pub message: String,
    /// Optional auto-fix for this diagnostic (present only for format rules).
    pub fix: Option<Fix>,
}
