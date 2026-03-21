use std::path::PathBuf;

/// A byte range in the source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
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

/// A suggested fix: replace `span` with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub span: Span,
    pub replacement: String,
}

impl Fix {
    pub fn new(span: Span, replacement: String) -> Self {
        Self { span, replacement }
    }
}

/// A diagnostic message produced by a rule.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub file: PathBuf,
    pub line: usize,
    pub column: usize,
    pub severity: Severity,
    /// Unique rule identifier (e.g., "W001", "E001", "F001").
    pub rule_id: &'static str,
    /// Kebab-case rule name (e.g., "heading-level-gap").
    pub rule: &'static str,
    pub message: String,
    pub fix: Option<Fix>,
}
