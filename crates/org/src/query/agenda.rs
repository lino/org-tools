// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Agenda view — shows scheduled/deadline items grouped by day.

use org_tools_core::document::{OrgDocument, OrgEntry};
use org_tools_core::rules::timestamp::OrgTimestamp;

/// An agenda item with its source context.
#[derive(Debug)]
pub struct AgendaItem<'a> {
    /// Reference to the entry.
    pub entry: &'a OrgEntry,
    /// File path.
    pub file: &'a std::path::Path,
    /// Why this item appears on this day.
    pub kind: AgendaKind,
    /// The relevant timestamp.
    pub timestamp: &'a OrgTimestamp,
}

/// Why an entry appears in the agenda.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AgendaKind {
    /// Scheduled for this day.
    Scheduled,
    /// Deadline on this day.
    Deadline,
}

/// A day in the agenda with its items.
#[derive(Debug)]
pub struct AgendaDay<'a> {
    /// The date.
    pub year: u16,
    pub month: u8,
    pub day: u8,
    /// Items on this day.
    pub items: Vec<AgendaItem<'a>>,
}

/// Build an agenda for a range of days starting from `start_date`.
pub fn build_agenda<'a>(
    docs: &'a [OrgDocument],
    start_date: (u16, u8, u8),
    num_days: usize,
) -> Vec<AgendaDay<'a>> {
    let start_days = date_to_days(start_date.0, start_date.1, start_date.2);

    let mut days: Vec<AgendaDay<'a>> = (0..num_days as i64)
        .map(|offset| {
            let (y, m, d) = days_to_date(start_days + offset);
            AgendaDay {
                year: y,
                month: m,
                day: d,
                items: Vec::new(),
            }
        })
        .collect();

    for doc in docs {
        for entry in &doc.entries {
            if let Some(ts) = &entry.planning.scheduled {
                let ts_days = date_to_days(ts.year, ts.month, ts.day);
                let offset = ts_days - start_days;
                if offset >= 0 && (offset as usize) < num_days {
                    days[offset as usize].items.push(AgendaItem {
                        entry,
                        file: &doc.file,
                        kind: AgendaKind::Scheduled,
                        timestamp: ts,
                    });
                }
            }
            if let Some(ts) = &entry.planning.deadline {
                let ts_days = date_to_days(ts.year, ts.month, ts.day);
                let offset = ts_days - start_days;
                if offset >= 0 && (offset as usize) < num_days {
                    days[offset as usize].items.push(AgendaItem {
                        entry,
                        file: &doc.file,
                        kind: AgendaKind::Deadline,
                        timestamp: ts,
                    });
                }
            }
        }
    }

    days
}

/// Render agenda in human-readable format.
pub fn render_agenda_human(days: &[AgendaDay<'_>]) -> String {
    let mut out = String::new();
    let day_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

    for day in days {
        if day.items.is_empty() {
            continue;
        }

        let dow = day_of_week(day.year, day.month, day.day);
        let dow_name = day_names[dow as usize % 7];
        out.push_str(&format!(
            "{:04}-{:02}-{:02} {dow_name}\n",
            day.year, day.month, day.day
        ));

        for item in &day.items {
            let kw = item
                .entry
                .keyword
                .as_deref()
                .map(|k| format!("{k} "))
                .unwrap_or_default();
            let pri = item
                .entry
                .priority
                .map(|p| format!("[#{p}] "))
                .unwrap_or_default();
            let tags = if item.entry.tags.is_empty() {
                String::new()
            } else {
                format!(" :{}:", item.entry.tags.join(":"))
            };
            let time = match (item.timestamp.hour, item.timestamp.minute) {
                (Some(h), Some(m)) => format!("{h:02}:{m:02} "),
                _ => String::new(),
            };
            let kind_str = match item.kind {
                AgendaKind::Scheduled => "Scheduled",
                AgendaKind::Deadline => "DEADLINE",
            };

            out.push_str(&format!(
                "  {}:{}: {kw}{pri}{}{tags}  {time}{kind_str}\n",
                item.file.display(),
                item.entry.heading_line,
                item.entry.title,
            ));
        }
    }

    out
}

/// Approximate day-of-week (0=Mon, 6=Sun) using Zeller-like formula.
fn day_of_week(year: u16, month: u8, day: u8) -> u8 {
    let y = year as i64;
    let m = month as i64;
    let d = day as i64;
    // Tomohiko Sakamoto's algorithm.
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let dow = (y + y / 4 - y / 100 + y / 400 + t[(m - 1) as usize] + d) % 7;
    // Result: 0=Sun, 1=Mon, ..., 6=Sat. Convert to 0=Mon.
    ((dow + 6) % 7) as u8
}

/// Public wrapper for date_to_days, used by predicate module.
pub fn date_to_days_pub(year: u16, month: u8, day: u8) -> i64 {
    date_to_days(year, month, day)
}

/// Convert (year, month, day) to a day count using Howard Hinnant's algorithm.
/// See <https://howardhinnant.github.io/date_algorithms.html#days_from_civil>
fn date_to_days(year: u16, month: u8, day: u8) -> i64 {
    let y = if month <= 2 {
        year as i64 - 1
    } else {
        year as i64
    };
    let m = if month <= 2 {
        month as i64 + 9
    } else {
        month as i64 - 3
    };
    let era = y / 400;
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe
}

/// Convert a day count back to (year, month, day) using Howard Hinnant's algorithm.
/// See <https://howardhinnant.github.io/date_algorithms.html#civil_from_days>
fn days_to_date(z: i64) -> (u16, u8, u8) {
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u16, m as u8, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_roundtrip() {
        let test_dates = [
            (2024, 1, 1),
            (2024, 6, 15),
            (2024, 12, 31),
            (2025, 2, 28),
            (2026, 3, 21),
        ];
        for (y, m, d) in test_dates {
            let days = date_to_days(y, m, d);
            let (ry, rm, rd) = days_to_date(days);
            assert_eq!((ry, rm, rd), (y, m, d), "roundtrip failed for {y}-{m}-{d}");
        }
    }

    #[test]
    fn day_of_week_known() {
        // 2024-01-01 is a Monday.
        assert_eq!(day_of_week(2024, 1, 1), 0); // Mon
                                                // 2024-06-15 is a Saturday.
        assert_eq!(day_of_week(2024, 6, 15), 5); // Sat
    }

    #[test]
    fn build_agenda_basic() {
        use org_tools_core::document::OrgDocument;
        use org_tools_core::source::SourceFile;

        let source = SourceFile::new(
            "test.org",
            "* TODO Task\nSCHEDULED: <2024-06-16 Sun 09:00>\n* TODO DL\nDEADLINE: <2024-06-17 Mon>\n".to_string(),
        );
        let doc = OrgDocument::from_source(&source);
        let docs = [doc];
        let days = build_agenda(&docs, (2024, 6, 15), 7);

        // Day 0 (June 15) — nothing.
        assert!(days[0].items.is_empty());
        // Day 1 (June 16) — scheduled task.
        assert_eq!(days[1].items.len(), 1);
        assert_eq!(days[1].items[0].kind, AgendaKind::Scheduled);
        // Day 2 (June 17) — deadline.
        assert_eq!(days[2].items.len(), 1);
        assert_eq!(days[2].items[0].kind, AgendaKind::Deadline);
    }
}
