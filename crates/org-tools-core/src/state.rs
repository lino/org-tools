// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Change TODO state on org-mode entries, with Org §8.3.2 repeater auto-advance.
//!
//! Replaces the TODO keyword on a heading line, manages `CLOSED:` planning
//! timestamps when transitioning between todo and done states, and advances
//! repeating active timestamps (`+`, `++`, `.+`) back to the initial TODO state.

use crate::diagnostic::{Fix, Span};
use crate::document::{OrgDocument, OrgEntry};
use crate::formatter::apply_fixes;
use crate::planning::{
    advance_line_active_repeaters, for_each_entry_content_line, is_planning_line_trimmed,
    rewrite_planning_line,
};
use crate::rules::heading::{parse_heading_with_keywords, HeadingParts};
use crate::source::SourceFile;

/// Re-exported civil calendar conversion helpers.
pub use crate::planning::{
    date_to_days as days_from_civil, day_of_week_abbrev as weekday_abbrev,
    days_to_date as civil_from_days,
};

// region Public Types

/// Result of a state change operation.
#[derive(Debug, Clone)]
pub struct SetStateResult {
    /// The modified file content.
    pub content: String,
    /// Number of entries whose state was changed.
    pub changed: usize,
    /// Entry indices in [`OrgDocument::entries`] whose TODO keyword or repeater changed.
    pub changed_indices: Vec<usize>,
    /// Entry indices that transitioned from a non-done state to a done state.
    pub newly_done_indices: Vec<usize>,
    /// Entry indices whose repeating timestamps auto-advanced and reset to initial TODO.
    pub repeated_indices: Vec<usize>,
}

/// Alias for [`SetStateResult`] carrying `changed_indices`, `newly_done_indices`, and `repeated_indices`.
pub type StateChangeResult = SetStateResult;

/// Options controlling `CLOSED:` timestamps and repeater auto-advance in [`set_state_with_options`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetStateOptions {
    /// Add or remove `CLOSED:` planning timestamps on non-repeating done transitions.
    pub add_closed: bool,
    /// Auto-advance repeating active timestamps instead of marking the entry permanently done.
    pub advance_repeaters: bool,
    /// Optional civil `(year, month, day)` override for repeater math (`None` uses local OS date).
    pub today: Option<(u16, u8, u8)>,
}

impl Default for SetStateOptions {
    /// Returns default options (`add_closed: true`, `advance_repeaters: true`, `today: None`).
    fn default() -> Self {
        Self {
            add_closed: true,
            advance_repeaters: true,
            today: None,
        }
    }
}

/// Outcome category for a single entry processed by [`set_state_with_options`].
enum EntryOutcome {
    /// Standard keyword transition (`true` when newly transitioned to a done state).
    Standard(Vec<Fix>, bool),
    /// Repeating entry auto-advanced and reset to the initial TODO state.
    Repeated(Vec<Fix>),
}

// endregion

// region Public API

/// Changes the TODO keyword on entries to `new_state`.
///
/// Wraps [`set_state_with_options`] with `advance_repeaters = true` and the local OS date.
pub fn set_state(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_indices: Option<&[usize]>,
    new_state: &str,
    add_closed: bool,
) -> Option<SetStateResult> {
    set_state_with_options(
        source,
        doc,
        entry_indices,
        new_state,
        SetStateOptions {
            add_closed,
            advance_repeaters: true,
            today: None,
        },
    )
}

