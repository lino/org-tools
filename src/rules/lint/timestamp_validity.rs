/// Validates org timestamps: date validity, repeater format, warning delay format.
///
/// Spec: [§8.1 Timestamps](https://orgmode.org/manual/Timestamps.html)
use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::timestamp::{find_timestamps, is_valid_date, is_valid_repeater, is_valid_warning};
use crate::rules::{LintContext, LintRule};

pub struct TimestampValidity;

impl LintRule for TimestampValidity {
    fn id(&self) -> &'static str {
        "W027"
    }

    fn name(&self) -> &'static str {
        "timestamp-validity"
    }

    fn description(&self) -> &'static str {
        "Validate timestamp dates, repeaters, and warning delays"
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

            for (ts, _start, _end) in find_timestamps(raw) {
                let (line_num, _) = ctx.source.line_col(offset);

                if !is_valid_date(ts.year, ts.month, ts.day) {
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: 1,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!(
                            "invalid date {}-{:02}-{:02} in timestamp",
                            ts.year, ts.month, ts.day
                        ),
                        fix: None,
                    });
                }

                if let Some(ref rep) = ts.repeater {
                    if !is_valid_repeater(rep) {
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: 1,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!("invalid repeater '{}' in timestamp", rep),
                            fix: None,
                        });
                    }
                }

                if let Some(ref warn) = ts.warning {
                    if !is_valid_warning(warn) {
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: 1,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: format!("invalid warning delay '{}' in timestamp", warn),
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
        TimestampValidity.check(&ctx)
    }

    #[test]
    fn valid_timestamp() {
        assert!(check_it("<2024-01-15 Mon>\n").is_empty());
    }

    #[test]
    fn invalid_date() {
        let diags = check_it("<2024-02-30 Fri>\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("invalid date"));
    }

    #[test]
    fn invalid_month() {
        let diags = check_it("<2024-13-01 Mon>\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn leap_year_valid() {
        assert!(check_it("<2024-02-29 Thu>\n").is_empty());
    }

    #[test]
    fn non_leap_year_invalid() {
        let diags = check_it("<2023-02-29 Wed>\n");
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn valid_repeater() {
        assert!(check_it("<2024-01-15 Mon +1w>\n").is_empty());
    }

    #[test]
    fn in_code_block_skipped() {
        let input = "#+BEGIN_SRC org\n<2024-13-01 Mon>\n#+END_SRC\n";
        assert!(check_it(input).is_empty());
    }
}
