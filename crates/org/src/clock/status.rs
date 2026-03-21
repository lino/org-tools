// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Find running clocks (clock entries without an end timestamp).

use org_tools_core::document::{OrgDocument, OrgEntry};

/// A running clock with its source context.
pub struct RunningClock<'a> {
    /// The entry with the running clock.
    pub entry: &'a OrgEntry,
    /// File path.
    pub file: &'a std::path::Path,
    /// The clock start time as formatted string.
    pub since: String,
}

/// Find all running clocks across documents.
pub fn find_running_clocks<'a>(docs: &'a [OrgDocument]) -> Vec<RunningClock<'a>> {
    let mut running = Vec::new();
    for doc in docs {
        for entry in &doc.entries {
            for clock in &entry.clocks {
                if clock.end.is_none() {
                    let since = match (clock.start.hour, clock.start.minute) {
                        (Some(h), Some(m)) => format!(
                            "{:04}-{:02}-{:02} {:02}:{:02}",
                            clock.start.year, clock.start.month, clock.start.day, h, m
                        ),
                        _ => format!(
                            "{:04}-{:02}-{:02}",
                            clock.start.year, clock.start.month, clock.start.day
                        ),
                    };
                    running.push(RunningClock {
                        entry,
                        file: &doc.file,
                        since,
                    });
                }
            }
        }
    }
    running
}

/// Render running clocks in human-readable format.
pub fn render_human(clocks: &[RunningClock<'_>]) -> String {
    if clocks.is_empty() {
        return "No running clocks.\n".to_string();
    }
    let mut out = String::new();
    out.push_str("Running clocks:\n");
    for rc in clocks {
        let kw = rc
            .entry
            .keyword
            .as_deref()
            .map(|k| format!("{k} "))
            .unwrap_or_default();
        let tags = if rc.entry.tags.is_empty() {
            String::new()
        } else {
            format!(" :{}:", rc.entry.tags.join(":"))
        };
        out.push_str(&format!(
            "  {}:{}: {kw}{}{tags}  (since {})\n",
            rc.file.display(),
            rc.entry.heading_line,
            rc.entry.title,
            rc.since,
        ));
    }
    out
}

/// Render running clocks as JSON.
pub fn render_json(clocks: &[RunningClock<'_>]) -> String {
    let items: Vec<serde_json::Value> = clocks
        .iter()
        .map(|rc| {
            serde_json::json!({
                "file": rc.file.display().to_string(),
                "line": rc.entry.heading_line,
                "keyword": rc.entry.keyword,
                "title": rc.entry.title,
                "tags": rc.entry.tags,
                "since": rc.since,
            })
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_tools_core::source::SourceFile;

    #[test]
    fn finds_running_clock() {
        let source = SourceFile::new(
            "test.org",
            "* TODO Task\n:LOGBOOK:\nCLOCK: [2024-01-15 Mon 09:00]\n:END:\n".to_string(),
        );
        let doc = OrgDocument::from_source(&source);
        let docs = [doc];
        let running = find_running_clocks(&docs);
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].entry.title, "Task");
        assert!(running[0].since.contains("09:00"));
    }

    #[test]
    fn ignores_completed_clocks() {
        let source = SourceFile::new(
            "test.org",
            "* Task\nCLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:00] =>  1:00\n".to_string(),
        );
        let doc = OrgDocument::from_source(&source);
        let docs = [doc];
        assert!(find_running_clocks(&docs).is_empty());
    }

    #[test]
    fn no_clocks_at_all() {
        let source = SourceFile::new("test.org", "* Task\nSome text.\n".to_string());
        let doc = OrgDocument::from_source(&source);
        let docs = [doc];
        assert!(find_running_clocks(&docs).is_empty());
    }

    #[test]
    fn human_output_empty() {
        let out = render_human(&[]);
        assert!(out.contains("No running clocks"));
    }
}
