// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::Diagnostic;
use serde::Serialize;

/// Output format for rendering [`Diagnostic`] messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// Human-readable format: `file:line:col: severity [id/name] message`.
    Human,
    /// Machine-readable JSON array of diagnostic objects.
    Json,
}

/// Renders a slice of [`Diagnostic`] values into a string using the given format.
pub fn render_diagnostics(diagnostics: &[Diagnostic], format: OutputFormat) -> String {
    match format {
        OutputFormat::Human => render_human(diagnostics),
        OutputFormat::Json => render_json(diagnostics),
    }
}

/// Renders diagnostics in human-readable `file:line:col: severity [id/name] message` format.
fn render_human(diagnostics: &[Diagnostic]) -> String {
    let mut out = String::new();
    for d in diagnostics {
        out.push_str(&format!(
            "{}:{}:{}: {} [{}/{}] {}\n",
            d.file.display(),
            d.line,
            d.column,
            d.severity,
            d.rule_id,
            d.rule,
            d.message,
        ));
    }
    out
}

/// Serialisable representation of a [`Diagnostic`] for JSON output.
#[derive(Serialize)]
struct JsonDiagnostic {
    /// Source file path.
    file: String,
    /// 1-based line number.
    line: usize,
    /// 1-based column number.
    column: usize,
    /// Severity as a lowercase string (`"info"`, `"warning"`, `"error"`).
    severity: String,
    /// Unique rule identifier (e.g., `"W001"`).
    rule_id: String,
    /// Kebab-case rule name (e.g., `"heading-level-gap"`).
    rule: String,
    /// Human-readable issue description.
    message: String,
    /// Whether an auto-fix is available for this diagnostic.
    fixable: bool,
}

/// Renders diagnostics as a pretty-printed JSON array.
fn render_json(diagnostics: &[Diagnostic]) -> String {
    let items: Vec<_> = diagnostics
        .iter()
        .map(|d| JsonDiagnostic {
            file: d.file.display().to_string(),
            line: d.line,
            column: d.column,
            severity: d.severity.to_string(),
            rule_id: d.rule_id.to_string(),
            rule: d.rule.to_string(),
            message: d.message.clone(),
            fixable: d.fix.is_some(),
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;
    use std::path::PathBuf;

    fn sample_diagnostic() -> Diagnostic {
        Diagnostic {
            file: PathBuf::from("test.org"),
            line: 1,
            column: 10,
            severity: Severity::Warning,
            rule_id: "F001",
            rule: "trailing-whitespace",
            message: "trailing whitespace".to_string(),
            fix: None,
        }
    }

    #[test]
    fn human_format() {
        let d = sample_diagnostic();
        let out = render_diagnostics(&[d], OutputFormat::Human);
        assert!(out.contains("test.org:1:10"));
        assert!(out.contains("warning"));
        assert!(out.contains("[F001/trailing-whitespace]"));
    }

    #[test]
    fn json_format() {
        let d = sample_diagnostic();
        let out = render_diagnostics(&[d], OutputFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed[0]["rule"], "trailing-whitespace");
    }
}
