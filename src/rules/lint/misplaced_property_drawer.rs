use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::{LintContext, LintRule};

pub struct MisplacedPropertyDrawer;

fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('*') {
        return false;
    }
    let after = trimmed.trim_start_matches('*');
    after.starts_with(' ') || after.is_empty()
}

fn is_planning_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

impl LintRule for MisplacedPropertyDrawer {
    fn id(&self) -> &'static str {
        "W003"
    }

    fn name(&self) -> &'static str {
        "misplaced-property-drawer"
    }

    fn description(&self) -> &'static str {
        "Detect property drawers not directly after headings"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let lines: Vec<&str> = ctx.source.content.split('\n').collect();
        let mut offset = 0;

        for (i, &line) in lines.iter().enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            if raw.trim() == ":PROPERTIES:" {
                // Check if this drawer is properly placed after a heading.
                let mut valid = false;

                if i == 0 {
                    // Top-level properties before any heading — acceptable.
                    valid = true;
                } else {
                    // Walk backwards to find the nearest heading.
                    let mut j = i - 1;
                    loop {
                        let prev = lines[j].strip_suffix('\r').unwrap_or(lines[j]);
                        let prev_trimmed = prev.trim();

                        if is_heading(prev_trimmed) {
                            valid = true;
                            break;
                        } else if is_planning_line(prev_trimmed) {
                            // Planning lines between heading and properties are OK.
                            if j == 0 {
                                break;
                            }
                            j -= 1;
                        } else if prev_trimmed.is_empty() {
                            // Blank line between heading and properties — not valid.
                            break;
                        } else {
                            // Some other content between heading and properties — not valid.
                            break;
                        }
                    }
                }

                if !valid {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: "property drawer is not directly after a heading".to_string(),
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
        MisplacedPropertyDrawer.check(&ctx)
    }

    #[test]
    fn properly_placed() {
        let input = "* Heading\n:PROPERTIES:\n:ID: abc\n:END:\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn with_planning_line() {
        let input = "* TODO Task\nSCHEDULED: <2024-01-01>\n:PROPERTIES:\n:ID: abc\n:END:\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn misplaced_after_text() {
        let input = "* Heading\nSome text\n:PROPERTIES:\n:ID: abc\n:END:\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("not directly after a heading"));
    }

    #[test]
    fn misplaced_after_blank_line() {
        let input = "* Heading\n\n:PROPERTIES:\n:ID: abc\n:END:\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn top_level_properties() {
        let input = ":PROPERTIES:\n:ID: file-level\n:END:\n* Heading\n";
        assert!(check_it(input).is_empty());
    }
}
