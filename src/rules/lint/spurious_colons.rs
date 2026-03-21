/// Detects empty tag strings (double colons `::`) on headlines.
///
/// Spec: [§6.2 Setting Tags](https://orgmode.org/manual/Setting-Tags.html)
/// org-lint: `spurious-colons`
///
/// Tags should be `:tag1:tag2:` — `::` indicates an empty tag between colons.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct SpuriousColons;

fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('*') {
        return false;
    }
    let after = trimmed.trim_start_matches('*');
    after.starts_with(' ') || after.is_empty()
}

impl LintRule for SpuriousColons {
    fn id(&self) -> &'static str {
        "W012"
    }

    fn name(&self) -> &'static str {
        "spurious-colons"
    }

    fn description(&self) -> &'static str {
        "Detect empty tags (::) in heading tag strings"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_heading(raw) {
                let trimmed = raw.trim_end();
                // Tags are at the end of the line, matching :[word]:
                if trimmed.ends_with(':') {
                    // Find the tag string (last sequence of :tag:tag:).
                    if let Some(tag_start) = trimmed.rfind(" :") {
                        let tag_str = &trimmed[tag_start + 1..];
                        if tag_str.contains("::") {
                            let (line_num, _) = ctx.source.line_col(offset);
                            diagnostics.push(Diagnostic {
                                file: ctx.source.path.clone(),
                                line: line_num,
                                column: 1,
                                severity: Severity::Warning,
                                rule_id: self.id(),
                                rule: self.name(),
                                message: "heading has empty tag (spurious :: in tag string)"
                                    .to_string(),
                                fix: None,
                            });
                        }
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
        SpuriousColons.check(&ctx)
    }

    #[test]
    fn valid_tags() {
        assert!(check_it("* Heading :tag1:tag2:\n").is_empty());
    }

    #[test]
    fn spurious_double_colon() {
        let diags = check_it("* Heading :tag1::tag2:\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("spurious"));
    }

    #[test]
    fn no_tags() {
        assert!(check_it("* Heading\n").is_empty());
    }

    #[test]
    fn not_a_heading() {
        assert!(check_it("text with :: in it\n").is_empty());
    }
}
