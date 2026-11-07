// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=770 reason="Cohesive dispatcher and pure-Rust executors for all 10 org-edna TRIGGER actions"

//! Pure-Rust execution of org-edna `:TRIGGER:` actions (`todo!`, `set-property!`,
//! `delete-property!`, `tag!`, `scheduled!`, `deadline!`, `set-priority!`,
//! `set-effort!`, `chain!`, and `archive!`).
//!
//! Spec: <https://www.nongnu.org/org-edna-el/>

use std::path::PathBuf;

use crate::archive::parse_archive_target;
use crate::document::OrgDocument;
use crate::property::{
    delete_property, format_heading_with_tags, format_property_lines, parse_tag_input,
    resolve_tag_column, set_property, set_tags,
};
use crate::rules::heading::parse_heading_with_keywords;
use crate::rules::timestamp::OrgTimestamp;
use crate::source::SourceFile;
use crate::state::{civil_from_days, days_from_civil, now_timestamp, set_state, weekday_abbrev};

use super::parser::Action;

// region Constants & Types

/// Single entry mutation recorded during `:TRIGGER:` execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggeredMutation {
    /// Path to the modified Org file.
    pub file: PathBuf,
    /// One-based line number of the affected entry (or `1` for archive appends).
    pub line: usize,
    /// Machine-readable mutation action label (e.g., `"triggered-todo"`).
    pub action: String,
    /// Optional human-readable change summary.
    pub detail: Option<String>,
}

/// Output of applying one [`Action`] to a single resolved heading entry.
pub(crate) struct ActionApplyOutcome {
    /// Updated content of the target entry's source file, if changed.
    pub(crate) updated_content: Option<String>,
    /// Optional `(archive_path, new_archive_file_content)` produced by `archive!`.
    pub(crate) archive_update: Option<(PathBuf, String)>,
    /// Recorded mutation detail when an observable change occurred.
    pub(crate) mutation: Option<TriggeredMutation>,
    /// Whether this action transitioned the target entry from non-done to done.
    pub(crate) transitioned_to_done: bool,
}

impl ActionApplyOutcome {
    /// Returns a no-op outcome when an action produces no file changes.
    fn unchanged() -> Self {
        Self {
            updated_content: None,
            archive_update: None,
            mutation: None,
            transitioned_to_done: false,
        }
    }
}

/// Context describing the source entry whose `:TRIGGER:` is currently firing.
pub(crate) struct TriggerSourceInfo<'a> {
    /// Document containing the source entry.
    pub(crate) doc: &'a OrgDocument,
    /// Index of the source entry inside `doc.entries`.
    pub(crate) entry_idx: usize,
    /// Whether `CLOSED:` timestamps should be managed on `todo!` transitions.
    pub(crate) add_closed: bool,
}

// endregion

// region Action Dispatcher

/// Applies `action` to `entry_idx` in `(target_source, target_doc)`.
pub(crate) fn apply_action_to_entry(
    action: &Action,
    target_source: &SourceFile,
    target_doc: &OrgDocument,
    entry_idx: usize,
    source_info: &TriggerSourceInfo<'_>,
    existing_archive_content: Option<&str>,
) -> ActionApplyOutcome {
    if entry_idx >= target_doc.entries.len() {
        return ActionApplyOutcome::unchanged();
    }
    if let Some(res) =
        apply_property_or_tag_action(action, target_source, target_doc, entry_idx, source_info)
    {
        return res;
    }
    match action {
        Action::Todo(st) => apply_todo_action(
            target_source,
            target_doc,
            entry_idx,
            st,
            source_info.add_closed,
        ),
        Action::Scheduled(s) => {
            apply_planning_action(target_source, target_doc, entry_idx, "SCHEDULED", s)
        }
        Action::Deadline(s) => {
            apply_planning_action(target_source, target_doc, entry_idx, "DEADLINE", s)
        }
        Action::SetPriority(p) => apply_priority_action(target_source, target_doc, entry_idx, p),
        Action::Archive => apply_archive_action(
            target_source,
            target_doc,
            entry_idx,
            existing_archive_content,
        ),
        _ => ActionApplyOutcome::unchanged(),
    }
}

