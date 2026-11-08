// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! State-change logging in `:LOGBOOK:`, `:LAST_REPEAT:` updates, `:LOGGING:`, and `org-habit` summaries.
//!
//! Spec: [§5.3 Progress Logging](https://orgmode.org/manual/Progress-Logging.html),
//! [§5.3.2 Tracking TODO state changes](https://orgmode.org/manual/Tracking-TODO-state-changes.html),
//! [§5.3.3 Tracking your habits](https://orgmode.org/manual/Tracking-your-habits.html).

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::document::OrgDocument;
use crate::planning::is_planning_line_trimmed;
use crate::property::set_property;
use crate::rules::heading::parse_heading;
use crate::rules::timestamp::{
    date_to_days, day_of_week_abbrev, days_to_date, find_timestamps, parse_repeater, TimeUnit,
};
use crate::source::SourceFile;

// region Public Types & Constants

/// Environment variable overriding wall-clock time (`YYYY-MM-DDTHH:MM`) for deterministic tests.
pub const ENV_ORG_TOOLS_NOW: &str = "ORG_TOOLS_NOW";

/// Number of calendar days rendered in an `org-habit` consistency graph (`[from - 20 ..= from]`).
pub const HABIT_GRAPH_WINDOW_DAYS: i64 = 21;

/// Effective TODO state-transition and repeater logging configuration for an entry or document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggingConfig {
    /// True when `:LOGGING: nil` explicitly disables all state and repeater logging.
    pub disabled: bool,
    /// True when transitioning into a done keyword logs a `- State` entry (`logdone`).
    pub log_done: bool,
    /// True when repeater auto-advance logs `- State` and updates `:LAST_REPEAT:` (`logrepeat`).
    pub log_repeat: bool,
    /// Keywords that trigger a `- State` entry when entered (`!` or `@` before `/`).
    pub log_on_enter: HashSet<String>,
    /// Keywords that trigger a `- State` entry when left (`!` after `/`).
    pub log_on_leave: HashSet<String>,
}

/// Captured TODO state transition for a single mutated entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateTransitionRecord {
    /// Entry index in [`OrgDocument::entries`].
    pub entry_idx: usize,
    /// Previous TODO keyword before the transition, if any.
    pub old_state: Option<String>,
    /// Target TODO keyword requested for the transition.
    pub new_state: String,
    /// True when the entry auto-advanced a repeater instead of closing permanently.
    pub repeated: bool,
}

/// Repeater interval and 21-day `org-habit` consistency summary attached to an agenda item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HabitSummary {
    /// Minimum repeater interval in days.
    pub min_interval_days: u32,
    /// Optional maximum repeater window in days (e.g., `4` for `.+2d/4d`).
    pub max_interval_days: Option<u32>,
    /// Most recent `:LAST_REPEAT:` inactive timestamp string, if present.
    pub last_repeat: Option<String>,
    /// 21-character ASCII consistency graph ending on the agenda start date (`[from - 20 ..= from]`).
    pub consistency_graph: String,
}

// endregion

// region Logging & Transition Mutation

/// Parses a `#+TODO:` or `:LOGGING:` keyword token into `(keyword, log_on_enter, log_on_leave)`.
pub fn parse_todo_keyword_flags(raw_token: &str) -> (&str, bool, bool) {
    let (Some(open), Some(close)) = (raw_token.find('('), raw_token.rfind(')')) else {
        return (raw_token, false, false);
    };
    if close <= open {
        return (raw_token, false, false);
    }
    let name = &raw_token[..open];
    let inside = &raw_token[open + 1..close];
    let (enter_part, leave_part) = inside.split_once('/').unwrap_or((inside, ""));
    let enter = enter_part.contains('!') || enter_part.contains('@');
    let leave = leave_part.contains('!');
    (name, enter, leave)
}

/// Inserts or removes `kw` in `set` according to `enabled`.
fn toggle_keyword(set: &mut HashSet<String>, kw: &str, enabled: bool) {
    if enabled {
        set.insert(kw.to_string());
    } else {
        set.remove(kw);
    }
}

