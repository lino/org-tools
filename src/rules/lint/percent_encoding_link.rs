/// Detects obsolete percent-encoding in bracket links.
///
/// Spec: [§4.1 Link Format](https://orgmode.org/manual/Link-Format.html)
/// org-lint: `percent-encoding-link-escape`
///
/// Only `%25` (`%`), `%5B` (`[`), `%5D` (`]`), and `%20` (space) are valid
/// percent escapes in org bracket links. Other percent-encoded chars are obsolete.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

pub struct PercentEncodingLink;

/// Returns true if the percent encoding is one of the allowed ones.
fn is_allowed_encoding(code: &str) -> bool {
    let upper = code.to_uppercase();
    matches!(upper.as_str(), "25" | "5B" | "5D" | "20")
}

impl LintRule for PercentEncodingLink {
    fn id(&self) -> &'static str {
        "W025"
    }

    fn name(&self) -> &'static str {
        "percent-encoding-link"
    }

    fn description(&self) -> &'static str {
        "Detect obsolete percent-encoding in bracket links"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);
        let mut offset = 0;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            // Find bracket links [[...]] and check for percent-encoding.
            let mut search = raw;
            while let Some(link_start) = search.find("[[") {
                let rest = &search[link_start + 2..];
                let link_end = rest.find("]]").unwrap_or(rest.len());
                let link_content = &rest[..link_end];

                // Check for %XX patterns in the link target (before ][ if present).
                let target = if let Some(desc_start) = link_content.find("][") {
                    &link_content[..desc_start]
                } else {
                    link_content
                };

                let mut check = target;
                while let Some(pct_pos) = check.find('%') {
                    if pct_pos + 2 < check.len() {
                        let code = &check[pct_pos + 1..pct_pos + 3];
                        if code.chars().all(|c| c.is_ascii_hexdigit())
                            && !is_allowed_encoding(code)
                        {
                            let (line_num, _) = ctx.source.line_col(offset);
                            diagnostics.push(Diagnostic {
                                file: ctx.source.path.clone(),
                                line: line_num,
                                column: 1,
                                severity: Severity::Warning,
                                rule_id: self.id(),
                                rule: self.name(),
                                message: format!(
                                    "obsolete percent-encoding %{} in link — only %25, %5B, %5D, %20 are valid",
                                    code
                                ),
                                fix: None,
                            });
                            // Only report once per link.
                            break;
                        }
                    }
                    check = &check[pct_pos + 1..];
                }

                search = &rest[link_end..];
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
        PercentEncodingLink.check(&ctx)
    }

    #[test]
    fn no_encoding() {
        assert!(check_it("[[https://example.com]]\n").is_empty());
    }

    #[test]
    fn allowed_encoding() {
        assert!(check_it("[[file:my%20file.org]]\n").is_empty());
        assert!(check_it("[[file:path%5Bname%5D]]\n").is_empty());
    }

    #[test]
    fn obsolete_encoding() {
        let diags = check_it("[[file:path%2Fto%2Ffile]]\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("%2F"));
    }

    #[test]
    fn no_links() {
        assert!(check_it("just text\n").is_empty());
    }
}
