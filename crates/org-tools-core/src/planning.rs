// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Org-mode timestamp repeaters, deadline warning delays, active body timestamp
//! extraction, and `SCHEDULED:` / `DEADLINE:` planning line mutations.
//!
//! Spec: [§8.3 Deadlines and Scheduling](https://orgmode.org/manual/Deadlines-and-Scheduling.html),
//! [§8.3.2 Repeated tasks](https://orgmode.org/manual/Repeated-tasks.html),
//! [Syntax: Planning](https://orgmode.org/worg/org-syntax.html#Planning),
//! [Syntax: Timestamps](https://orgmode.org/worg/org-syntax.html#Timestamps).

use crate::document::OrgDocument;
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::timestamp::{
    find_timestamps, inline_verbatim_spans, is_valid_date, parse_value_and_unit, OrgTimestamp,
};
use crate::source::SourceFile;

/// Re-exported repeater, warning, and proleptic Gregorian calendar primitives.
pub use crate::rules::timestamp::{
    date_to_days, day_of_week_abbrev, days_to_date, effective_deadline_warning,
    effective_deadline_warning_days, is_first_only_warning, parse_repeater, parse_warning,
    shift_calendar_date, RepeaterKind, RepeaterSpec, TimeUnit, WarningKind, WarningSpec,
};

// region Public Types

/// Active plain timestamp discovered on an entry's heading or direct body line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyTimestamp {
    /// One-based line number in [`SourceFile`].
    pub line: usize,
    /// Parsed active [`OrgTimestamp`].
    pub timestamp: OrgTimestamp,
}

/// Target planning keyword for [`set_planning_timestamp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanningKind {
    /// `SCHEDULED:` planning timestamp.
    Scheduled,
    /// `DEADLINE:` planning timestamp.
    Deadline,
}

/// Result of setting or clearing a `SCHEDULED:` or `DEADLINE:` planning timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanningMutation {
    /// Updated full document content.
    pub content: String,
    /// Whether the document content changed relative to the input source.
    pub changed: bool,
    /// One-based heading line number of the mutated entry.
    pub line: usize,
    /// Previous `<...>` timestamp string for `kind`, if present.
    pub old_timestamp: Option<String>,
    /// New `<...>` timestamp string for `kind`, or `None` when cleared.
    pub new_timestamp: Option<String>,
}

// endregion

// region Repeater Advancement & Projection

/// Steps `anchor` forward by `steps * spec.value` units, preserving `0..24` hours for hourly shifts.
fn step_date_or_hour(
    ts: &OrgTimestamp,
    anchor: (u16, u8, u8),
    steps: i64,
    spec: RepeaterSpec,
) -> ((u16, u8, u8), Option<u8>, Option<u8>) {
    let total = steps.saturating_mul(i64::from(spec.value));
    if spec.unit == TimeUnit::Hour {
        let base_h =
            date_to_days(anchor.0, anchor.1, anchor.2) * 24 + i64::from(ts.hour.unwrap_or(0));
        let next_h = base_h + total;
        let h = next_h.rem_euclid(24) as u8;
        let hm = if ts.hour.is_none() && h == 0 {
            (None, None)
        } else {
            (Some(h), Some(ts.minute.unwrap_or(0)))
        };
        return (days_to_date(next_h.div_euclid(24)), hm.0, hm.1);
    }
    let delta = total.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let next_date = shift_calendar_date(anchor.0, anchor.1, anchor.2, delta, spec.unit);
    (next_date, ts.hour, ts.minute)
}

/// Computes a conservative lower-bound step index `k >= 1` near `min_days`.
fn initial_step_estimate(ts: &OrgTimestamp, min_days: i64, spec: RepeaterSpec) -> i64 {
    let diff = min_days - date_to_days(ts.year, ts.month, ts.day);
    if diff <= 0 {
        return 1;
    }
    let (v, h) = (i64::from(spec.value), i64::from(ts.hour.unwrap_or(0)));
    let max_unit_days = match spec.unit {
        TimeUnit::Hour => return ((diff * 24 - h).max(1) / v).max(1),
        TimeUnit::Day => v,
        TimeUnit::Week => v * 7,
        TimeUnit::Month => v * 31,
        TimeUnit::Year => v * 366,
    };
    (diff / max_unit_days).max(1)
}

/// Finds the smallest `k >= 1` such that `base + k * spec` lands on or after `min_days`.
fn catch_up_steps(ts: &OrgTimestamp, min_days: i64, spec: RepeaterSpec) -> i64 {
    let base = (ts.year, ts.month, ts.day);
    let mut k = initial_step_estimate(ts, min_days, spec);
    loop {
        let (c, _, _) = step_date_or_hour(ts, base, k, spec);
        if date_to_days(c.0, c.1, c.2) >= min_days || c.0 == u16::MAX {
            return k;
        }
        k += 1;
    }
}