/// Applies whitespace-separated `#+STARTUP:` or `:LOGGING:` tokens onto `cfg`.
fn apply_logging_tokens(tokens_str: &str, done_kws: &[String], cfg: &mut LoggingConfig) {
    for token in tokens_str.split_whitespace() {
        match token.to_ascii_lowercase().as_str() {
            "nil" => {
                (cfg.disabled, cfg.log_done, cfg.log_repeat) = (true, false, false);
                cfg.log_on_enter.clear();
                cfg.log_on_leave.clear();
            }
            "logdone" | "lognotedone" => (cfg.disabled, cfg.log_done) = (false, true),
            "nologdone" => {
                cfg.log_done = false;
                for k in done_kws {
                    cfg.log_on_enter.remove(k);
                }
            }
            "logrepeat" | "lognoterepeat" => (cfg.disabled, cfg.log_repeat) = (false, true),
            "nologrepeat" => cfg.log_repeat = false,
            _ if token.contains('(') => {
                let (kw, enter, leave) = parse_todo_keyword_flags(token);
                if !kw.is_empty() {
                    cfg.disabled = false;
                    toggle_keyword(&mut cfg.log_on_enter, kw, enter);
                    toggle_keyword(&mut cfg.log_on_leave, kw, leave);
                }
            }
            _ => {}
        }
    }
}

/// Resolves effective [`LoggingConfig`] for `entry_idx` in `doc`.
pub fn resolve_entry_logging(doc: &OrgDocument, entry_idx: usize) -> LoggingConfig {
    let mut cfg = LoggingConfig {
        disabled: false,
        log_done: false,
        log_repeat: true,
        log_on_enter: HashSet::new(),
        log_on_leave: HashSet::new(),
    };
    if let Some(startup) = doc.file_keywords.get("STARTUP") {
        apply_logging_tokens(startup, &doc.todo_keywords.done, &mut cfg);
    }
    if !cfg.disabled {
        cfg.log_on_enter
            .extend(doc.todo_keywords.log_on_enter.iter().cloned());
        cfg.log_on_leave
            .extend(doc.todo_keywords.log_on_leave.iter().cloned());
    }
    if let Some(prop_logging) = doc.property(entry_idx, "LOGGING") {
        apply_logging_tokens(prop_logging, &doc.todo_keywords.done, &mut cfg);
    }
    cfg
}

/// Returns `true` when transitioning `entry_idx` from `old_state` to `new_state` should be logged.
pub fn should_log_state_transition(
    doc: &OrgDocument,
    entry_idx: usize,
    old_state: Option<&str>,
    new_state: &str,
    repeated: bool,
) -> bool {
    let cfg = resolve_entry_logging(doc, entry_idx);
    if cfg.disabled {
        return false;
    }
    if repeated {
        return cfg.log_repeat;
    }
    (cfg.log_done && doc.todo_keywords.is_done(new_state))
        || cfg.log_on_enter.contains(new_state)
        || old_state.is_some_and(|old| cfg.log_on_leave.contains(old))
}

/// Formats an Emacs-compatible `:LOGBOOK:` state transition line.
pub fn format_state_log_line(new_state: &str, old_state: Option<&str>, timestamp: &str) -> String {
    let new_quoted = format!("\"{new_state}\"");
    let old_quoted = format!("\"{}\"", old_state.unwrap_or(""));
    format!("- State {new_quoted:<12} from {old_quoted:<12} [{timestamp}]")
}

/// Finds the 0-based line index immediately after an entry's heading, planning line, and `:PROPERTIES:`.
fn find_post_metadata_line(lines: &[String], head_idx: usize, end_idx: usize) -> usize {
    let mut cursor = head_idx + 1;
    while cursor < end_idx && is_planning_line_trimmed(lines[cursor].trim()) {
        cursor += 1;
    }
    if cursor < end_idx && lines[cursor].trim().eq_ignore_ascii_case(":PROPERTIES:") {
        for (idx, line) in lines.iter().enumerate().take(end_idx).skip(cursor + 1) {
            if line.trim().eq_ignore_ascii_case(":END:") {
                return idx + 1;
            }
        }
    }
    cursor
}

