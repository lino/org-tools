// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Clock time report generation with `:EFFORT:` variance analysis.

use std::collections::BTreeMap;

use org_tools_core::columns::parse_duration_minutes;
use org_tools_core::document::OrgDocument;

use crate::date;

// region Public Types

/// How to group clock time in the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum GroupBy {
    /// Group by entry (one row per heading with clocked time).
    Entry,
    /// Group by tag (sum time across all entries sharing a tag).
    Tag,
    /// Group by day.
    Day,
}

/// A single row in a clock report.
#[derive(Debug)]
pub struct ReportRow {
    /// Label for this row (entry title, tag name, or date).
    pub label: String,
    /// File path (for entry grouping).
    pub file: String,
    /// Total clocked minutes.
    pub minutes: i64,
    /// Optional `:EFFORT:` estimate in minutes (for entry grouping).
    pub effort_minutes: Option<i64>,
    /// Optional variance (`minutes - effort_minutes`) when `:EFFORT:` is present.
    pub variance_minutes: Option<i64>,
}

// endregion

// region Report Builders

/// Builds a clock report from documents.
pub fn build_report(
    docs: &[OrgDocument],
    from: Option<(u16, u8, u8)>,
    to: Option<(u16, u8, u8)>,
    group_by: GroupBy,
    filter_tags: &[String],
) -> Vec<ReportRow> {
    let from_days = from.map(|(y, m, d)| date::date_to_days(y, m, d));
    let to_days = to.map(|(y, m, d)| date::date_to_days(y, m, d));

    match group_by {
        GroupBy::Entry => build_entry_report(docs, from_days, to_days, filter_tags),
        GroupBy::Tag => build_tag_report(docs, from_days, to_days, filter_tags),
        GroupBy::Day => build_day_report(docs, from_days, to_days, filter_tags),
    }
}

/// Builds per-entry clock report rows with optional `:EFFORT:` variance.
fn build_entry_report(
    docs: &[OrgDocument],
    from_days: Option<i64>,
    to_days: Option<i64>,
    filter_tags: &[String],
) -> Vec<ReportRow> {
    let mut rows = Vec::new();

    for doc in docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            if !filter_tags.is_empty() {
                let inherited = doc.inherited_tags(idx);
                if !filter_tags
                    .iter()
                    .all(|ft| inherited.iter().any(|t| t.eq_ignore_ascii_case(ft)))
                {
                    continue;
                }
            }

            let total = sum_clocked_minutes(entry, from_days, to_days);
            if total > 0 {
                let effort_minutes = entry
                    .properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("EFFORT"))
                    .or_else(|| {
                        doc.default_properties
                            .iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case("EFFORT"))
                    })
                    .and_then(|(_, v)| parse_duration_minutes(v));
                let variance_minutes = effort_minutes.map(|eff| total - eff);

                rows.push(ReportRow {
                    label: entry.title.clone(),
                    file: doc.file.display().to_string(),
                    minutes: total,
                    effort_minutes,
                    variance_minutes,
                });
            }
        }
    }

    rows.sort_by_key(|a| std::cmp::Reverse(a.minutes));
    rows
}

/// Builds per-tag clock report rows.
fn build_tag_report(
    docs: &[OrgDocument],
    from_days: Option<i64>,
    to_days: Option<i64>,
    filter_tags: &[String],
) -> Vec<ReportRow> {
    let mut tag_totals: BTreeMap<String, i64> = BTreeMap::new();

    for doc in docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            let inherited = doc.inherited_tags(idx);

            if !filter_tags.is_empty()
                && !filter_tags
                    .iter()
                    .all(|ft| inherited.iter().any(|t| t.eq_ignore_ascii_case(ft)))
            {
                continue;
            }

            let total = sum_clocked_minutes(entry, from_days, to_days);
            if total > 0 {
                if inherited.is_empty() {
                    *tag_totals.entry("(untagged)".to_string()).or_default() += total;
                } else {
                    for tag in &inherited {
                        *tag_totals.entry(tag.to_string()).or_default() += total;
                    }
                }
            }
        }
    }

    let mut rows: Vec<ReportRow> = tag_totals
        .into_iter()
        .map(|(tag, minutes)| ReportRow {
            label: tag,
            file: String::new(),
            minutes,
            effort_minutes: None,
            variance_minutes: None,
        })
        .collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.minutes));
    rows
}

