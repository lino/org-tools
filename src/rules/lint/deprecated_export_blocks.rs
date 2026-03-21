/// Detects old-style export blocks like `#+BEGIN_HTML` and suggests `#+BEGIN_EXPORT html`.
///
/// Spec: [§2.6 Blocks](https://orgmode.org/manual/Blocks.html)
/// org-lint: `deprecated-export-blocks`
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct DeprecatedExportBlocks;

const DEPRECATED_BACKENDS: &[&str] = &[
    "ASCII", "BEAMER", "HTML", "LATEX", "MAN", "MARKDOWN", "MD", "ODT", "ORG", "TEXINFO",
];

impl LintRule for DeprecatedExportBlocks {
    fn id(&self) -> &'static str {
        "W007"
    }

    fn name(&self) -> &'static str {
        "deprecated-export-blocks"
    }

    fn description(&self) -> &'static str {
        "Detect old-style #+BEGIN_HTML blocks (use #+BEGIN_EXPORT html)"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();

            if let Some(rest) = trimmed
                .to_uppercase()
                .strip_prefix("#+BEGIN_")
                .map(|r| r.to_string())
            {
                let block_type = rest.split_whitespace().next().unwrap_or("");
                if DEPRECATED_BACKENDS.contains(&block_type) {
                    let (line_num, col) = ctx.source.line_col(offset);
                    let lower = block_type.to_lowercase();
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!(
                            "#+BEGIN_{} is deprecated — use #+BEGIN_EXPORT {} instead",
                            block_type, lower
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
        DeprecatedExportBlocks.check(&ctx)
    }

    #[test]
    fn valid_export_block() {
        assert!(check_it("#+BEGIN_EXPORT html\n<div></div>\n#+END_EXPORT\n").is_empty());
    }

    #[test]
    fn deprecated_html_block() {
        let diags = check_it("#+BEGIN_HTML\n<div></div>\n#+END_HTML\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("#+BEGIN_EXPORT html"));
    }

    #[test]
    fn deprecated_latex_block() {
        let diags = check_it("#+BEGIN_LATEX\n\\section{}\n#+END_LATEX\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn src_block_not_flagged() {
        assert!(check_it("#+BEGIN_SRC python\ncode\n#+END_SRC\n").is_empty());
    }

    #[test]
    fn quote_block_not_flagged() {
        assert!(check_it("#+BEGIN_QUOTE\ntext\n#+END_QUOTE\n").is_empty());
    }
}
