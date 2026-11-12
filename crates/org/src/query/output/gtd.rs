// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! GTD, blocked dependencies, and stuck project renderers for `org query`.

use org_tools_core::document::{OrgDocument, OrgEntry};
use org_tools_core::edna::{self, EdnaContext};
use org_tools_core::locator::locator_for_entry;

use super::super::{parser::Predicate, predicate};
use super::{build_json_entry, format_entry_line, MatchedEntry};

// region Blocked & Waiting Renderers

/// Renders blocked entries with dependency details in human-readable format.
pub fn render_blocked_human(matches: &[MatchedEntry<'_>], all_docs: &[&OrgDocument]) -> String {
    let mut out = String::new();
    for m in matches {
        out.push_str(&format!("{}\n", format_entry_line(m)));
        let details = edna::blocking_details(&EdnaContext {
            all_docs,
            doc: m.doc,
            entry_idx: m.entry_idx,
        });
        if !details.is_empty() {
            out.push_str("  Blocked by:\n");
            for d in &details {
                let kw = d
                    .keyword
                    .as_deref()
                    .map(|k| format!("{k} — "))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "    {}:{}: {} ({kw}{})\n",
                    d.file, d.line, d.title, d.condition_desc
                ));
            }
        }
        out.push('\n');
    }
    out
}

/// Renders blocked entries as pretty JSON conforming to `org-blocked-output.schema.json`.
pub fn render_blocked_json(matches: &[MatchedEntry<'_>], all_docs: &[&OrgDocument]) -> String {
    let items: Vec<serde_json::Map<String, serde_json::Value>> = matches
        .iter()
        .map(|m| {
            let mut obj = match serde_json::to_value(build_json_entry(m, all_docs)) {
                Ok(serde_json::Value::Object(map)) => map,
                _ => serde_json::Map::new(),
            };
            obj.remove("checklist_summary");
            obj.remove("checklist");
            let blockers: Vec<serde_json::Value> = edna::blocking_details(&EdnaContext {
                all_docs,
                doc: m.doc,
                entry_idx: m.entry_idx,
            })
            .into_iter()
            .map(|d| {
                serde_json::json!({
                    "title": d.title,
                    "keyword": d.keyword,
                    "file": d.file,
                    "line": d.line,
                    "locator": d.locator,
                    "condition": d.condition_desc,
                })
            })
            .collect();
            obj.insert("blocked".into(), true.into());
            obj.insert("blocking_entries".into(), blockers.into());
            obj
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}

/// Appends a single named GTD section header and 2-space indented entries to `out`.
pub(crate) fn append_named_group(out: &mut String, label: &str, entries: &[String]) {
    out.push_str(&format!("{label} ({})\n", entries.len()));
    for entry in entries {
        out.push_str(&format!("  {entry}\n"));
    }
    out.push('\n');
}

/// Renders waiting entries with optional `:WAITING_FOR:` details.
pub fn render_waiting_human(matches: &[MatchedEntry<'_>]) -> String {
    let mut out = String::new();
    for m in matches {
        out.push_str(&format!("{}\n", format_entry_line(m)));
        if let Some(waiting_for) = m.entry().properties.get("WAITING_FOR") {
            out.push_str(&format!("  Waiting for: {waiting_for}\n"));
        }
    }
    out
}

// endregion

// region Stuck Project Analysis & Renderers

/// An inactive project entry identified by zero actionable direct TODO children.
pub struct StuckProject<'a> {
    /// Document containing the project heading.
    pub doc: &'a OrgDocument,
    /// Index of the project heading in `doc.entries`.
    pub entry_idx: usize,
    /// Number of completed child tasks.
    pub done: usize,
    /// Number of actionable child tasks (`0` for a stuck project).
    pub actionable: usize,
    /// Number of blocked child tasks.
    pub blocked_count: usize,
    /// Number of waiting child tasks.
    pub waiting: usize,
}

/// Evaluates whether `entry` is a project with zero actionable direct TODO children.
fn evaluate_stuck_candidate<'a>(
    doc: &'a OrgDocument,
    idx: usize,
    entry: &OrgEntry,
    doc_refs: &[&'a OrgDocument],
    today: (u16, u8, u8),
) -> Option<StuckProject<'a>> {
    if !entry
        .children
        .iter()
        .any(|&i| doc.entries[i].keyword.is_some())
    {
        return None;
    }

    let (mut done, mut actionable, mut blocked_count, mut waiting) = (0, 0, 0, 0);
    for &ci in &entry.children {
        let counts = (&mut done, &mut actionable, &mut blocked_count, &mut waiting);
        classify_project_child(doc, ci, doc_refs, today, counts);
    }

    (actionable == 0).then_some(StuckProject {
        doc,
        entry_idx: idx,
        done,
        actionable,
        blocked_count,
        waiting,
    })
}

/// Classifies a single project child into done, waiting, blocked, or actionable buckets.
fn classify_project_child(
    doc: &OrgDocument,
    ci: usize,
    refs: &[&OrgDocument],
    today: (u16, u8, u8),
    counts: (&mut usize, &mut usize, &mut usize, &mut usize),
) {
    let child = &doc.entries[ci];
    let Some(kw) = child.keyword.as_deref() else {
        return;
    };
    let act = Predicate::Actionable;
    let is_actionable = predicate::matches_with_body(&act, child, ci, None, doc, refs, today);
    if doc.todo_keywords.is_done(kw) {
        *counts.0 += 1;
    } else if kw.to_uppercase().contains("WAIT") || child.properties.contains_key("WAITING_FOR") {
        *counts.3 += 1;
    } else if edna::is_blocked(&EdnaContext {
        all_docs: refs,
        doc,
        entry_idx: ci,
    }) {
        *counts.2 += 1;
    } else if is_actionable {
        *counts.1 += 1;
    }
}

/// Finds projects (headings with TODO children) that have zero actionable children.
pub fn find_stuck_projects<'a>(
    docs: &'a [OrgDocument],
    doc_refs: &[&'a OrgDocument],
    today: (u16, u8, u8),
) -> Vec<StuckProject<'a>> {
    let mut stuck = Vec::new();
    for doc in docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            if let Some(project) = evaluate_stuck_candidate(doc, idx, entry, doc_refs, today) {
                stuck.push(project);
            }
        }
    }
    stuck
}

