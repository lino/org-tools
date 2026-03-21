/// Detects drawers without a matching `:END:`.
///
/// Spec: [§2.8 Drawers](https://orgmode.org/manual/Drawers.html)
/// org-lint: `incomplete-drawer`
///
/// A drawer opens with `:NAME:` (where NAME is all uppercase letters/digits/hyphens)
/// and must close with `:END:`. An unclosed drawer is an error.
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::{LintContext, LintRule};

pub struct IncompleteDrawer;

/// Returns the drawer name if the line opens a drawer.
fn drawer_open_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with(':') || !trimmed.ends_with(':') || trimmed.len() < 3 {
        return None;
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    // Drawer names are word characters (letters, digits, hyphens, underscores).
    if inner.is_empty() || inner.eq_ignore_ascii_case("END") {
        return None;
    }
    // Must be uppercase alphanumeric, hyphens, or underscores.
    if inner
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some(inner)
    } else {
        None
    }
}

impl LintRule for IncompleteDrawer {
    fn id(&self) -> &'static str {
        "E006"
    }

    fn name(&self) -> &'static str {
        "incomplete-drawer"
    }

    fn description(&self) -> &'static str {
        "Detect drawers without matching :END:"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);
        let mut open_drawer: Option<(String, usize, usize)> = None; // (name, line, col)
        let mut offset = 0;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);

            // Skip lines inside protected regions.
            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            let trimmed = raw.trim();

            if trimmed.eq_ignore_ascii_case(":END:") {
                open_drawer = None;
            } else if open_drawer
                .as_ref()
                .is_some_and(|(name, _, _)| name == "PROPERTIES")
            {
                // Inside PROPERTIES drawer, skip property lines (:KEY: value).
                // Don't treat them as drawer openings.
            } else if let Some(name) = drawer_open_name(raw) {
                // If there's already an open drawer, it's incomplete.
                if let Some((prev_name, prev_line, prev_col)) = open_drawer.take() {
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: prev_line,
                        column: prev_col,
                        severity: Severity::Error,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!(":{}:  drawer is never closed with :END:", prev_name),
                        fix: None,
                    });
                }
                let (line_num, col) = ctx.source.line_col(offset);
                open_drawer = Some((name.to_string(), line_num, col));
            }

            offset += line.len() + 1;
        }

        // Handle drawer still open at end of file.
        if let Some((name, line_num, col)) = open_drawer {
            diagnostics.push(Diagnostic {
                file: ctx.source.path.clone(),
                line: line_num,
                column: col,
                severity: Severity::Error,
                rule_id: self.id(),
                rule: self.name(),
                message: format!(":{}:  drawer is never closed with :END:", name),
                fix: None,
            });
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
        IncompleteDrawer.check(&ctx)
    }

    #[test]
    fn complete_drawer() {
        assert!(check_it(":PROPERTIES:\n:ID: abc\n:END:\n").is_empty());
    }

    #[test]
    fn incomplete_drawer() {
        let diags = check_it(":LOGBOOK:\nCLOCK: ...\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("never closed"));
    }

    #[test]
    fn multiple_complete_drawers() {
        let input = ":PROPERTIES:\n:END:\n:LOGBOOK:\n:END:\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn drawer_in_code_block_ignored() {
        let input = "#+BEGIN_SRC org\n:FAKE:\nno end\n#+END_SRC\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn not_a_drawer() {
        // Lines like `:not a drawer:` with spaces are not drawer syntax.
        assert!(check_it("text with :colons: in it\n").is_empty());
    }
}
