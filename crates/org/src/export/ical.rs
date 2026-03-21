// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! iCalendar (RFC 5545) export for org-mode entries.
//!
//! Exports entries with SCHEDULED timestamps as VEVENT components
//! and entries with DEADLINE timestamps as VTODO components.

use org_tools_core::document::OrgDocument;
use org_tools_core::rules::timestamp::OrgTimestamp;

use crate::date;

/// Generate iCalendar output from documents.
pub fn export_ical(
    docs: &[OrgDocument],
    from: Option<(u16, u8, u8)>,
    to: Option<(u16, u8, u8)>,
    filter_tags: &[String],
) -> String {
    let from_days = from.map(|(y, m, d)| date::date_to_days(y, m, d));
    let to_days = to.map(|(y, m, d)| date::date_to_days(y, m, d));

    let mut out = String::new();
    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:-//org-tools//EN\r\n");

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

            // SCHEDULED → VEVENT
            if let Some(ts) = &entry.planning.scheduled {
                if in_date_range(ts, from_days, to_days) {
                    out.push_str("BEGIN:VEVENT\r\n");
                    write_common_fields(&mut out, entry, doc, idx, ts);
                    out.push_str(&format!("DTSTART:{}\r\n", format_ical_datetime(ts)));
                    out.push_str("END:VEVENT\r\n");
                }
            }

            // DEADLINE → VTODO
            if let Some(ts) = &entry.planning.deadline {
                if in_date_range(ts, from_days, to_days) {
                    out.push_str("BEGIN:VTODO\r\n");
                    write_common_fields(&mut out, entry, doc, idx, ts);
                    out.push_str(&format!("DUE:{}\r\n", format_ical_datetime(ts)));
                    out.push_str("END:VTODO\r\n");
                }
            }
        }
    }

    out.push_str("END:VCALENDAR\r\n");
    out
}

fn write_common_fields(
    out: &mut String,
    entry: &org_tools_core::document::OrgEntry,
    doc: &OrgDocument,
    idx: usize,
    _ts: &OrgTimestamp,
) {
    // UID from :ID: property or generate from file+line.
    let uid = entry
        .properties
        .get("ID")
        .cloned()
        .unwrap_or_else(|| format!("{}:{}", doc.file.display(), entry.heading_line));
    out.push_str(&format!("UID:{uid}\r\n"));

    // SUMMARY: keyword + priority + title
    let mut summary = String::new();
    if let Some(kw) = &entry.keyword {
        summary.push_str(kw);
        summary.push(' ');
    }
    if let Some(pri) = entry.priority {
        summary.push_str(&format!("[#{pri}] "));
    }
    summary.push_str(&entry.title);
    out.push_str(&format!("SUMMARY:{}\r\n", escape_ical(&summary)));

    // CATEGORIES from tags.
    let tags = doc.inherited_tags(idx);
    if !tags.is_empty() {
        out.push_str(&format!("CATEGORIES:{}\r\n", tags.join(",")));
    }

    // DESCRIPTION: file reference.
    out.push_str(&format!(
        "DESCRIPTION:{}:{}\r\n",
        escape_ical(&doc.file.display().to_string()),
        entry.heading_line
    ));

    // PRIORITY: map org priority to iCal (1=highest, 9=lowest).
    if let Some(pri) = entry.priority {
        let ical_pri = match pri {
            'A' => 1,
            'B' => 5,
            'C' => 9,
            _ => ((pri as u8).saturating_sub(b'A') as i32).clamp(1, 9),
        };
        out.push_str(&format!("PRIORITY:{ical_pri}\r\n"));
    }
}

/// Format a timestamp as iCalendar datetime (YYYYMMDD or YYYYMMDDTHHMMSS).
fn format_ical_datetime(ts: &OrgTimestamp) -> String {
    match (ts.hour, ts.minute) {
        (Some(h), Some(m)) => {
            format!(
                "{:04}{:02}{:02}T{:02}{:02}00",
                ts.year, ts.month, ts.day, h, m
            )
        }
        _ => format!("{:04}{:02}{:02}", ts.year, ts.month, ts.day),
    }
}