/// Builds per-day clock report rows.
fn build_day_report(
    docs: &[OrgDocument],
    from_days: Option<i64>,
    to_days: Option<i64>,
    filter_tags: &[String],
) -> Vec<ReportRow> {
    let mut day_totals: BTreeMap<String, i64> = BTreeMap::new();

    for doc in docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            if !filter_tags.is_empty() {
                let inherited = doc.inherited_tags(idx);
                if !filter_tags
                    .iter()
                    .all(|ft| inherited.iter().any(|t| t.eq_ignore_ascii_case(ft)))
                {
                    continue;
                }
            }

            for clock in &entry.clocks {
                if clock.end.is_none() {
                    continue;
                }
                let clock_day =
                    date::date_to_days(clock.start.year, clock.start.month, clock.start.day);

                if let Some(from) = from_days {
                    if clock_day < from {
                        continue;
                    }
                }
                if let Some(to) = to_days {
                    if clock_day > to {
                        continue;
                    }
                }

                if let Some(mins) = clock.duration_minutes {
                    let label = format!(
                        "{:04}-{:02}-{:02}",
                        clock.start.year, clock.start.month, clock.start.day
                    );
                    *day_totals.entry(label).or_default() += mins;
                }
            }
        }
    }

    day_totals
        .into_iter()
        .map(|(day, minutes)| ReportRow {
            label: day,
            file: String::new(),
            minutes,
            effort_minutes: None,
            variance_minutes: None,
        })
        .collect()
}

/// Sums clocked minutes for an entry within an optional date range.
fn sum_clocked_minutes(
    entry: &org_tools_core::document::OrgEntry,
    from_days: Option<i64>,
    to_days: Option<i64>,
) -> i64 {
    let mut total = 0i64;
    for clock in &entry.clocks {
        if clock.end.is_none() {
            continue;
        }
        let clock_day = date::date_to_days(clock.start.year, clock.start.month, clock.start.day);
        if let Some(from) = from_days {
            if clock_day < from {
                continue;
            }
        }
        if let Some(to) = to_days {
            if clock_day > to {
                continue;
            }
        }
        if let Some(mins) = clock.duration_minutes {
            total += mins;
        }
    }
    total
}

// endregion

// region Output Rendering

/// Formats signed variance minutes as `+H:MM` or `-H:MM`.
fn format_variance(variance: i64) -> String {
    let sign = if variance < 0 { "-" } else { "+" };
    format!("{sign}{}", date::format_duration(variance.abs()))
}

/// Renders entry-grouped rows in human-readable table format.
fn render_entry_rows_human(out: &mut String, rows: &[ReportRow], max_label: usize) {
    let max_file = rows.iter().map(|r| r.file.len()).max().unwrap_or(4).max(4);
    let has_effort = rows.iter().any(|r| r.effort_minutes.is_some());
    if has_effort {
        out.push_str(&format!(
            "  {:<max_file$}  {:<max_label$}  {:<6}  {:<6}  Variance\n",
            "File", "Entry", "Time", "Effort"
        ));
        out.push_str(&format!(
            "  {:<max_file$}  {:<max_label$}  ──────  ──────  ────────\n",
            "─".repeat(max_file),
            "─".repeat(max_label)
        ));
        for row in rows {
            let eff = row
                .effort_minutes
                .map(date::format_duration)
                .unwrap_or_else(|| "-".to_string());
            let var = row
                .variance_minutes
                .map(format_variance)
                .unwrap_or_else(|| "-".to_string());
            out.push_str(&format!(
                "  {:<max_file$}  {:<max_label$}  {:<6}  {:<6}  {}\n",
                row.file,
                row.label,
                date::format_duration(row.minutes),
                eff,
                var
            ));
        }
    } else {
        out.push_str(&format!(
            "  {:<max_file$}  {:<max_label$}  Time\n",
            "File", "Entry"
        ));
        out.push_str(&format!(
            "  {:<max_file$}  {:<max_label$}  ─────\n",
            "─".repeat(max_file),
            "─".repeat(max_label)
        ));
        for row in rows {
            out.push_str(&format!(
                "  {:<max_file$}  {:<max_label$}  {}\n",
                row.file,
                row.label,
                date::format_duration(row.minutes)
            ));
        }
    }
}