/// Renders stuck projects in human-readable summary format.
pub fn render_stuck_human(stuck: &[StuckProject<'_>]) -> String {
    let mut out = String::new();
    for sp in stuck {
        let entry = &sp.doc.entries[sp.entry_idx];
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
        out.push_str(&format!(
            "{}:{}: {} {kw}{}{tags}\n  Children: {}\n\n",
            sp.doc.file.display(),
            entry.heading_line,
            "*".repeat(entry.level),
            entry.title,
            format_stuck_counts(sp),
        ));
    }
    out
}

/// Formats comma-separated child status counts for a [`StuckProject`].
fn format_stuck_counts(sp: &StuckProject<'_>) -> String {
    let mut parts: Vec<String> = (sp.done > 0)
        .then(|| format!("{} done", sp.done))
        .into_iter()
        .chain(std::iter::once(format!("{} actionable", sp.actionable)))
        .collect();
    for (n, lbl) in [(sp.blocked_count, "blocked"), (sp.waiting, "waiting")] {
        if n > 0 {
            parts.push(format!("{n} {lbl}"));
        }
    }
    parts.join(", ")
}

/// Renders stuck projects as pretty JSON matching `org-stuck-output.schema.json`.
pub fn render_stuck_json(stuck: &[StuckProject<'_>]) -> String {
    let items: Vec<serde_json::Value> = stuck
        .iter()
        .map(|sp| {
            let entry = &sp.doc.entries[sp.entry_idx];
            serde_json::json!({
                "file": sp.doc.file.display().to_string(),
                "line": entry.heading_line,
                "locator": locator_for_entry(sp.doc, sp.entry_idx).to_string(),
                "keyword": entry.keyword,
                "title": entry.title,
                "tags": entry.tags,
                "children_done": sp.done,
                "children_actionable": sp.actionable,
                "children_blocked": sp.blocked_count,
                "children_waiting": sp.waiting,
            })
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}

// endregion
