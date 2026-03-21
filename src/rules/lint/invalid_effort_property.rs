/// Detects `:EFFORT:` property values that are not valid duration formats.
///
/// Spec: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
/// org-lint: `invalid-effort-property`
///
/// Valid formats: `HH:MM`, `H:MM`, `MM` (minutes only), or `Xd Xh Xm` style.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct InvalidEffortProperty;

/// Returns true if the value is a valid effort/duration format.
fn is_valid_effort(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    // HH:MM or H:MM format.
    if trimmed.contains(':') {
        let parts: Vec<&str> = trimmed.split(':').collect();
        if parts.len() == 2 {
            return parts[0].parse::<u32>().is_ok() && parts[1].parse::<u32>().is_ok();
        }
        return false;
    }

    // Pure numeric (minutes).
    if trimmed.parse::<u32>().is_ok() {
        return true;
    }

    // Duration with units: "1d 2h 30m", "2h", "30min", etc.
    let has_unit = trimmed.contains('d')
        || trimmed.contains('h')
        || trimmed.contains('m')
        || trimmed.contains('w');
    if has_unit {
        // Rough validation: should be digits followed by unit letters.
        return trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_whitespace() || "dhmswin".contains(c));
    }

    false
}

impl LintRule for InvalidEffortProperty {
    fn id(&self) -> &'static str {
        "W010"
    }

    fn name(&self) -> &'static str {
        "invalid-effort-property"
    }

    fn description(&self) -> &'static str {
        "Detect invalid :EFFORT: duration values"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            if let Some(rest) = trimmed.strip_prefix(":EFFORT:") {
                let value = rest.trim();
                if !value.is_empty() && !is_valid_effort(value) {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!(
                            "invalid :EFFORT: value \"{}\" — expected HH:MM or duration",
                            value
                        ),
                        fix: None,
                    });
                }
            }

            offset += line.len() + 1;
        }

        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        InvalidEffortProperty.check(&ctx)
    }

    #[test]
    fn valid_hhmm() {
        assert!(check_it(":EFFORT: 2:30\n").is_empty());
    }

    #[test]
    fn valid_minutes() {
        assert!(check_it(":EFFORT: 90\n").is_empty());
    }

    #[test]
    fn valid_duration_units() {
        assert!(check_it(":EFFORT: 1h 30m\n").is_empty());
    }

    #[test]
    fn invalid_value() {
        let diags = check_it(":EFFORT: lots of time\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("invalid :EFFORT:"));
    }

    #[test]
    fn empty_effort() {
        assert!(check_it(":EFFORT:\n").is_empty());
    }
}
