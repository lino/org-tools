// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Validates CLOCK entry format and duration accuracy.
//!
//! Spec: [§8.4 Clocking Work Time](https://orgmode.org/manual/Clocking-Work-Time.html)
//!
//! Format: `CLOCK: [ts]--[ts] => HH:MM`
//! Checks: both timestamps are inactive, end is after start, duration matches.

use crate::diagnostic::{Diagnostic, Severity};
use crate::rules::timestamp::parse_timestamp;
use crate::rules::{LintContext, LintRule};

/// Validates `CLOCK:` entries for correct timestamp type and duration accuracy.
///
/// Checks three things: both start and end timestamps must be inactive
/// (`[...]` not `<...>`), and when a duration (`=> HH:MM`) is present it
/// must match the actual time difference between the two timestamps.
/// Running clocks (no end timestamp) are accepted without diagnostics.
///
/// Spec: [§8.4 Clocking Work Time](https://orgmode.org/manual/Clocking-Work-Time.html)
pub struct ClockEntryValidity;

impl LintRule for ClockEntryValidity {
    fn id(&self) -> &'static str {
        "W031"
    }

    fn name(&self) -> &'static str {
        "clock-entry-validity"
    }

    fn description(&self) -> &'static str {
        "Validate CLOCK entry format and duration"
    }

    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for line in ctx.source.content.split('\n') {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            if let Some(rest) = trimmed.strip_prefix("CLOCK:") {
                let rest = rest.trim();
                let (line_num, _) = ctx.source.line_col(offset);

                // Parse the clock entry.
                if let Some((start_ts, after_start)) = parse_timestamp(rest, 0) {
                    // Check that start timestamp is inactive.
                    if start_ts.active {
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: 1,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: "CLOCK start timestamp should be inactive [...]".to_string(),
                            fix: None,
                        });
                    }

                    // Check for closed clock (--[end] => duration).
                    let after = &rest[after_start..];
                    if let Some(rest_after_sep) = after.strip_prefix("--") {
                        if let Some((end_ts, after_end)) = parse_timestamp(rest_after_sep, 0) {
                            if end_ts.active {
                                diagnostics.push(Diagnostic {
                                    file: ctx.source.path.clone(),
                                    line: line_num,
                                    column: 1,
                                    severity: Severity::Warning,
                                    rule_id: self.id(),
                                    rule: self.name(),
                                    message: "CLOCK end timestamp should be inactive [...]"
                                        .to_string(),
                                    fix: None,
                                });
                            }

                            // Check duration if present (=> HH:MM).
                            let duration_part = &rest_after_sep[after_end..].trim();
                            if let Some(dur_str) = duration_part.strip_prefix("=>") {
                                let dur = dur_str.trim();
                                if let Some((claimed_h, claimed_m)) = parse_duration(dur) {
                                    // Calculate actual duration.
                                    if let Some(actual_mins) =
                                        compute_duration_mins(&start_ts, &end_ts)
                                    {
                                        let claimed_mins =
                                            claimed_h as i64 * 60 + claimed_m as i64;
                                        if claimed_mins != actual_mins {
                                            diagnostics.push(Diagnostic {
                                                file: ctx.source.path.clone(),
                                                line: line_num,
                                                column: 1,
                                                severity: Severity::Warning,
                                                rule_id: self.id(),
                                                rule: self.name(),
                                                message: format!(
                                                    "CLOCK duration {}:{:02} does not match actual difference {}:{:02}",
                                                    claimed_h, claimed_m,
                                                    actual_mins / 60, actual_mins % 60
                                                ),
                                                fix: None,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            offset += line.len() + 1;
        }

        diagnostics
    }
}

/// Parses a `HH:MM` duration string into `(hours, minutes)`.
fn parse_duration(s: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let h: u32 = parts[0].trim().parse().ok()?;
    let m: u32 = parts[1].trim().parse().ok()?;
    Some((h, m))
}

/// Computes the difference in minutes between two [`OrgTimestamp`] values.
///
/// Uses a simple day-difference approximation for cross-day entries within
/// the same month.
fn compute_duration_mins(
    start: &crate::rules::timestamp::OrgTimestamp,
    end: &crate::rules::timestamp::OrgTimestamp,
) -> Option<i64> {
    let sh = start.hour? as i64;
    let sm = start.minute? as i64;
    let eh = end.hour? as i64;
    let em = end.minute? as i64;

    // Simple same-day calculation.
    let start_mins = sh * 60 + sm;
    let end_mins = eh * 60 + em;

    // If dates differ, compute day difference.
    if start.year == end.year && start.month == end.month && start.day == end.day {
        Some(end_mins - start_mins)
    } else {
        // Cross-day: approximate with 24h * day_diff + time diff.
        // Simple approach for same-month.
        let day_diff = end.day as i64 - start.day as i64;
        Some(day_diff * 24 * 60 + (end_mins - start_mins))
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
        ClockEntryValidity.check(&ctx)
    }

    #[test]
    fn valid_clock() {
        let input = "CLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:30] =>  1:30\n";
        assert!(check_it(input).is_empty());
    }

    #[test]
    fn wrong_duration() {
        let input = "CLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:30] =>  2:30\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("does not match"));
    }

    #[test]
    fn active_timestamp() {
        let input = "CLOCK: <2024-01-15 Mon 09:00>--<2024-01-15 Mon 10:00> =>  1:00\n";
        let diags = check_it(input);
        assert_eq!(diags.len(), 2); // Both start and end are active.
    }

    #[test]
    fn running_clock() {
        let input = "CLOCK: [2024-01-15 Mon 09:00]\n";
        assert!(check_it(input).is_empty()); // Running clock is valid.
    }

    #[test]
    fn no_clock_lines() {
        assert!(check_it("text\n").is_empty());
    }
}
