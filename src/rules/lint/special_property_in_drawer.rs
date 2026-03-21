/// Detects computed/special properties manually set in PROPERTIES drawers.
///
/// Spec: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
/// org-lint: `special-property-in-properties-drawer`
///
/// These properties are computed by org-mode and setting them manually in a
/// PROPERTIES drawer has no effect. They should be removed.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct SpecialPropertyInDrawer;

/// Properties that are computed by org-mode and should not be set manually.
const SPECIAL_PROPERTIES: &[&str] = &[
    "ALLTAGS",
    "BLOCKED",
    "CATEGORY",
    "CLOCKSUM",
    "CLOCKSUM_T",
    "CLOSED",
    "DEADLINE",
    "FILE",
    "ITEM",
    "PRIORITY",
    "SCHEDULED",
    "TAGS",
    "TIMESTAMP",
    "TIMESTAMP_IA",
    "TODO",
];

impl LintRule for SpecialPropertyInDrawer {
    fn id(&self) -> &'static str {
        "W019"
    }

    fn name(&self) -> &'static str {
        "special-property-in-drawer"
    }

    fn description(&self) -> &'static str {
        "Detect computed properties manually set in PROPERTIES drawers"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut in_properties = false;
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            if trimmed == ":PROPERTIES:" {
                in_properties = true;
            } else if trimmed == ":END:" {
                in_properties = false;
            } else if in_properties && trimmed.starts_with(':') {
                // Extract the property key.
                let rest = &trimmed[1..];
                if let Some(colon_pos) = rest.find(':') {
                    let key = &rest[..colon_pos];
                    // Check against special properties (case-insensitive).
                    let upper = key.to_uppercase();
                    if SPECIAL_PROPERTIES.contains(&upper.as_str()) {
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                ":{}: is a computed property — setting it in PROPERTIES has no effect",
                                key
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
        SpecialPropertyInDrawer.check(&ctx)
    }

    #[test]
    fn normal_properties() {
        let input = ":PROPERTIES:\n:ID: abc\n:CUSTOM_ID: foo\n:END:\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn special_category() {
        let diags = check_it(":PROPERTIES:\n:CATEGORY: test\n:END:\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("computed property"));
    }

    #[test]
    fn special_todo() {
        let diags = check_it(":PROPERTIES:\n:TODO: DONE\n:END:\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn outside_drawer_not_flagged() {
        assert!(check_it(":CATEGORY: test\n").is_empty());
    }

    #[test]
    fn multiple_special() {
        let input = ":PROPERTIES:\n:TAGS: :foo:\n:PRIORITY: A\n:ID: ok\n:END:\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 2); // TAGS and PRIORITY
    }
}