/// Advances `ts` according to its repeater (`+`, `++`, `.+`) relative to `today`.
pub fn advance_timestamp_repeater(ts: &OrgTimestamp, today: (u16, u8, u8)) -> Option<OrgTimestamp> {
    let spec = parse_repeater(ts.repeater.as_deref()?).filter(|s| s.value > 0)?;
    let base = (ts.year, ts.month, ts.day);
    let min_d = date_to_days(today.0, today.1, today.2) + 1;
    let (anchor, steps) = match spec.kind {
        RepeaterKind::Cumulative => (base, 1),
        RepeaterKind::Restart => (today, 1),
        RepeaterKind::CatchUp => (base, catch_up_steps(ts, min_d, spec)),
    };
    let ((year, month, day), hour, minute) = step_date_or_hour(ts, anchor, steps, spec);
    Some(OrgTimestamp {
        year,
        month,
        day,
        dayname: Some(day_of_week_abbrev(year, month, day).to_string()),
        hour,
        minute,
        ..ts.clone()
    })
}

/// Projects sorted, deduplicated repeating `(year, month, day)` dates strictly after `ts` within `[start_date, start_date + num_days)`.
pub fn project_repeating_dates(
    ts: &OrgTimestamp,
    start_date: (u16, u8, u8),
    num_days: usize,
) -> Vec<(u16, u8, u8)> {
    let opt = ts.repeater.as_deref().and_then(parse_repeater);
    let Some(spec) = opt.filter(|s| s.value > 0 && num_days > 0) else {
        return Vec::new();
    };
    let base = (ts.year, ts.month, ts.day);
    let start_days = date_to_days(start_date.0, start_date.1, start_date.2);
    let end_days = start_days + num_days as i64;
    let min_days = start_days.max(date_to_days(base.0, base.1, base.2) + 1);
    if min_days >= end_days {
        return Vec::new();
    }
    let (mut k, mut out) = (initial_step_estimate(ts, min_days, spec), Vec::new());
    loop {
        let (cand, _, _) = step_date_or_hour(ts, base, k, spec);
        let d = date_to_days(cand.0, cand.1, cand.2);
        if d >= end_days {
            break;
        }
        if d >= min_days && out.last() != Some(&cand) {
            out.push(cand);
        }
        if cand.0 == u16::MAX {
            break;
        }
        k += 1;
    }
    out
}

// endregion

// region Body Timestamp Scanner

