// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Output rendering for query results.

use orgfmt_core::document::{OrgDocument, OrgEntry};
use orgfmt_core::locator::locator_for_entry;
use serde::Serialize;

/// A matched entry with its document context.
pub struct MatchedEntry<'a> {
    /// The document containing this entry.
    pub doc: &'a OrgDocument,
    /// Index into `doc.entries`.
    pub entry_idx: usize,
}

impl<'a> MatchedEntry<'a> {
    /// Get the entry reference.
    pub fn entry(&self) -> &OrgEntry {
        &self.doc.entries[self.entry_idx]
    }
}

/// Render matches in human-readable format.
pub fn render_human(matches: &[MatchedEntry<'_>]) -> String {
    let mut out = String::new();
    for m in matches {
        let entry = m.entry();
        let tags = if entry.tags.is_empty() {
            String::new()
        } else {
            format!(" :{}:", entry.tags.join(":"))
        };
        let kw = entry
            .keyword
            .as_deref()
            .map(|k| format!("{k} "))
            .unwrap_or_default();
        let pri = entry
            .priority
            .map(|p| format!("[#{p}] "))
            .unwrap_or_default();
        let stars = "*".repeat(entry.level);

        out.push_str(&format!(
            "{}:{}: {stars} {kw}{pri}{}{tags}\n",
            m.doc.file.display(),
            entry.heading_line,
            entry.title,
        ));
    }
    out
}

/// JSON output entry.
#[derive(Serialize)]
pub struct JsonEntry {
    pub file: String,
    pub line: usize,
    pub locator: String,
    pub level: usize,
    pub keyword: Option<String>,
    pub priority: Option<String>,
    pub title: String,
    pub tags: Vec<String>,
    pub properties: std::collections::HashMap<String, String>,
    pub scheduled: Option<String>,
    pub deadline: Option<String>,
    pub closed: Option<String>,
    pub clocked_minutes: i64,
}

/// Render matches as JSON.
pub fn render_json(matches: &[MatchedEntry<'_>]) -> String {
    let items: Vec<JsonEntry> = matches.iter().map(|m| {
        let entry = m.entry();
        let loc = locator_for_entry(m.doc, m.entry_idx);
        let total_clocked: i64 = entry
            .clocks
            .iter()
            .filter_map(|c| c.duration_minutes)
            .sum();

        JsonEntry {
            file: m.doc.file.display().to_string(),
            line: entry.heading_line,
            locator: loc.to_string(),
            level: entry.level,
            keyword: entry.keyword.clone(),
            priority: entry.priority.map(|p| p.to_string()),
            title: entry.title.clone(),
            tags: entry.tags.clone(),
            properties: entry.properties.clone(),
            scheduled: entry.planning.scheduled.as_ref().map(format_ts),
            deadline: entry.planning.deadline.as_ref().map(format_ts),
            closed: entry.planning.closed.as_ref().map(format_ts),
            clocked_minutes: total_clocked,
        }
    }).collect();

    serde_json::to_string_pretty(&items).unwrap_or_default()
}

/// Render matches as locator strings (one per line).
pub fn render_locators(matches: &[MatchedEntry<'_>]) -> String {
    let mut out = String::new();
    for m in matches {
        let loc = locator_for_entry(m.doc, m.entry_idx);
        out.push_str(&loc.to_string());
        out.push('\n');
    }
    out
}

/// Format a timestamp for display.
fn format_ts(ts: &orgfmt_core::rules::timestamp::OrgTimestamp) -> String {
    let time = match (ts.hour, ts.minute) {
        (Some(h), Some(m)) => format!(" {h:02}:{m:02}"),
        _ => String::new(),
    };
    format!("{:04}-{:02}-{:02}{time}", ts.year, ts.month, ts.day)
}
