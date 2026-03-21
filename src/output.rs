use crate::diagnostic::Diagnostic;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

pub fn render_diagnostics(diagnostics: &[Diagnostic], format: OutputFormat) -> String {
    match format {
        OutputFormat::Human => render_human(diagnostics),
        OutputFormat::Json => render_json(diagnostics),
    }
}

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

#[derive(Serialize)]
struct JsonDiagnostic {
    file: String,
    line: usize,
    column: usize,
    severity: String,
    rule_id: String,
    rule: String,
    message: String,
    fixable: bool,
}

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