/// Renders the report in human-readable format.
pub fn render_human(rows: &[ReportRow], group_by: GroupBy) -> String {
    if rows.is_empty() {
        return "No clocked time found.\n".to_string();
    }
    let total_mins: i64 = match group_by {
        GroupBy::Tag => 0,
        _ => rows.iter().map(|r| r.minutes).sum(),
    };
    let mut out = String::new();
    if group_by != GroupBy::Tag {
        out.push_str(&format!("Total: {}\n\n", date::format_duration(total_mins)));
    }
    let max_label = rows.iter().map(|r| r.label.len()).max().unwrap_or(5).max(5);
    match group_by {
        GroupBy::Entry => render_entry_rows_human(&mut out, rows, max_label),
        GroupBy::Tag | GroupBy::Day => {
            let hdr = if group_by == GroupBy::Tag {
                "Tag"
            } else {
                "Day"
            };
            out.push_str(&format!("  {:<max_label$}  Time\n", hdr));
            out.push_str(&format!("  {:<max_label$}  ─────\n", "─".repeat(max_label)));
            for row in rows {
                out.push_str(&format!(
                    "  {:<max_label$}  {}\n",
                    row.label,
                    date::format_duration(row.minutes)
                ));
            }
        }
    }
    out
}

/// Renders the report as pretty-printed JSON.
pub fn render_json(rows: &[ReportRow], group_by: GroupBy) -> String {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let mut obj = serde_json::json!({
                "label": r.label,
                "minutes": r.minutes,
                "duration": date::format_duration(r.minutes),
            });
            if group_by == GroupBy::Entry {
                obj["file"] = serde_json::Value::String(r.file.clone());
                if let Some(eff) = r.effort_minutes {
                    obj["effort_minutes"] = serde_json::json!(eff);
                    obj["effort"] = serde_json::Value::String(date::format_duration(eff));
                    obj["variance_minutes"] = serde_json::json!(r.variance_minutes);
                }
            }
            obj
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}

// endregion

#[cfg(test)]
mod tests {
    use super::*;
    use org_tools_core::source::SourceFile;

    /// Parses an in-memory Org document fixture.
    fn make_doc(content: &str) -> OrgDocument {
        let source = SourceFile::new("test.org", content.to_string());
        OrgDocument::from_source(&source)
    }

    /// Verifies basic entry report sorting by total minutes descending.
    #[test]
    fn entry_report_basic() {
        let doc = make_doc(
            "* Task A\nCLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:30] =>  1:30\n* Task B\nCLOCK: [2024-01-15 Mon 14:00]--[2024-01-15 Mon 14:45] =>  0:45\n",
        );
        let rows = build_report(&[doc], None, None, GroupBy::Entry, &[]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].minutes, 90);
        assert_eq!(rows[1].minutes, 45);
    }

    /// Verifies `--from` date filtering on clock start dates.
    #[test]
    fn date_filter() {
        let doc = make_doc(
            "* Task\nCLOCK: [2024-01-10 Wed 09:00]--[2024-01-10 Wed 10:00] =>  1:00\nCLOCK: [2024-01-20 Sat 09:00]--[2024-01-20 Sat 10:00] =>  1:00\n",
        );
        let rows = build_report(&[doc], Some((2024, 1, 15)), None, GroupBy::Entry, &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].minutes, 60);
    }

    /// Verifies tag-grouped clock aggregation.
    #[test]
    fn tag_report() {
        let doc = make_doc(
            "* Task :work:\nCLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:30] =>  1:30\n",
        );
        let rows = build_report(&[doc], None, None, GroupBy::Tag, &[]);
        assert!(rows.iter().any(|r| r.label == "work" && r.minutes == 90));
    }

    /// Verifies day-grouped clock aggregation.
    #[test]
    fn day_report() {
        let doc = make_doc(
            "* Task\nCLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:30] =>  1:30\nCLOCK: [2024-01-16 Tue 09:00]--[2024-01-16 Tue 09:45] =>  0:45\n",
        );
        let rows = build_report(&[doc], None, None, GroupBy::Day, &[]);
        assert_eq!(rows.len(), 2);
    }

    /// Verifies entries without clocks produce an empty report.
    #[test]
    fn no_clocks() {
        let doc = make_doc("* Task\nSome text.\n");
        let rows = build_report(&[doc], None, None, GroupBy::Entry, &[]);
        assert!(rows.is_empty());
    }

    /// Verifies open running clocks are excluded from completed clock reports.
    #[test]
    fn running_clocks_excluded() {
        let doc = make_doc("* Task\nCLOCK: [2024-01-15 Mon 09:00]\n");
        let rows = build_report(&[doc], None, None, GroupBy::Entry, &[]);
        assert!(rows.is_empty());
    }
}