/// Escape special characters for iCalendar text values.
fn escape_ical(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
}

/// Check if a timestamp falls within a date range.
fn in_date_range(ts: &OrgTimestamp, from_days: Option<i64>, to_days: Option<i64>) -> bool {
    let ts_days = date::date_to_days(ts.year, ts.month, ts.day);
    if let Some(from) = from_days {
        if ts_days < from {
            return false;
        }
    }
    if let Some(to) = to_days {
        if ts_days > to {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_tools_core::source::SourceFile;

    fn make_doc(content: &str) -> OrgDocument {
        let source = SourceFile::new("test.org", content.to_string());
        OrgDocument::from_source(&source)
    }

    #[test]
    fn basic_vevent() {
        let doc = make_doc("* TODO Meeting\nSCHEDULED: <2024-01-15 Mon 09:00>\n");
        let ical = export_ical(&[doc], None, None, &[]);
        assert!(ical.contains("BEGIN:VCALENDAR"));
        assert!(ical.contains("BEGIN:VEVENT"));
        assert!(ical.contains("DTSTART:20240115T090000"));
        assert!(ical.contains("SUMMARY:TODO Meeting"));
        assert!(ical.contains("END:VEVENT"));
        assert!(ical.contains("END:VCALENDAR"));
    }

    #[test]
    fn deadline_as_vtodo() {
        let doc = make_doc("* TODO Report\nDEADLINE: <2024-02-01 Thu>\n");
        let ical = export_ical(&[doc], None, None, &[]);
        assert!(ical.contains("BEGIN:VTODO"));
        assert!(ical.contains("DUE:20240201"));
        assert!(ical.contains("END:VTODO"));
    }

    #[test]
    fn priority_mapped() {
        let doc = make_doc("* TODO [#A] Urgent\nSCHEDULED: <2024-01-15 Mon>\n");
        let ical = export_ical(&[doc], None, None, &[]);
        assert!(ical.contains("PRIORITY:1"));
    }

    #[test]
    fn tags_as_categories() {
        let doc = make_doc("* TODO Task :work:urgent:\nSCHEDULED: <2024-01-15 Mon>\n");
        let ical = export_ical(&[doc], None, None, &[]);
        assert!(ical.contains("CATEGORIES:work,urgent"));
    }

    #[test]
    fn date_filter() {
        let doc =
            make_doc("* Early\nSCHEDULED: <2024-01-10 Wed>\n* Late\nSCHEDULED: <2024-01-20 Sat>\n");
        let ical = export_ical(&[doc], Some((2024, 1, 15)), None, &[]);
        assert!(!ical.contains("Early"));
        assert!(ical.contains("Late"));
    }

    #[test]
    fn no_planning_no_events() {
        let doc = make_doc("* Just a heading\nSome text.\n");
        let ical = export_ical(&[doc], None, None, &[]);
        assert!(!ical.contains("VEVENT"));
        assert!(!ical.contains("VTODO"));
    }

    #[test]
    fn id_property_as_uid() {
        let doc =
            make_doc("* Task\nSCHEDULED: <2024-01-15 Mon>\n:PROPERTIES:\n:ID: abc-123\n:END:\n");
        let ical = export_ical(&[doc], None, None, &[]);
        assert!(ical.contains("UID:abc-123"));
    }

    #[test]
    fn crlf_line_endings() {
        let doc = make_doc("* Task\nSCHEDULED: <2024-01-15 Mon>\n");
        let ical = export_ical(&[doc], None, None, &[]);
        // All lines should end with CRLF per RFC 5545.
        for line in ical.split("\r\n") {
            assert!(!line.contains('\r'), "bare CR found");
        }
    }
}