/// Dispatches property, effort, chain, and tag actions when `action` belongs to that group.
fn apply_property_or_tag_action(
    action: &Action,
    src: &SourceFile,
    doc: &OrgDocument,
    idx: usize,
    source_info: &TriggerSourceInfo<'_>,
) -> Option<ActionApplyOutcome> {
    match action {
        Action::SetProperty(k, v) => Some(apply_set_property_action(
            src,
            doc,
            idx,
            k,
            v,
            "triggered-property",
        )),
        Action::DeleteProperty(k) => Some(apply_delete_property_action(src, doc, idx, k)),
        Action::Tag(t) => Some(apply_tag_action(src, doc, idx, t)),
        Action::SetEffort(e) => Some(apply_set_property_action(
            src,
            doc,
            idx,
            "EFFORT",
            e,
            "triggered-effort",
        )),
        Action::Chain(prop) => {
            let Some(val) = source_info.doc.property(source_info.entry_idx, prop) else {
                return Some(ActionApplyOutcome::unchanged());
            };
            Some(apply_set_property_action(
                src,
                doc,
                idx,
                prop,
                val,
                "triggered-chain",
            ))
        }
        _ => None,
    }
}

// endregion

// region Heading, Property & Tag Actions

/// Executes `todo!("STATE")`, preserving `F007` tag alignment and detecting newly-done transitions.
fn apply_todo_action(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    new_state: &str,
    add_closed: bool,
) -> ActionApplyOutcome {
    let prev_kw = doc.entries[entry_idx]
        .keyword
        .as_deref()
        .unwrap_or("<none>")
        .to_string();
    let existing_tags = doc.entries[entry_idx].tags.clone();

    let Some(res) = set_state(source, doc, Some(&[entry_idx]), new_state, add_closed) else {
        return ActionApplyOutcome::unchanged();
    };
    let mut final_content = res.content;
    if !existing_tags.is_empty() {
        let mid_src = SourceFile::new(source.path.clone(), final_content.clone());
        let mid_doc = OrgDocument::from_source(&mid_src);
        if let Ok(aligned) = set_tags(
            &mid_src,
            &mid_doc,
            entry_idx,
            &existing_tags,
            resolve_tag_column(None),
        ) {
            final_content = aligned.content;
        }
    }

    let after_src = SourceFile::new(source.path.clone(), final_content.clone());
    let after_doc = OrgDocument::from_source(&after_src);
    let line = after_doc
        .entries
        .get(entry_idx)
        .map_or(doc.entries[entry_idx].heading_line, |e| e.heading_line);

    ActionApplyOutcome {
        updated_content: Some(final_content),
        archive_update: None,
        mutation: Some(TriggeredMutation {
            file: source.path.clone(),
            line,
            action: "triggered-todo".to_string(),
            detail: Some(format!("{prev_kw} -> {new_state}")),
        }),
        transitioned_to_done: res.newly_done_indices.contains(&entry_idx),
    }
}

/// Executes `set-property!`, `set-effort!`, or `chain!` with `F005` drawer alignment.
fn apply_set_property_action(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    key: &str,
    val: &str,
    action_label: &str,
) -> ActionApplyOutcome {
    let Ok(res) = set_property(source, doc, entry_idx, key, val) else {
        return ActionApplyOutcome::unchanged();
    };
    if !res.changed {
        return ActionApplyOutcome::unchanged();
    }
    ActionApplyOutcome {
        updated_content: Some(res.content),
        archive_update: None,
        mutation: Some(TriggeredMutation {
            file: source.path.clone(),
            line: res.line,
            action: action_label.to_string(),
            detail: Some(format!("{key}={val}")),
        }),
        transitioned_to_done: false,
    }
}

/// Executes `delete-property!("KEY")` with `F005` drawer realignment.
fn apply_delete_property_action(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    key: &str,
) -> ActionApplyOutcome {
    let Ok(res) = delete_property(source, doc, entry_idx, key) else {
        return ActionApplyOutcome::unchanged();
    };
    if !res.changed {
        return ActionApplyOutcome::unchanged();
    }
    ActionApplyOutcome {
        updated_content: Some(res.content),
        archive_update: None,
        mutation: Some(TriggeredMutation {
            file: source.path.clone(),
            line: res.line,
            action: "triggered-delete-property".to_string(),
            detail: Some(key.to_string()),
        }),
        transitioned_to_done: false,
    }
}

