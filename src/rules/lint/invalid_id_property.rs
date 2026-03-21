/// Detects `:ID:` property values containing `::` (search string delimiter).
///
/// Spec: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
/// org-lint: `invalid-id-property`
///
/// An ID with `::` would be interpreted as containing a search string,
/// making it unusable for linking.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct InvalidIdProperty;

impl LintRule for InvalidIdProperty {
    fn id(&self) -> &'static str {
        "W011"
    }

    fn name(&self) -> &'static str {
        "invalid-id-property"
    }

    fn description(&self) -> &'static str {
        "Detect :ID: values containing :: (search string delimiter)"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            if let Some(rest) = trimmed.strip_prefix(":ID:") {
                let value = rest.trim();
                if value.contains("::") {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!(
                            ":ID: value \"{}\" contains :: which is interpreted as a search delimiter",
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
        InvalidIdProperty.check(&ctx)
    }

    #[test]
    fn valid_id() {
        assert!(check_it(":ID: 550e8400-e29b-41d4-a716-446655440000\n").is_empty());
    }

    #[test]
    fn id_with_double_colon() {
        let diags = check_it(":ID: some::value\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("::"));
    }

    #[test]
    fn no_id() {
        assert!(check_it(":CUSTOM_ID: foo\n").is_empty());
    }
}
