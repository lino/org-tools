/// Detects source blocks with unrecognized language identifiers.
///
/// Spec: [§16.3 Languages](https://orgmode.org/manual/Languages.html)
/// org-lint: `suspicious-language-in-src-block`
///
/// Warns on languages not in the known Babel language list.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct SuspiciousLanguage;

/// Known Babel/org-mode source block languages.
const KNOWN_LANGUAGES: &[&str] = &[
    "asymptote", "awk", "bash", "c", "calc", "clojure", "clojurescript", "comint",
    "cpp", "css", "d", "ditaa", "dot", "elisp", "elixir", "emacs-lisp", "eshell",
    "f90", "forth", "fortran", "gnuplot", "go", "groovy", "haskell", "html",
    "java", "javascript", "js", "json", "julia", "kotlin", "latex", "ledger",
    "lilypond", "lisp", "lua", "makefile", "matlab", "maxima", "mermaid",
    "mscgen", "nix", "objc", "objective-c", "ocaml", "octave", "org", "perl",
    "php", "plantuml", "powershell", "python", "r", "racket", "ruby", "rust",
    "sass", "scala", "scheme", "screen", "sed", "sh", "shell", "sql", "sqlite",
    "swift", "tcl", "tex", "toml", "ts", "typescript", "vala", "xml", "yaml",
    "zsh",
];

impl LintRule for SuspiciousLanguage {
    fn id(&self) -> &'static str {
        "I001"
    }

    fn name(&self) -> &'static str {
        "suspicious-language"
    }

    fn description(&self) -> &'static str {
        "Detect unrecognized source block languages"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim_start();
            let is_src_begin = trimmed.len() > 11
                && trimmed[..11].eq_ignore_ascii_case("#+begin_src")
                && trimmed.as_bytes().get(11) == Some(&b' ');

            if is_src_begin {
                let lang = trimmed[12..].split_whitespace().next().unwrap_or("");
                if !lang.is_empty() {
                    let lower = lang.to_lowercase();
                    if !KNOWN_LANGUAGES.contains(&lower.as_str()) {
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Info,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!(
                                "unrecognized source block language '{}'",
                                lang
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
        SuspiciousLanguage.check(&ctx)
    }

    #[test]
    fn known_language() {
        assert!(check_it("#+BEGIN_SRC python\ncode\n#+END_SRC\n").is_empty());
    }

    #[test]
    fn known_language_rust() {
        assert!(check_it("#+BEGIN_SRC rust\ncode\n#+END_SRC\n").is_empty());
    }

    #[test]
    fn unknown_language() {
        let diags = check_it("#+BEGIN_SRC foobar\ncode\n#+END_SRC\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Info);
        assert!(diags[0].message.contains("foobar"));
    }

    #[test]
    fn no_language_not_flagged() {
        // missing-src-language handles this case.
        assert!(check_it("#+BEGIN_SRC\ncode\n#+END_SRC\n").is_empty());
    }

    #[test]
    fn case_insensitive() {
        assert!(check_it("#+BEGIN_SRC Python\ncode\n#+END_SRC\n").is_empty());
    }
}
