/// Detects `#+KEYWORD` lines missing the colon after the keyword name.
///
/// Spec: [Syntax: Keywords](https://orgmode.org/worg/org-syntax.html#Keywords)
/// org-lint: `invalid-keyword-syntax`
///
/// Example: `#+TITLE My Title` should be `#+TITLE: My Title`.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct InvalidKeywordSyntax;

impl LintRule for InvalidKeywordSyntax {
    fn id(&self) -> &'static str {
        "W005"
    }

    fn name(&self) -> &'static str {
        "invalid-keyword-syntax"
    }

    fn description(&self) -> &'static str {
        "Detect #+KEYWORD lines missing the colon"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            // Match lines starting with #+ followed by uppercase letters and then a space
            // (not a colon), which suggests a missing colon.
            if let Some(rest) = trimmed.strip_prefix("#+") {
                // Skip block delimiters, comments, and CALL/RESULTS lines.
                let rest_upper = rest.to_uppercase();
                if rest.starts_with(' ')
                    || rest.is_empty()
                    || rest_upper.starts_with("BEGIN")
                    || rest_upper.starts_with("END")
                    || rest_upper.starts_with("CALL")
                    || rest_upper.starts_with("RESULTS")
                {
                    offset += line.len() + 1;
                    continue;
                }

                // Find where the keyword name ends.
                let keyword_end = rest
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(rest.len());

                if keyword_end > 0 && keyword_end < rest.len() {
                    let after_keyword = rest.as_bytes()[keyword_end];
                    // If the character after the keyword is a space (not a colon), flag it.
                    if after_keyword == b' ' {
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                "#+{} is missing a colon — should be #+{}:",
                                &rest[..keyword_end],
                                &rest[..keyword_end]
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
        InvalidKeywordSyntax.check(&ctx)
    }

    #[test]
    fn valid_keyword() {
        assert!(check_it("#+TITLE: My Title\n").is_empty());
    }

    #[test]
    fn missing_colon() {
        let diags = check_it("#+TITLE My Title\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing a colon"));
    }

    #[test]
    fn block_delimiters_not_flagged() {
        assert!(check_it("#+BEGIN_SRC python\n#+END_SRC\n").is_empty());
    }

    #[test]
    fn no_keywords() {
        assert!(check_it("just text\n").is_empty());
    }

    #[test]
    fn keyword_with_underscore() {
        let diags = check_it("#+STARTUP overview\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn keyword_at_end_of_line() {
        // #+TITLE alone with nothing after — not flagged (could be intentional).
        assert!(check_it("#+TITLE\n").is_empty());
    }
}
