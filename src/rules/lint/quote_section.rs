/// Detects deprecated `QUOTE` prefix in heading titles.
///
/// org-lint: `quote-section`
///
/// Old syntax: `* QUOTE Heading` — this was removed from org-mode.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct QuoteSection;

impl LintRule for QuoteSection {
    fn id(&self) -> &'static str {
        "W018"
    }

    fn name(&self) -> &'static str {
        "quote-section"
    }

    fn description(&self) -> &'static str {
        "Detect deprecated QUOTE heading prefix"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            if trimmed.starts_with('*') {
                let after_stars = trimmed.trim_start_matches('*');
                if after_stars.starts_with(' ') {
                    let title = after_stars.trim_start();
                    if title.starts_with("QUOTE ") || title == "QUOTE" {
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message:
                                "QUOTE heading prefix is deprecated — use a quote block instead"
                                    .to_string(),
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
        QuoteSection.check(&ctx)
    }

    #[test]
    fn normal_heading() {
        assert!(check_it("* Normal Heading\n").is_empty());
    }

    #[test]
    fn quote_heading() {
        let diags = check_it("* QUOTE Some text\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("deprecated"));
    }

    #[test]
    fn quote_in_body_not_flagged() {
        assert!(check_it("QUOTE is just text here\n").is_empty());
    }

    #[test]
    fn heading_containing_quote_word() {
        // "A QUOTE from..." should not be flagged — QUOTE must be the first word.
        assert!(check_it("* A QUOTE from someone\n").is_empty());
    }
}
