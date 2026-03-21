use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct MissingSrcLanguage;

impl LintRule for MissingSrcLanguage {
    fn id(&self) -> &'static str {
        "W002"
    }

    fn name(&self) -> &'static str {
        "missing-src-language"
    }

    fn description(&self) -> &'static str {
        "Detect source blocks without a language identifier"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            let is_src_begin = trimmed.eq_ignore_ascii_case("#+begin_src")
                || (trimmed.len() > 11
                    && trimmed[..11].eq_ignore_ascii_case("#+begin_src")
                    && trimmed.as_bytes()[11] == b' ');

            if is_src_begin {
                let after = if trimmed.len() > 11 {
                    trimmed[11..].trim()
                } else {
                    ""
                };

                if after.is_empty() {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "source block is missing a language identifier".to_string(),
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
    use crate::source::SourceFile;
    use crate::config::Config;

    fn check_it(input: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        MissingSrcLanguage.check(&ctx)
    }

    #[test]
    fn with_language() {
        let diags = check_it("#+BEGIN_SRC python\nprint('hi')\n#+END_SRC\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn without_language() {
        let diags = check_it("#+BEGIN_SRC\nsome code\n#+END_SRC\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing a language"));
    }

    #[test]
    fn lowercase() {
        let diags = check_it("#+begin_src\ncode\n#+end_src\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn with_language_lowercase() {
        let diags = check_it("#+begin_src rust\ncode\n#+end_src\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn no_src_blocks() {
        let diags = check_it("just text\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn example_block_ignored() {
        let diags = check_it("#+BEGIN_EXAMPLE\nstuff\n#+END_EXAMPLE\n");
        assert!(diags.is_empty());
    }
}