/// Changes TODO state on entries using explicit [`SetStateOptions`] (including repeater auto-advance).
pub fn set_state_with_options(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_indices: Option<&[usize]>,
    new_state: &str,
    options: SetStateOptions,
) -> Option<StateChangeResult> {
    let lines: Vec<&str> = source.content.split('\n').collect();
    let kw_refs = doc.todo_keywords.all();
    let today = options.today.unwrap_or_else(current_local_date);
    let indices: Vec<usize> = match entry_indices {
        Some(idx) => idx.to_vec(),
        None => (0..doc.entries.len()).collect(),
    };
    let mut fixes: Vec<Fix> = Vec::new();
    let mut changed_indices = Vec::new();
    let mut newly_done_indices = Vec::new();
    let mut repeated_indices = Vec::new();
    for &idx in &indices {
        let Some(outcome) = build_entry_outcome(
            source, doc, &lines, &kw_refs, idx, new_state, options, today,
        ) else {
            continue;
        };
        changed_indices.push(idx);
        match outcome {
            EntryOutcome::Standard(entry_fixes, newly_done) => {
                fixes.extend(entry_fixes);
                if newly_done {
                    newly_done_indices.push(idx);
                }
            }
            EntryOutcome::Repeated(entry_fixes) => {
                fixes.extend(entry_fixes);
                repeated_indices.push(idx);
            }
        }
    }

    if changed_indices.is_empty() {
        return None;
    }
    fixes.sort_by_key(|f| f.span.start);
    let content = apply_fixes(&source.content, &fixes);
    Some(SetStateResult {
        content,
        changed: changed_indices.len(),
        changed_indices,
        newly_done_indices,
        repeated_indices,
    })
}

// endregion

// region Fix Builders

/// Collects non-overlapping repeater fixes across direct body lines of `idx`.
fn collect_body_repeater_fixes(
    source: &SourceFile,
    doc: &OrgDocument,
    idx: usize,
    head_idx: usize,
    today: (u16, u8, u8),
) -> Vec<Fix> {
    let mut body_fixes = Vec::new();
    for_each_entry_content_line(source, doc, idx, |li, raw| {
        if li > head_idx {
            let (u, ch) = advance_line_active_repeaters(raw, today);
            if ch {
                let s = line_offset(&source.content, li);
                body_fixes.push(Fix::new(Span::new(s, s + raw.len()), u));
            }
        }
    });
    body_fixes
}

/// Evaluates an entry's optional planning line for `CLOSED:` stripping and repeater advancement.
fn advance_planning_slot(
    lines: &[&str],
    plan_idx: usize,
    end_excl: usize,
    today: (u16, u8, u8),
) -> (bool, String, bool) {
    if plan_idx >= end_excl || !is_planning_line_trimmed(lines[plan_idx].trim()) {
        return (false, String::new(), false);
    }
    let stripped = rewrite_planning_line(lines[plan_idx], "CLOSED:", None);
    let (next_plan, plan_adv) = advance_line_active_repeaters(&stripped, today);
    (true, next_plan, plan_adv)
}

/// Builds non-overlapping repeater auto-advance fixes for `idx`, or returns `None` if non-repeating.
fn build_repeater_entry_fixes(
    source: &SourceFile,
    doc: &OrgDocument,
    lines: &[&str],
    idx: usize,
    parts: &HeadingParts<'_>,
    today: (u16, u8, u8),
) -> Option<Vec<Fix>> {
    let head_idx = doc.entries[idx].heading_line - 1;
    let next_h = doc.entries.get(idx + 1).map(|n| n.heading_line - 1);
    let end_excl = next_h.unwrap_or(lines.len()).min(lines.len());
    let (new_title, title_adv) = advance_line_active_repeaters(parts.title, today);
    let plan_idx = head_idx + 1;
    let (has_plan, next_plan, plan_adv) = advance_planning_slot(lines, plan_idx, end_excl, today);
    let body_fixes = collect_body_repeater_fixes(source, doc, idx, head_idx, today);
    if !(title_adv || plan_adv || !body_fixes.is_empty()) {
        return None;
    }
    let reset = doc
        .todo_keywords
        .todo
        .first()
        .map_or("TODO", String::as_str);
    let new_head = build_heading_with_state(parts, reset, &new_title);
    let mut fixes = Vec::new();
    if new_head != lines[head_idx] {
        let s = line_offset(&source.content, head_idx);
        fixes.push(Fix::new(Span::new(s, s + lines[head_idx].len()), new_head));
    }
    if has_plan && next_plan != lines[plan_idx] {
        let s = line_offset(&source.content, plan_idx);
        let e = (s + lines[plan_idx].len() + usize::from(next_plan.is_empty()))
            .min(source.content.len());
        fixes.push(Fix::new(Span::new(s, e), next_plan));
    }
    fixes.extend(body_fixes);
    Some(fixes)
}