/// Executes `tag!("...")` aligned to column 77 (`F007`).
fn apply_tag_action(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    raw_tags: &str,
) -> ActionApplyOutcome {
    let clear = raw_tags.trim().is_empty() || raw_tags.eq_ignore_ascii_case("none");
    let Ok(parsed_tags) = parse_tag_input(Some(raw_tags), clear) else {
        return ActionApplyOutcome::unchanged();
    };
    let Ok(res) = set_tags(
        source,
        doc,
        entry_idx,
        &parsed_tags,
        resolve_tag_column(None),
    ) else {
        return ActionApplyOutcome::unchanged();
    };
    if !res.changed {
        return ActionApplyOutcome::unchanged();
    }
    ActionApplyOutcome {
        updated_content: Some(res.content),
        archive_update: None,
        mutation: Some(TriggeredMutation {
            file: source.path.clone(),
            line: res.line,
            action: "triggered-tag".to_string(),
            detail: Some(raw_tags.to_string()),
        }),
        transitioned_to_done: false,
    }
}

/// Splits `source.content` into normalized LF lines.
fn split_source_lines(source: &SourceFile) -> Vec<String> {
    source
        .content
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect()
}

/// Parses a priority argument (`"A"`, `"B"`, `"C"`, or `"none"` / `"remove"`).
fn parse_priority_arg(trimmed: &str) -> Option<char> {
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("none")
        || trimmed.eq_ignore_ascii_case("remove")
    {
        None
    } else {
        trimmed.chars().next().map(|c| c.to_ascii_uppercase())
    }
}

/// Executes `set-priority!("A"|"B"|"C"|"none")`, preserving `F007` tag alignment.
fn apply_priority_action(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    raw_pri: &str,
) -> ActionApplyOutcome {
    let entry = &doc.entries[entry_idx];
    let heading_idx = entry.heading_line - 1;
    let mut lines = split_source_lines(source);
    let kw_refs = doc.todo_keywords.all();
    let Some(parts) = parse_heading_with_keywords(&lines[heading_idx], &kw_refs) else {
        return ActionApplyOutcome::unchanged();
    };

    let trimmed = raw_pri.trim();
    let mut rebuilt = format!("{} ", "*".repeat(parts.level));
    if let Some(kw) = parts.keyword {
        rebuilt.push_str(kw);
        rebuilt.push(' ');
    }
    if let Some(p) = parse_priority_arg(trimmed) {
        rebuilt.push_str(&format!("[#{p}] "));
    }
    rebuilt.push_str(parts.title);
    let base_heading = rebuilt.trim_end().to_string();
    let tag_vec: Vec<String> = parts.tags.iter().map(|t| (*t).to_string()).collect();
    lines[heading_idx] =
        format_heading_with_tags(&base_heading, &kw_refs, &tag_vec, resolve_tag_column(None))
            .unwrap_or(base_heading);

    let updated = lines.join("\n");
    if updated == source.content {
        return ActionApplyOutcome::unchanged();
    }
    ActionApplyOutcome {
        updated_content: Some(updated),
        archive_update: None,
        mutation: Some(TriggeredMutation {
            file: source.path.clone(),
            line: entry.heading_line,
            action: "triggered-priority".to_string(),
            detail: Some(trimmed.to_string()),
        }),
        transitioned_to_done: false,
    }
}

// endregion

// region Planning Timestamp & Archive Actions

/// Formats a `TIMESPEC` string into an active Org timestamp `<YYYY-MM-DD Day>`, or `None` to clear.
fn resolve_planning_timestamp(
    spec: &str,
    existing: Option<&OrgTimestamp>,
) -> Option<Option<String>> {
    let trimmed = spec.trim();
    if trimmed.is_empty()
        || matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "rm" | "remove" | "clear" | "none"
        )
    {
        return Some(None);
    }
    if trimmed.starts_with('<') && trimmed.ends_with('>') {
        return Some(Some(trimmed.to_string()));
    }
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return Some(Some(format!("<{}>", &trimmed[1..trimmed.len() - 1])));
    }
    if let Some(iso_ts) = parse_iso_date_spec(trimmed) {
        return Some(Some(iso_ts));
    }
    parse_relative_date_spec(trimmed, existing).map(Some)
}