/// Returns `true` if `trimmed` starts with `SCHEDULED:`, `DEADLINE:`, or `CLOSED:`.
pub(crate) fn is_planning_line_trimmed(trimmed: &str) -> bool {
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

/// Finds active, calendrically valid timestamps outside inline `=...=`/`~...~` spans on `line`.
fn active_unquoted_timestamps(line: &str) -> Vec<(OrgTimestamp, usize, usize)> {
    let v = inline_verbatim_spans(line);
    let keep = |&(ref ts, s, _): &(OrgTimestamp, usize, usize)| {
        ts.active
            && is_valid_date(ts.year, ts.month, ts.day)
            && !v.iter().any(|&(a, b)| s >= a && s < b)
    };
    find_timestamps(line).into_iter().filter(keep).collect()
}

/// Rewrites active, calendrically valid repeating timestamps outside inline verbatim spans on `line`.
pub(crate) fn advance_line_active_repeaters(line: &str, today: (u16, u8, u8)) -> (String, bool) {
    let (mut out, mut cursor, mut advanced) = (String::with_capacity(line.len()), 0, false);
    for (ts, start, end) in active_unquoted_timestamps(line) {
        if let Some(next_ts) = advance_timestamp_repeater(&ts, today) {
            out.push_str(&line[cursor..start]);
            out.push_str(&next_ts.to_string());
            cursor = end;
            advanced = true;
        }
    }
    if !advanced {
        return (line.to_string(), false);
    }
    out.push_str(&line[cursor..]);
    (out, true)
}

/// Visits unprotected, non-drawer, non-planning direct lines `(line_idx, line)` of `idx`.
pub(crate) fn for_each_entry_content_line(
    src: &SourceFile,
    doc: &OrgDocument,
    idx: usize,
    mut f: impl FnMut(usize, &str),
) {
    let Some(entry) = doc.entries.get(idx) else {
        return;
    };
    let lines: Vec<&str> = src.content.split('\n').collect();
    let protected = protected_regions(&src.content);
    let head_idx = entry.heading_line - 1;
    let next_h = doc.entries.get(idx + 1).map(|n| n.heading_line - 1);
    let end_excl = next_h.unwrap_or(lines.len()).min(lines.len());
    let mut in_drawer = false;
    for (li, &raw_line) in lines.iter().enumerate().take(end_excl).skip(head_idx) {
        if is_protected(li, &protected) {
            continue;
        }
        let t = raw_line.trim();
        if in_drawer {
            in_drawer = !t.eq_ignore_ascii_case(":END:");
            continue;
        }
        if li > head_idx
            && (t.eq_ignore_ascii_case(":PROPERTIES:") || t.eq_ignore_ascii_case(":LOGBOOK:"))
        {
            in_drawer = true;
            continue;
        }
        if li == head_idx || !is_planning_line_trimmed(t) {
            f(li, raw_line);
        }
    }
}

/// Extracts active, calendrically valid `<...>` timestamps from `idx`'s heading and direct body lines.
pub fn extract_entry_active_body_timestamps(
    src: &SourceFile,
    doc: &OrgDocument,
    idx: usize,
) -> Vec<BodyTimestamp> {
    let mut out = Vec::new();
    for_each_entry_content_line(src, doc, idx, |li, raw_line| {
        for (timestamp, _, _) in active_unquoted_timestamps(raw_line) {
            out.push(BodyTimestamp {
                line: li + 1,
                timestamp,
            });
        }
    });
    out
}

// endregion

// region Planning Line Mutations

/// Resolves the leading date token (`.`/`today`, `[+|-]N<d|w|m|y>`, or `YYYY-MM-DD`) against `today`.
fn resolve_date_token(tok: &str, today: (u16, u8, u8)) -> Result<(u16, u8, u8), String> {
    if tok == "." || tok.eq_ignore_ascii_case("today") {
        return is_valid_date(today.0, today.1, today.2)
            .then_some(today)
            .ok_or_else(|| "invalid reference date".to_string());
    }
    if matches!(tok.as_bytes().first(), Some(b'+' | b'-')) {
        let sign = if tok.starts_with('-') { -1_i32 } else { 1_i32 };
        let (v, u) = parse_value_and_unit(&tok[1..].to_ascii_lowercase())
            .filter(|(_, u)| *u != TimeUnit::Hour)
            .ok_or_else(|| format!("invalid relative date offset: '{tok}'"))?;
        let d = sign.saturating_mul(v.min(i32::MAX as u32) as i32);
        return Ok(shift_calendar_date(today.0, today.1, today.2, d, u));
    }
    let mut it = tok.split('-');
    if tok.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        if let (Some(ys), Some(ms), Some(ds), None) = (it.next(), it.next(), it.next(), it.next()) {
            if let (Ok(y), Ok(m), Ok(d)) = (ys.parse(), ms.parse(), ds.parse()) {
                if is_valid_date(y, m, d) {
                    return Ok((y, m, d));
                }
            }
        }
    }
    Err(format!("invalid calendar date: '{tok}'"))
}

/// Applies one trailing `[Day] | [HH:MM] | [+repeater] | [-warning]` token onto `ts`.
fn apply_spec_token(ts: &mut OrgTimestamp, tok: &str) -> Result<(), String> {
    if let Some((hs, ms)) = tok.split('-').next().unwrap_or(tok).split_once(':') {
        let (Ok(h), Ok(m)) = (hs.parse::<u8>(), ms.parse::<u8>()) else {
            return Err(format!("invalid time token: '{tok}'"));
        };
        if ms.contains(':') || h >= 24 || m >= 60 {
            return Err(format!("out-of-range time token: '{tok}'"));
        }
        (ts.hour, ts.minute) = (Some(h), Some(m));
    } else if tok.starts_with('+') || tok.starts_with(".+") {
        parse_repeater(tok)
            .filter(|r| r.value > 0)
            .ok_or_else(|| format!("invalid repeater token: '{tok}'"))?;
        ts.repeater = Some(tok.to_string());
    } else if tok.starts_with('-') {
        parse_warning(tok).ok_or_else(|| format!("invalid warning token: '{tok}'"))?;
        ts.warning = Some(tok.to_string());
    } else if !tok.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(format!("unrecognized planning token: '{tok}'"));
    }
    Ok(())
}

