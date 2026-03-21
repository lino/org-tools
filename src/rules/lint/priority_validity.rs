/// Validates `[#X]` priority cookies on headings.
///
/// Spec: [§5.4 Priorities](https://orgmode.org/manual/Priorities.html)
/// org-lint: `priority`
///
/// Priority must be a single uppercase letter A-Z.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::heading::parse_heading;
use crate::rules::{LintContext, LintRule};

pub struct PriorityValidity;

impl LintRule for PriorityValidity {
    fn id(&self) -> &'static str {
        "W030"
    }

    fn name(&self) -> &'static str {
        "priority-validity"
    }

    fn description(&self) -> &'static str {
        "Validate priority cookie format"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if let Some(parts) = parse_heading(raw) {
                if let Some(ch) = parts.priority {
                    // Priority was parsed — check if it's lowercase.
                    if ch.is_ascii_lowercase() {
                        let (line_num, _) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: 1,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                "priority [#{}] should be uppercase [#{}]",
                                ch,
                                ch.to_ascii_uppercase()
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
        PriorityValidity.check(&ctx)
    }

    #[test]
    fn valid_priority() {
        assert!(check_it("* TODO [#A] Task\n").is_empty());
    }

    #[test]
    fn valid_priority_c() {
        assert!(check_it("* [#C] Task\n").is_empty());
    }

    #[test]
    fn lowercase_priority() {
        let diags = check_it("* TODO [#a] Task\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("uppercase"));
    }

    #[test]
    fn no_priority() {
        assert!(check_it("* TODO Task\n").is_empty());
    }

    #[test]
    fn not_a_heading() {
        assert!(check_it("text\n").is_empty());
    }
}