/// Parses `"YYYY-MM-DD"` or `"YYYY-MM-DD HH:MM"` into `<YYYY-MM-DD Day[ HH:MM]>`.
fn parse_iso_date_spec(trimmed: &str) -> Option<String> {
    let (date_part, time_part) = match trimmed.split_once(' ') {
        Some((d, t)) => (d.trim(), Some(t.trim())),
        None => (trimmed, None),
    };
    let mut segs = date_part.split('-');
    let year: u16 = segs.next()?.parse().ok()?;
    let month: u8 = segs.next()?.parse().ok()?;
    let day: u8 = segs.next()?.parse().ok()?;
    if segs.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let dow = weekday_abbrev(year, month, day);
    match time_part {
        Some(t) if !t.is_empty() => Some(format!("<{year:04}-{month:02}-{day:02} {dow} {t}>")),
        _ => Some(format!("<{year:04}-{month:02}-{day:02} {dow}>")),
    }
}

/// Formats an epoch-day count into `<YYYY-MM-DD Day>`.
fn format_epoch_days(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("<{y:04}-{m:02}-{d:02} {}>", weekday_abbrev(y, m, d))
}

/// Converts a unit suffix (`"d"`, `"w"`, `"m"`, `"y"`) and `delta` into day offset.
fn unit_day_offset(unit: &str, delta: i64) -> Option<i64> {
    match unit {
        "d" => Some(delta),
        "w" => Some(delta * 7),
        "m" => Some(delta * 30),
        "y" => Some(delta * 365),
        _ => None,
    }
}

/// Parses `"today"`, `"tomorrow"`, `"+Nd"`, `".+Nw"`, `"++Nm"`, or `"+Ny"` into `<YYYY-MM-DD Day>`.
fn parse_relative_date_spec(trimmed: &str, existing: Option<&OrgTimestamp>) -> Option<String> {
    let now_days = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400) as i64;
    let lower = trimmed.to_ascii_lowercase();
    if lower == "today" {
        return Some(format_epoch_days(now_days));
    }
    if lower == "tomorrow" {
        return Some(format_epoch_days(now_days + 1));
    }

    let (use_existing, rest) = match lower.strip_prefix("++") {
        Some(r) => (true, r),
        None => (false, lower.strip_prefix(".+").unwrap_or(lower.as_str())),
    };
    let rest = rest.strip_prefix('+').unwrap_or(rest);
    if rest.len() < 2 {
        return None;
    }
    let (num_str, unit) = rest.split_at(rest.len() - 1);
    let delta: i64 = num_str.parse().ok()?;
    let base_days = match (use_existing, existing) {
        (true, Some(ts)) => days_from_civil(ts.year, ts.month, ts.day),
        _ => now_days,
    };
    Some(format_epoch_days(base_days + unit_day_offset(unit, delta)?))
}

/// Applies `scheduled!("...")` or `deadline!("...")` on the planning line right below the heading.
fn apply_planning_action(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    keyword: &str,
    spec: &str,
) -> ActionApplyOutcome {
    let entry = &doc.entries[entry_idx];
    let existing_ts = if keyword == "SCHEDULED" {
        entry.planning.scheduled.as_ref()
    } else {
        entry.planning.deadline.as_ref()
    };
    let Some(resolved_ts) = resolve_planning_timestamp(spec, existing_ts) else {
        return ActionApplyOutcome::unchanged();
    };

    let mut lines = split_source_lines(source);
    let plan_idx = entry.heading_line;
    if lines.get(plan_idx).is_some_and(|l| is_planning_line(l)) {
        let updated_line = update_planning_line(&lines[plan_idx], keyword, resolved_ts.as_deref());
        if updated_line.trim().is_empty() {
            lines.remove(plan_idx);
        } else {
            lines[plan_idx] = updated_line;
        }
    } else if let Some(ts) = resolved_ts.as_deref() {
        lines.insert(plan_idx, format!("{keyword}: {ts}"));
    } else {
        return ActionApplyOutcome::unchanged();
    }

    let updated = lines.join("\n");
    if updated == source.content {
        return ActionApplyOutcome::unchanged();
    }
    let label = if keyword == "SCHEDULED" {
        "triggered-scheduled"
    } else {
        "triggered-deadline"
    };
    ActionApplyOutcome {
        updated_content: Some(updated),
        archive_update: None,
        mutation: Some(TriggeredMutation {
            file: source.path.clone(),
            line: entry.heading_line,
            action: label.to_string(),
            detail: Some(spec.to_string()),
        }),
        transitioned_to_done: false,
    }
}

