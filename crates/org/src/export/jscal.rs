// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSCalendar (RFC 8984) export for org-mode entries.
//!
//! Exports entries with SCHEDULED timestamps as Event objects
//! and entries with DEADLINE timestamps as Task objects.

use org_tools_core::document::OrgDocument;
use org_tools_core::rules::timestamp::OrgTimestamp;

use crate::date;

/// Generate JSCalendar JSON output from documents.
pub fn export_jscal(
    docs: &[OrgDocument],
    from: Option<(u16, u8, u8)>,
    to: Option<(u16, u8, u8)>,
    filter_tags: &[String],
) -> String {
    let from_days = from.map(|(y, m, d)| date::date_to_days(y, m, d));
    let to_days = to.map(|(y, m, d)| date::date_to_days(y, m, d));

    let mut items: Vec<serde_json::Value> = Vec::new();

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

            // SCHEDULED → Event
            if let Some(ts) = &entry.planning.scheduled {
                if in_date_range(ts, from_days, to_days) {
                    let mut obj = serde_json::json!({
                        "@type": "Event",
                        "title": entry.title,
                        "start": format_jscal_datetime(ts),
                    });
                    add_common_fields(&mut obj, entry, doc, idx);
                    items.push(obj);
                }
            }

            // DEADLINE → Task
            if let Some(ts) = &entry.planning.deadline {
                if in_date_range(ts, from_days, to_days) {
                    let mut obj = serde_json::json!({
                        "@type": "Task",
                        "title": entry.title,
                        "due": format_jscal_datetime(ts),
                    });
                    add_common_fields(&mut obj, entry, doc, idx);
                    items.push(obj);
                }
            }
        }
    }

    serde_json::to_string_pretty(&items).unwrap_or_default()
}

fn add_common_fields(
    obj: &mut serde_json::Value,
    entry: &org_tools_core::document::OrgEntry,
    doc: &OrgDocument,
    idx: usize,
) {
    // UID from :ID: property.
    if let Some(id) = entry.properties.get("ID") {
        obj["uid"] = serde_json::Value::String(id.clone());
    }

    // Priority: map A=1, B=5, C=9.
    if let Some(pri) = entry.priority {
        let jscal_pri = match pri {
            'A' => 1,
            'B' => 5,
            'C' => 9,
            _ => ((pri as u8).saturating_sub(b'A') as i32).clamp(1, 9),
        };
        obj["priority"] = serde_json::Value::Number(serde_json::Number::from(jscal_pri));
    }

    // Keywords (tags) as a map.
    let tags = doc.inherited_tags(idx);
    if !tags.is_empty() {
        let mut keywords = serde_json::Map::new();
        for tag in &tags {
            keywords.insert(tag.to_string(), serde_json::Value::Bool(true));
        }
        obj["keywords"] = serde_json::Value::Object(keywords);
    }

    // Status from TODO keyword.
    if let Some(kw) = &entry.keyword {
        obj["status"] = serde_json::Value::String(kw.clone());
    }

    // Source reference.
    obj["x-org-file"] = serde_json::Value::String(doc.file.display().to_string());
    obj["x-org-line"] = serde_json::Value::Number(serde_json::Number::from(entry.heading_line));
}

/// Format a timestamp as ISO 8601 datetime.
fn format_jscal_datetime(ts: &OrgTimestamp) -> String {
    match (ts.hour, ts.minute) {
        (Some(h), Some(m)) => {
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:00",
                ts.year, ts.month, ts.day, h, m
            )
        }
        _ => format!("{:04}-{:02}-{:02}", ts.year, ts.month, ts.day),
    }
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
    fn scheduled_as_event() {
        let doc = make_doc("* TODO Meeting\nSCHEDULED: <2024-01-15 Mon 09:00>\n");
        let json = export_jscal(&[doc], None, None, &[]);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["@type"], "Event");
        assert_eq!(parsed[0]["title"], "Meeting");
        assert_eq!(parsed[0]["start"], "2024-01-15T09:00:00");
        assert_eq!(parsed[0]["status"], "TODO");
    }

    #[test]
    fn deadline_as_task() {
        let doc = make_doc("* TODO Report\nDEADLINE: <2024-02-01 Thu>\n");
        let json = export_jscal(&[doc], None, None, &[]);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["@type"], "Task");
        assert_eq!(parsed[0]["due"], "2024-02-01");
    }

    #[test]
    fn priority_mapped() {
        let doc = make_doc("* TODO [#A] Urgent\nSCHEDULED: <2024-01-15 Mon>\n");
        let json = export_jscal(&[doc], None, None, &[]);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["priority"], 1);
    }

    #[test]
    fn tags_as_keywords() {
        let doc = make_doc("* Task :work:urgent:\nSCHEDULED: <2024-01-15 Mon>\n");
        let json = export_jscal(&[doc], None, None, &[]);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["keywords"]["work"], true);
        assert_eq!(parsed[0]["keywords"]["urgent"], true);
    }

    #[test]
    fn no_planning_no_items() {
        let doc = make_doc("* Just a heading\n");
        let json = export_jscal(&[doc], None, None, &[]);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn id_as_uid() {
        let doc =
            make_doc("* Task\nSCHEDULED: <2024-01-15 Mon>\n:PROPERTIES:\n:ID: abc-123\n:END:\n");
        let json = export_jscal(&[doc], None, None, &[]);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["uid"], "abc-123");
    }
}
