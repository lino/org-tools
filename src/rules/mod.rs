pub mod format;
pub mod heading;
pub mod lint;
pub mod list;
pub mod timestamp;

use crate::config::Config;
use crate::diagnostic::{Diagnostic, Fix};
use crate::source::SourceFile;

/// Context for format rules: raw text with line-level access + config.
pub struct FormatContext<'a> {
    pub source: &'a SourceFile,
    pub config: &'a Config,
}

impl<'a> FormatContext<'a> {
    pub fn new(source: &'a SourceFile, config: &'a Config) -> Self {
        Self { source, config }
    }
}

/// Context for lint rules: source text + config.
pub struct LintContext<'a> {
    pub source: &'a SourceFile,
    pub config: &'a Config,
}

impl<'a> LintContext<'a> {
    pub fn new(source: &'a SourceFile, config: &'a Config) -> Self {
        Self { source, config }
    }
}

/// A rule that produces fixes to format org source text.
///
/// Rule IDs use the scheme `F` + 3-digit number (e.g., `F001`).
pub trait FormatRule: Send + Sync {
    /// Unique rule identifier (e.g., "F001").
    fn id(&self) -> &'static str;
    /// Kebab-case name (e.g., "trailing-whitespace").
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn format(&self, ctx: &FormatContext) -> Vec<Fix>;
}

/// A rule that inspects org source and reports diagnostics.
///
/// Rule IDs use the scheme:
/// - `E` + 3-digit number for errors (structural issues)
/// - `W` + 3-digit number for warnings (style/correctness)
/// - `I` + 3-digit number for info (suggestions)
pub trait LintRule: Send + Sync {
    /// Unique rule identifier (e.g., "W001").
    fn id(&self) -> &'static str;
    /// Kebab-case name (e.g., "heading-level-gap").
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic>;
}