/// Inserts `log_line` at the top of `entry_idx`'s `:LOGBOOK:` drawer (creating one if absent).
fn insert_logbook_entry_line(
    content: &str,
    doc: &OrgDocument,
    entry_idx: usize,
    log_line: &str,
) -> String {
    let Some(entry) = doc.entries.get(entry_idx) else {
        return content.to_string();
    };
    let had_trailing_nl = content.ends_with('\n');
    let mut lines: Vec<String> = content
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    let head_idx = entry.heading_line - 1;
    let end_idx = (entry.content_end_line - 1).min(lines.len());
    let meta_idx = find_post_metadata_line(&lines, head_idx, end_idx);

    for idx in meta_idx..end_idx {
        if lines[idx].trim().eq_ignore_ascii_case(":LOGBOOK:") {
            let raw = &lines[idx];
            let indent = &raw[..raw.len() - raw.trim_start().len()];
            lines.insert(idx + 1, format!("{indent}{log_line}"));
            return lines.join("\n");
        }
    }

    let block = [":LOGBOOK:", log_line, ":END:"].map(str::to_string);
    let append_empty = !had_trailing_nl && meta_idx == lines.len();
    lines.splice(meta_idx..meta_idx, block);
    if append_empty {
        lines.push(String::new());
    }
    lines.join("\n")
}

/// Applies `:LAST_REPEAT:` and `:LOGBOOK:` state-entry logging across `transitions`.
pub fn apply_transition_logging(
    path: &Path,
    mut content: String,
    transitions: &[StateTransitionRecord],
    timestamp: &str,
) -> String {
    let repeat_val = format!("[{timestamp}]");
    for tr in transitions {
        let src = SourceFile::new(path.to_path_buf(), content.clone());
        let doc = OrgDocument::from_source(&src);
        let old = tr.old_state.as_deref();
        if !should_log_state_transition(&doc, tr.entry_idx, old, &tr.new_state, tr.repeated) {
            continue;
        }
        if tr.repeated {
            if let Ok(res) = set_property(&src, &doc, tr.entry_idx, "LAST_REPEAT", &repeat_val) {
                content = res.content;
            }
        }
        let src2 = SourceFile::new(path.to_path_buf(), content.clone());
        let doc2 = OrgDocument::from_source(&src2);
        let log_line = format_state_log_line(&tr.new_state, old, timestamp);
        content = insert_logbook_entry_line(&content, &doc2, tr.entry_idx, &log_line);
    }
    content
}

// endregion

// region Habit Summary & Time Helpers

/// Converts a repeater `(value, unit)` into an approximate day count for [`HabitSummary`].
fn repeater_unit_to_days(value: u32, unit: TimeUnit) -> u32 {
    match unit {
        TimeUnit::Hour => (value / 24).max(1),
        TimeUnit::Day => value,
        TimeUnit::Week => value * 7,
        TimeUnit::Month => value * 30,
        TimeUnit::Year => value * 365,
    }
}

/// Extracts completion day numbers from a single `:LOGBOOK:` state-change line if the target state is done.
fn record_logbook_done_days(trimmed: &str, doc: &OrgDocument, done_days: &mut HashSet<i64>) {
    if let Some((kw, _)) = trimmed
        .strip_prefix("- State \"")
        .and_then(|r| r.split_once('"'))
    {
        if doc.todo_keywords.is_done(kw) {
            for (ts, _, _) in find_timestamps(trimmed) {
                done_days.insert(date_to_days(ts.year, ts.month, ts.day));
            }
        }
    }
}

/// Collects completion civil day numbers from `:LOGBOOK:` done-state lines and `:LAST_REPEAT:`.
fn collect_habit_completion_days(
    source: Option<&SourceFile>,
    doc: &OrgDocument,
    entry_idx: usize,
) -> HashSet<i64> {
    let mut done_days = HashSet::new();
    if let Some(lr) = doc.property(entry_idx, "LAST_REPEAT") {
        for (ts, _, _) in find_timestamps(lr) {
            done_days.insert(date_to_days(ts.year, ts.month, ts.day));
        }
    }
    let (Some(src), Some(entry)) = (source, doc.entries.get(entry_idx)) else {
        return done_days;
    };
    let lines: Vec<&str> = src.content.split('\n').collect();
    let end_idx = (entry.content_end_line - 1).min(lines.len());
    let mut in_logbook = false;
    for &raw in lines.iter().take(end_idx).skip(entry.heading_line) {
        let trimmed = raw.trim();
        if parse_heading(trimmed).is_some() {
            break;
        }
        if trimmed.eq_ignore_ascii_case(":LOGBOOK:") {
            in_logbook = true;
        } else if in_logbook && trimmed.eq_ignore_ascii_case(":END:") {
            in_logbook = false;
        } else if in_logbook {
            record_logbook_done_days(trimmed, doc, &mut done_days);
        }
    }
    done_days
}

