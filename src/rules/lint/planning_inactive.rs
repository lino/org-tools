/// Detects inactive timestamps in SCHEDULED/DEADLINE planning lines.
///
/// Spec: [§8.3 Deadlines and Scheduling](https://orgmode.org/manual/Deadlines-and-Scheduling.html)
/// org-lint: `planning-inactive`
///
/// SCHEDULED and DEADLINE must use active timestamps `<...>` to appear in the
/// agenda. Inactive timestamps `[...]` will not trigger agenda entries.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct PlanningInactive;

impl LintRule for PlanningInactive {
    fn id(&self) -> &'static str {
        "W014"
    }

    fn name(&self) -> &'static str {
        "planning-inactive"
    }

    fn description(&self) -> &'static str {
        "Detect inactive timestamps in SCHEDULED/DEADLINE"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            // Check SCHEDULED: and DEADLINE: for inactive timestamps.
            for keyword in &["SCHEDULED:", "DEADLINE:"] {
                if let Some(pos) = trimmed.find(keyword) {
                    let after = &trimmed[pos + keyword.len()..];
                    let after_trimmed = after.trim_start();
                    // Inactive timestamps start with [, active with <.
                    if after_trimmed.starts_with('[') && !after_trimmed.starts_with("[[") {
                        let (line_num, _) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: 1,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                "{} uses an inactive timestamp [...] — use <...> for agenda visibility",
                                keyword.trim_end_matches(':')
                            ),
                            fix: None,
                        });
                    }
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
        PlanningInactive.check(&ctx)
    }

    #[test]
    fn active_timestamp() {
        assert!(check_it("SCHEDULED: <2024-01-15 Mon>\n").is_empty());
    }

    #[test]
    fn inactive_scheduled() {
        let diags = check_it("SCHEDULED: [2024-01-15 Mon]\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("SCHEDULED"));
    }

    #[test]
    fn inactive_deadline() {
        let diags = check_it("DEADLINE: [2024-01-15 Mon]\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("DEADLINE"));
    }

    #[test]
    fn closed_not_flagged() {
        // CLOSED uses inactive timestamps by design.
        assert!(check_it("CLOSED: [2024-01-15 Mon 14:30]\n").is_empty());
    }

    #[test]
    fn both_on_same_line() {
        let diags = check_it("SCHEDULED: [2024-01-15 Mon] DEADLINE: [2024-02-01 Thu]\n");
        assert_eq!(diags.len(), 2);
    }
}