/// Checks whether `line` starts with an Org planning keyword.
fn is_planning_line(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("SCHEDULED:") || t.starts_with("DEADLINE:") || t.starts_with("CLOSED:")
}

/// Updates or removes `keyword` (`SCHEDULED` or `DEADLINE`) inside an existing planning line.
fn update_planning_line(existing_line: &str, keyword: &str, new_ts: Option<&str>) -> String {
    let indent_len = existing_line.len() - existing_line.trim_start().len();
    let indent = &existing_line[..indent_len];
    let closed = extract_planning_item(existing_line, "CLOSED:");
    let mut scheduled = extract_planning_item(existing_line, "SCHEDULED:");
    let mut deadline = extract_planning_item(existing_line, "DEADLINE:");

    if keyword == "SCHEDULED" {
        scheduled = new_ts.map(str::to_string);
    } else {
        deadline = new_ts.map(str::to_string);
    }

    let mut parts = Vec::new();
    if let Some(c) = closed {
        parts.push(format!("CLOSED: {c}"));
    }
    if let Some(s) = scheduled {
        parts.push(format!("SCHEDULED: {s}"));
    }
    if let Some(d) = deadline {
        parts.push(format!("DEADLINE: {d}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("{indent}{}", parts.join(" "))
    }
}

/// Extracts the `<...>` or `[...]` timestamp token following `label` on a planning line.
fn extract_planning_item(line: &str, label: &str) -> Option<String> {
    let pos = line.find(label)?;
    let rest = line[pos + label.len()..].trim_start();
    let close_ch = if rest.starts_with('<') {
        '>'
    } else if rest.starts_with('[') {
        ']'
    } else {
        return None;
    };
    let end = rest.find(close_ch)?;
    Some(rest[..=end].to_string())
}

/// Checks whether `line` is an Org heading (`*` stars followed by space or EOL).
fn is_heading_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('*') {
        return false;
    }
    let rest = trimmed.trim_start_matches('*');
    rest.is_empty() || rest.starts_with(' ')
}

/// Appends `archived_block` onto the existing (or newly initialized) archive text (`F003`-clean).
fn append_to_archive_buffer(
    base_archive: String,
    heading: Option<&str>,
    archived_block: &str,
) -> String {
    let block = archived_block.trim_end_matches(['\n', '\r']);
    let trimmed_base = base_archive.trim_end_matches(['\n', '\r']);
    if trimmed_base.trim().is_empty() {
        return match heading {
            Some(h) => format!("{h}\n{block}\n"),
            None => format!("{block}\n"),
        };
    }
    let prev_is_heading = trimmed_base
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .is_some_and(is_heading_line);
    if prev_is_heading {
        format!("{trimmed_base}\n{block}\n")
    } else {
        format!("{trimmed_base}\n\n{block}\n")
    }
}

/// Archives `entry_idx` out of `source` and appends it to its resolved archive file.
fn apply_archive_action(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    existing_archive_content: Option<&str>,
) -> ActionApplyOutcome {
    let entry = &doc.entries[entry_idx];
    let raw_target = doc
        .property(entry_idx, "ARCHIVE")
        .or_else(|| doc.file_keywords.get("ARCHIVE").map(String::as_str))
        .unwrap_or("");
    let target = parse_archive_target(raw_target, &doc.file);
    let archive_path = target.file.clone().unwrap_or_else(|| doc.file.clone());

    let lines: Vec<&str> = source.content.split('\n').collect();
    let start = (entry.heading_line - 1).min(lines.len());
    let end = (entry.subtree_end_line - 1).min(lines.len());
    let archived_block = build_archived_subtree(doc, entry_idx, &lines[start..end]);

    let mut remaining = lines;
    remaining.drain(start..end);
    while remaining.len() >= 2
        && remaining.last() == Some(&"")
        && remaining.get(remaining.len() - 2) == Some(&"")
    {
        remaining.pop();
    }
    let base_archive = existing_archive_content
        .map(str::to_string)
        .or_else(|| std::fs::read_to_string(&archive_path).ok())
        .unwrap_or_default();
    let new_archive =
        append_to_archive_buffer(base_archive, target.heading.as_deref(), &archived_block);

    ActionApplyOutcome {
        updated_content: Some(remaining.join("\n")),
        archive_update: Some((archive_path, new_archive)),
        mutation: Some(TriggeredMutation {
            file: source.path.clone(),
            line: entry.heading_line,
            action: "triggered-archive".to_string(),
            detail: Some(entry.title.clone()),
        }),
        transitioned_to_done: false,
    }
}

/// Parsed header/properties of an existing `:PROPERTIES:` drawer during archiving.
struct ExistingDrawer {
    /// Number of lines consumed from `:PROPERTIES:` through `:END:`.
    consumed: usize,
    /// Leading whitespace prefix of the drawer.
    indent: String,
    /// Existing `(key, value)` pairs inside the drawer.
    props: Vec<(String, String)>,
}

/// Parses `:KEY: val` lines from an existing `:PROPERTIES:` drawer at `lines[0]`.
fn parse_existing_drawer(lines: &[&str]) -> Option<ExistingDrawer> {
    let first = *lines.first()?;
    if !first.trim().eq_ignore_ascii_case(":PROPERTIES:") {
        return None;
    }
    let indent = first[..first.len() - first.trim_start().len()].to_string();
    let mut props = Vec::new();
    for (idx, &line) in lines.iter().enumerate().skip(1) {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(":END:") {
            return Some(ExistingDrawer {
                consumed: idx + 1,
                indent,
                props,
            });
        }
        if let Some(rest) = trimmed.strip_prefix(':') {
            if let Some(colon) = rest.find(':') {
                let key = &rest[..colon];
                if !key.is_empty() && !key.chars().any(char::is_whitespace) {
                    props.push((key.to_string(), rest[colon + 1..].trim().to_string()));
                }
            }
        }
    }
    None
}

/// Formats an archived entry subtree with `F005`-aligned `:ARCHIVE_*:` drawer lines.
fn build_archived_subtree(doc: &OrgDocument, entry_idx: usize, subtree_lines: &[&str]) -> String {
    let now = now_timestamp();
    let olpath = doc.outline_path(entry_idx).join("/");
    let mut out = String::new();
    let Some((&first, rest)) = subtree_lines.split_first() else {
        return out;
    };
    out.push_str(first);
    out.push('\n');

    let mut cursor = 0;
    if cursor < rest.len() && is_planning_line(rest[cursor]) {
        out.push_str(rest[cursor]);
        out.push('\n');
        cursor += 1;
    }

    let mut props = vec![
        ("ARCHIVE_TIME".to_string(), now),
        ("ARCHIVE_FILE".to_string(), doc.file.display().to_string()),
        ("ARCHIVE_OLPATH".to_string(), olpath),
    ];
    let indent = if let Some(existing) = parse_existing_drawer(&rest[cursor..]) {
        cursor += existing.consumed;
        props.extend(existing.props);
        existing.indent
    } else {
        String::new()
    };

    out.push_str(&format!("{indent}:PROPERTIES:\n"));
    for formatted in format_property_lines(&props, &indent) {
        out.push_str(&formatted);
        out.push('\n');
    }
    out.push_str(&format!("{indent}:END:\n"));
    for line in &rest[cursor..] {
        out.push_str(line);
        out.push('\n');
    }
    out
}

// endregion