/// Parses a non-clear `spec` string into a canonical active [`OrgTimestamp`].
fn parse_planning_spec(raw: &str, today: (u16, u8, u8)) -> Result<OrgTimestamp, String> {
    let inner = if raw.starts_with('<') {
        if !raw.ends_with('>') || raw[1..raw.len() - 1].contains('>') {
            return Err(format!("malformed active timestamp: '{raw}'"));
        }
        &raw[1..raw.len() - 1]
    } else if raw.starts_with('[') {
        return Err(format!("planning timestamp must be active: '{raw}'"));
    } else {
        raw
    };
    let parts: Vec<&str> = inner.split_whitespace().collect();
    let Some(&first) = parts.first() else {
        return Err("empty planning timestamp specification".to_string());
    };
    let (year, month, day) = resolve_date_token(first, today)?;
    let mut ts = OrgTimestamp::active_date(year, month, day);
    for &tok in &parts[1..] {
        apply_spec_token(&mut ts, tok)?;
    }
    Ok(ts)
}

/// Extracts ordered `(KEYWORD:, <...>|[...])` pairs from an existing planning line.
pub(crate) fn parse_planning_items(line: &str) -> Vec<(&'static str, String)> {
    let mut found: Vec<(usize, &'static str, String)> = ["CLOSED:", "SCHEDULED:", "DEADLINE:"]
        .into_iter()
        .filter_map(|kw| {
            let pos = line.find(kw)?;
            let rest = line[pos + kw.len()..].trim_start();
            let close = match rest.as_bytes().first()? {
                b'<' => '>',
                b'[' => ']',
                _ => return None,
            };
            Some((pos, kw, rest[..=rest.find(close)?].to_string()))
        })
        .collect();
    found.sort_by_key(|(pos, _, _)| *pos);
    found.into_iter().map(|(_, kw, ts)| (kw, ts)).collect()
}

/// Rebuilds `line` after updating or clearing `kw`, preserving indentation and order.
pub(crate) fn rewrite_planning_line(line: &str, kw: &'static str, new_ts: Option<&str>) -> String {
    let indent = &line[..line.len() - line.trim_start().len()];
    let mut items = parse_planning_items(line);
    match new_ts {
        Some(val) => {
            if let Some(slot) = items.iter_mut().find(|(k, _)| *k == kw) {
                slot.1 = val.to_string();
            } else if kw == "SCHEDULED:" {
                let pos = items.iter().position(|x| x.0 == "DEADLINE:");
                items.insert(pos.unwrap_or(items.len()), (kw, val.to_string()));
            } else {
                items.push((kw, val.to_string()));
            }
        }
        None => items.retain(|(k, _)| *k != kw),
    }
    if items.is_empty() {
        return String::new();
    }
    let joined: Vec<String> = items.into_iter().map(|(k, v)| format!("{k} {v}")).collect();
    format!("{indent}{}", joined.join(" "))
}

/// Returns `true` when `raw` is one of the clear keywords (`clear`, `none`, `rm`, `remove`).
fn is_clear_spec(raw: &str) -> bool {
    let t = raw.trim();
    ["clear", "none", "rm", "remove"]
        .iter()
        .any(|k| t.eq_ignore_ascii_case(k))
}

/// Inserts, updates, shifts, or clears `SCHEDULED:` / `DEADLINE:` on `entry_idx`.
pub fn set_planning_timestamp(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    kind: PlanningKind,
    spec: Option<&str>,
    today: (u16, u8, u8),
) -> Result<PlanningMutation, String> {
    let Some(entry) = doc.entries.get(entry_idx) else {
        return Err(format!("entry index {entry_idx} out of bounds"));
    };
    let label = ["SCHEDULED:", "DEADLINE:"][usize::from(kind == PlanningKind::Deadline)];
    let parsed = spec
        .filter(|s| !is_clear_spec(s))
        .map(|r| parse_planning_spec(r.trim(), today));
    let new_timestamp = parsed.transpose()?.map(|t| t.to_string());
    let mut lines: Vec<String> = source.content.split('\n').map(str::to_string).collect();
    let plan_idx = entry.heading_line;
    let next_h = doc.entries.get(entry_idx + 1).map(|n| n.heading_line - 1);
    let end_excl = next_h.unwrap_or(lines.len()).min(lines.len());
    let has_plan = plan_idx < end_excl && is_planning_line_trimmed(lines[plan_idx].trim());
    let items = parse_planning_items(if has_plan { &lines[plan_idx] } else { "" });
    let old_timestamp = items.into_iter().find(|x| x.0 == label).map(|x| x.1);
    if has_plan {
        let updated = rewrite_planning_line(&lines[plan_idx], label, new_timestamp.as_deref());
        if updated.is_empty() {
            lines.remove(plan_idx);
        } else {
            lines[plan_idx] = updated;
        }
    } else if let Some(ref ts) = new_timestamp {
        lines.insert(plan_idx, format!("{label} {ts}"));
    }
    let content = lines.join("\n");
    let changed = content != source.content;
    Ok(PlanningMutation {
        content,
        changed,
        line: entry.heading_line,
        old_timestamp,
        new_timestamp,
    })
}

// endregion
