/// Detects a `]` immediately after a link's closing `]]`, suggesting malformed syntax.
///
/// Spec: [§4.1 Link Format](https://orgmode.org/manual/Link-Format.html)
/// org-lint: `trailing-bracket-after-link`
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct TrailingBracketAfterLink;

impl LintRule for TrailingBracketAfterLink {
    fn id(&self) -> &'static str {
        "W009"
    }

    fn name(&self) -> &'static str {
        "trailing-bracket-after-link"
    }

    fn description(&self) -> &'static str {
        "Detect ] immediately after ]] in links"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            // Search for ]]] pattern — a link closing ]] followed by an extra ].
            let mut search = raw;
            let mut line_offset = 0;
            while let Some(pos) = search.find("]]]") {
                let (line_num, _) = ctx.source.line_col(offset);
                diagnostics.push(Diagnostic {
                    file: ctx.source.path.clone(),
                    line: line_num,
                    column: line_offset + pos + 3,
                    severity: Severity::Warning,
                    rule_id: self.id(),
                    rule: self.name(),
                    message: "extra ] after link closing ]] — possible malformed link".to_string(),
                    fix: None,
                });
                line_offset += pos + 3;
                search = &search[pos + 3..];
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
        TrailingBracketAfterLink.check(&ctx)
    }

    #[test]
    fn valid_link() {
        assert!(check_it("[[https://example.com][text]]\n").is_empty());
    }

    #[test]
    fn trailing_bracket() {
        let diags = check_it("[[target][desc]]]\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("extra ]"));
    }

    #[test]
    fn no_links() {
        assert!(check_it("just text\n").is_empty());
    }
}