/// Computes any `CLOSED:` addition or removal fix for a non-repeating state transition.
fn closed_fix_for_transition(
    entry: &OrgEntry,
    lines: &[&str],
    content: &str,
    was_done: bool,
    is_new_done: bool,
) -> Option<Fix> {
    if !was_done && is_new_done {
        add_closed_timestamp(entry, lines, content)
    } else if was_done && !is_new_done {
        remove_closed_timestamp(entry, lines, content)
    } else {
        None
    }
}

/// Computes either a repeater auto-advance or a standard keyword transition for `idx`.
#[allow(clippy::too_many_arguments)]
fn build_entry_outcome(
    source: &SourceFile,
    doc: &OrgDocument,
    lines: &[&str],
    kw_refs: &[&str],
    idx: usize,
    new_state: &str,
    options: SetStateOptions,
    today: (u16, u8, u8),
) -> Option<EntryOutcome> {
    let entry = doc.entries.get(idx)?;
    let heading_idx = entry.heading_line - 1;
    let heading_line = *lines.get(heading_idx)?;
    let parts = parse_heading_with_keywords(heading_line, kw_refs)?;
    if parts.keyword == Some(new_state) {
        return None;
    }

    let is_new_done = doc.todo_keywords.is_done(new_state);
    let was_done = parts
        .keyword
        .is_some_and(|kw| doc.todo_keywords.is_done(kw));
    let newly_done = !was_done && is_new_done;

    if options.advance_repeaters && newly_done {
        if let Some(rep) = build_repeater_entry_fixes(source, doc, lines, idx, &parts, today) {
            return Some(EntryOutcome::Repeated(rep));
        }
    }

    let mut entry_fixes = Vec::with_capacity(2);
    let new_heading = build_heading_with_state(&parts, new_state, parts.title);
    let line_start = line_offset(&source.content, heading_idx);
    entry_fixes.push(Fix::new(
        Span::new(line_start, line_start + heading_line.len()),
        new_heading,
    ));
    if options.add_closed {
        if let Some(fix) =
            closed_fix_for_transition(entry, lines, &source.content, was_done, is_new_done)
        {
            entry_fixes.push(fix);
        }
    }
    Some(EntryOutcome::Standard(entry_fixes, newly_done))
}

/// Builds a heading line with `new_state`, `title_text`, and `F007`-aligned tags.
fn build_heading_with_state(parts: &HeadingParts<'_>, new_state: &str, title_text: &str) -> String {
    let stars = "*".repeat(parts.level);
    let pri = parts
        .priority
        .map(|p| format!(" [#{p}]"))
        .unwrap_or_default();
    let title = if title_text.is_empty() {
        String::new()
    } else {
        format!(" {title_text}")
    };
    let prefix = format!("{stars} {new_state}{pri}{title}");
    let prefix_trimmed = prefix.trim_end();
    if parts.tags.is_empty() {
        return prefix_trimmed.to_string();
    }

    let tag_str = format!(":{}:", parts.tags.join(":"));
    let tag_col = crate::property::resolve_tag_column(None);
    let prefix_width = unicode_width::UnicodeWidthStr::width(prefix_trimmed);
    let tag_width = unicode_width::UnicodeWidthStr::width(tag_str.as_str());
    let spaces = if prefix_width + 1 + tag_width <= tag_col {
        tag_col - tag_width - prefix_width
    } else {
        1
    };
    format!("{prefix_trimmed}{}{tag_str}", " ".repeat(spaces))
}

