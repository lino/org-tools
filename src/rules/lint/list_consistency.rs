/// Detects mixed list markers at the same indentation level within a contiguous list.
///
/// Spec: [§2.7 Plain Lists](https://orgmode.org/manual/Plain-Lists.html)
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::list::{parse_list_item, ListMarker};
use crate::rules::{LintContext, LintRule};

pub struct ListConsistency;

/// Returns the specific marker character for unordered markers.
fn unordered_char(m: &ListMarker) -> Option<char> {
    match m {
        ListMarker::Dash => Some('-'),
        ListMarker::Plus => Some('+'),
        ListMarker::Star => Some('*'),
        _ => None,
    }
}

impl LintRule for ListConsistency {
    fn id(&self) -> &'static str {
        "W029"
    }

    fn name(&self) -> &'static str {
        "list-consistency"
    }

    fn description(&self) -> &'static str {
        "Detect mixed list markers at the same indentation level"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);

        // Track the first marker seen at each indent level within a contiguous list.
        let mut level_markers: std::collections::HashMap<usize, char> =
            std::collections::HashMap::new();
        let mut in_list = false;
        let mut offset = 0;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            if let Some(item) = parse_list_item(raw) {
                if !in_list {
                    level_markers.clear();
                    in_list = true;
                }

                if let Some(ch) = unordered_char(&item.marker) {
                    if let Some(&first_ch) = level_markers.get(&item.indent) {
                        if ch != first_ch {
                            let (line_num, _) = ctx.source.line_col(offset);
                            diagnostics.push(Diagnostic {
                                file: ctx.source.path.clone(),
                                line: line_num,
                                column: 1,
                                severity: Severity::Warning,
                                rule_id: self.id(),
                                rule: self.name(),
                                message: format!(
                                    "mixed list markers at indent {}: '{}' vs '{}' used earlier",
                                    item.indent, ch, first_ch
                                ),
                                fix: None,
                            });
                        }
                    } else {
                        level_markers.insert(item.indent, ch);
                    }
                }
            } else {
                // Non-list line. A blank line or non-list content ends the list context.
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    // A blank line ends the current list group.
                    in_list = false;
                } else if !trimmed.starts_with("#+") && !trimmed.starts_with(':') {
                    // Continuation text is OK, but non-list non-continuation resets.
                    // Keep in_list true for continuation lines.
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
        ListConsistency.check(&ctx)
    }

    #[test]
    fn consistent_markers() {
        assert!(check_it("- Item 1\n- Item 2\n- Item 3\n").is_empty());
    }

    #[test]
    fn mixed_markers() {
        let diags = check_it("- Item 1\n+ Item 2\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("mixed list markers"));
    }

    #[test]
    fn different_levels_ok() {
        // Different indent levels can use different markers.
        assert!(check_it("- Item 1\n  + Nested\n- Item 2\n").is_empty());
    }

    #[test]
    fn separate_lists_ok() {
        // Blank line separates lists — each can have different markers.
        assert!(check_it("- Item 1\n\n+ Item 2\n").is_empty());
    }

    #[test]
    fn no_list() {
        assert!(check_it("just text\n").is_empty());
    }
}