/// Builds a [`HabitSummary`] for `entry_idx` when `:STYLE:` is `"habit"` (case-insensitive).
pub fn build_habit_summary(
    source: Option<&SourceFile>,
    doc: &OrgDocument,
    entry_idx: usize,
    from_date: (u16, u8, u8),
) -> Option<HabitSummary> {
    let style = doc.property(entry_idx, "STYLE")?;
    if !style.trim().eq_ignore_ascii_case("habit") {
        return None;
    }
    let plan = &doc.entries.get(entry_idx)?.planning;
    let s_rep = plan.scheduled.as_ref().and_then(|t| t.repeater.as_deref());
    let d_rep = plan.deadline.as_ref().and_then(|t| t.repeater.as_deref());
    let spec = s_rep.or(d_rep).and_then(parse_repeater);
    let min_interval_days = spec.map_or(1, |s| repeater_unit_to_days(s.value, s.unit));
    let max_pair = spec.and_then(|s| s.max_value.zip(s.max_unit));
    let max_interval_days = max_pair.map(|(v, u)| repeater_unit_to_days(v, u));
    let last_repeat = doc
        .property(entry_idx, "LAST_REPEAT")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let done_days = collect_habit_completion_days(source, doc, entry_idx);
    let from_days = date_to_days(from_date.0, from_date.1, from_date.2);
    let start_days = from_days - (HABIT_GRAPH_WINDOW_DAYS - 1);
    let consistency_graph: String = (0..HABIT_GRAPH_WINDOW_DAYS)
        .map(|off| match start_days + off {
            d if done_days.contains(&d) => '*',
            d if d == from_days => '!',
            _ => '.',
        })
        .collect();
    Some(HabitSummary {
        min_interval_days,
        max_interval_days,
        last_repeat,
        consistency_graph,
    })
}

/// Parses [`ENV_ORG_TOOLS_NOW`] (`YYYY-MM-DDTHH:MM` or `YYYY-MM-DD HH:MM`) when set.
pub fn parse_env_now() -> Option<(u16, u8, u8, u32, u32)> {
    let raw = std::env::var(ENV_ORG_TOOLS_NOW).ok()?;
    let s = raw.trim();
    let (d_str, t_str) = s.split_once('T').or_else(|| s.split_once(' '))?;
    let [ys, ms, ds] = d_str.split('-').collect::<Vec<_>>().try_into().ok()?;
    let (hs, mins) = t_str.split_once(':')?;
    let m_str = mins.split(':').next()?;
    let (y, m, d) = (ys.parse().ok()?, ms.parse().ok()?, ds.parse().ok()?);
    Some((y, m, d, hs.parse().ok()?, m_str.parse().ok()?))
}

/// Extracts local `(year, month, day, hour, minute)` from Unix epoch `secs` via `libc`.
#[allow(deprecated)]
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

/// Resolves current `(year, month, day, hour, minute)` from [`ENV_ORG_TOOLS_NOW`] or OS clock.
fn current_local_parts() -> (u16, u8, u8, u32, u32) {
    if let Some(parts) = parse_env_now() {
        return parts;
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    if let Some(parts) = local_timestamp_parts(secs) {
        return parts;
    }
    let (y, m, d) = days_to_date(secs / 86_400);
    let rem = (secs % 86_400) as u32;
    (y, m, d, rem / 3_600, (rem % 3_600) / 60)
}

/// Returns today's local civil `(year, month, day)` date, honoring [`ENV_ORG_TOOLS_NOW`].
pub fn current_local_date() -> (u16, u8, u8) {
    let (y, m, d, _, _) = current_local_parts();
    (y, m, d)
}

/// Returns a formatted Org inactive timestamp payload (`YYYY-MM-DD Day HH:MM`), honoring [`ENV_ORG_TOOLS_NOW`].
pub fn now_timestamp() -> String {
    let (y, m, d, h, min) = current_local_parts();
    format!(
        "{y:04}-{m:02}-{d:02} {} {h:02}:{min:02}",
        day_of_week_abbrev(y, m, d)
    )
}

// endregion