/// Adds a `CLOSED:` timestamp after the heading or prepends it to an existing planning line.
fn add_closed_timestamp(entry: &OrgEntry, lines: &[&str], content: &str) -> Option<Fix> {
    let next_idx = entry.heading_line;
    let now = now_timestamp();
    if next_idx < lines.len() && is_planning_line_trimmed(lines[next_idx].trim()) {
        if !lines[next_idx].contains("CLOSED:") {
            let start = line_offset(content, next_idx);
            let indent = lines[next_idx].len() - lines[next_idx].trim_start().len();
            let prefix = &lines[next_idx][..indent];
            return Some(Fix::new(
                Span::new(start, start),
                format!("{prefix}CLOSED: [{now}] "),
            ));
        }
        return None;
    }
    let insert_offset = line_offset(content, next_idx);
    Some(Fix::new(
        Span::new(insert_offset, insert_offset),
        format!("CLOSED: [{now}]\n"),
    ))
}

/// Removes a `CLOSED:` timestamp from an entry's planning line.
fn remove_closed_timestamp(entry: &OrgEntry, lines: &[&str], content: &str) -> Option<Fix> {
    let next_idx = entry.heading_line;
    if next_idx >= lines.len() || !lines[next_idx].contains("CLOSED:") {
        return None;
    }
    let line_start = line_offset(content, next_idx);
    let updated = rewrite_planning_line(lines[next_idx], "CLOSED:", None);
    if updated.is_empty() {
        let line_end = (line_start + lines[next_idx].len() + 1).min(content.len());
        return Some(Fix::new(Span::new(line_start, line_end), String::new()));
    }
    Some(Fix::new(
        Span::new(line_start, line_start + lines[next_idx].len()),
        updated,
    ))
}

// endregion

// region Time Helpers

/// Extracts local `(year, month, day, hour, minute)` from Unix epoch `secs` via `libc`.
fn local_timestamp_parts(secs: i64) -> Option<(u16, u8, u8, u32, u32)> {
    #[cfg(unix)]
    let resolved = unsafe {
        let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
        let time_t = secs as libc::time_t;
        (!libc::localtime_r(&time_t, tm.as_mut_ptr()).is_null()).then(|| tm.assume_init())
    };
    #[cfg(windows)]
    let resolved = unsafe {
        let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
        let time_t = secs as libc::time_t;
        (libc::localtime_s(tm.as_mut_ptr(), &time_t) == 0).then(|| tm.assume_init())
    };
    #[cfg(not(any(unix, windows)))]
    let resolved: Option<libc::tm> = None;

    resolved.map(|tm| {
        (
            (tm.tm_year + 1900) as u16,
            (tm.tm_mon + 1) as u8,
            tm.tm_mday as u8,
            tm.tm_hour as u32,
            tm.tm_min as u32,
        )
    })
}

/// Returns today's local civil `(year, month, day)` date.
fn current_local_date() -> (u16, u8, u8) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    if let Some((y, m, d, _, _)) = local_timestamp_parts(secs) {
        return (y, m, d);
    }
    civil_from_days(secs / 86_400)
}

/// Returns a formatted Org inactive timestamp payload for the current local time (`YYYY-MM-DD Day HH:MM`).
pub fn now_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    if let Some((year, month, d, h, m)) = local_timestamp_parts(secs) {
        let day_name = weekday_abbrev(year, month, d);
        return format!("{year:04}-{month:02}-{d:02} {day_name} {h:02}:{m:02}");
    }
    let (year, month, d) = civil_from_days(secs / 86_400);
    let rem = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{d:02} {} {:02}:{:02}",
        weekday_abbrev(year, month, d),
        rem / 3_600,
        (rem % 3_600) / 60
    )
}

/// Calculates the byte offset of zero-based `line_idx` in `content`.
fn line_offset(content: &str, line_idx: usize) -> usize {
    content
        .split('\n')
        .take(line_idx)
        .map(|l| l.len() + 1)
        .sum()
}

// endregion
