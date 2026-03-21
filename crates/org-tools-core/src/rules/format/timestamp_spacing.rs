// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::diagnostic::{Fix, Span};
use crate::rules::{FormatContext, FormatRule};

/// Normalizes spacing in planning lines (`SCHEDULED`, `DEADLINE`, `CLOSED`).
///
/// Spec: [Deadlines and Scheduling](https://orgmode.org/manual/Deadlines-and-Scheduling.html),
/// [Planning](https://orgmode.org/worg/org-syntax.html#Planning)
///
/// Ensures exactly one space between the planning keyword colon and the
/// timestamp. Handles lines with multiple planning keywords on the same
/// line (e.g., `SCHEDULED: <...> DEADLINE: <...>`).
///
/// Examples:
/// - `SCHEDULED:  <2024-01-15 Mon>` becomes `SCHEDULED: <2024-01-15 Mon>`
/// - `DEADLINE:<2024-02-01 Thu>` becomes `DEADLINE: <2024-02-01 Thu>`
///
/// Rule ID: `F009`
pub struct TimestampSpacing;

const PLANNING_KEYWORDS: &[&str] = &["SCHEDULED:", "DEADLINE:", "CLOSED:"];

impl FormatRule for TimestampSpacing {
    fn id(&self) -> &'static str {
        "F009"
    }

    fn name(&self) -> &'static str {
        "timestamp-spacing"
    }

    fn description(&self) -> &'static str {
        "Normalize spacing in SCHEDULED/DEADLINE/CLOSED lines"
    }

    fn format(&self, ctx: &FormatContext) -> Vec<Fix> {
        let content = &ctx.source.content;
        let mut fixes = Vec::new();
        let mut offset = 0;

        for line in content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            // Check if this line contains any planning keyword.
            let mut has_planning = false;
            for kw in PLANNING_KEYWORDS {
                if trimmed.contains(kw) {
                    has_planning = true;
                    break;
                }
            }

            if has_planning {
                let mut new_line = raw.to_string();
                let mut changed = false;

                for kw in PLANNING_KEYWORDS {
                    if let Some(kw_pos) = new_line.find(kw) {
                        let after_kw = &new_line[kw_pos + kw.len()..];
                        let spaces = after_kw.len() - after_kw.trim_start().len();
                        if spaces != 1 && !after_kw.trim().is_empty() {
                            // Replace the spaces after this keyword with exactly one.
                            let replace_start = kw_pos + kw.len();
                            let replace_end = replace_start + spaces;
                            new_line = format!(
                                "{} {}",
                                &new_line[..replace_start],
                                &new_line[replace_end..]
                            );
                            changed = true;
                        }
                    }
                }

                if changed {
                    fixes.push(Fix::new(
                        Span::new(offset, offset + raw.len()),
                        new_line,
                    ));
                }
            }

            offset += line.len() + 1;
        }

        fixes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::formatter::apply_fixes;
    use crate::source::SourceFile;

    fn format_it(input: &str) -> String {
        let source = SourceFile::new("test.org", input.to_string());
        let config = Config::default();
        let ctx = FormatContext::new(&source, &config);
        let fixes = TimestampSpacing.format(&ctx);
        apply_fixes(input, &fixes)
    }

    #[test]
    fn already_correct() {
        assert_eq!(
            format_it("SCHEDULED: <2024-01-15 Mon>\n"),
            "SCHEDULED: <2024-01-15 Mon>\n"
        );
    }

    #[test]
    fn no_space() {
        assert_eq!(
            format_it("SCHEDULED:<2024-01-15 Mon>\n"),
            "SCHEDULED: <2024-01-15 Mon>\n"
        );
    }

    #[test]
    fn extra_spaces() {
        assert_eq!(
            format_it("SCHEDULED:   <2024-01-15 Mon>\n"),
            "SCHEDULED: <2024-01-15 Mon>\n"
        );
    }

    #[test]
    fn deadline() {
        assert_eq!(
            format_it("DEADLINE:  <2024-02-01 Thu>\n"),
            "DEADLINE: <2024-02-01 Thu>\n"
        );
    }

    #[test]
    fn both_on_same_line() {
        assert_eq!(
            format_it("SCHEDULED:  <2024-01-15 Mon> DEADLINE:  <2024-02-01 Thu>\n"),
            "SCHEDULED: <2024-01-15 Mon> DEADLINE: <2024-02-01 Thu>\n"
        );
    }

    #[test]
    fn not_a_planning_line() {
        let input = "regular text\n";
        assert_eq!(format_it(input), input);
    }
}
