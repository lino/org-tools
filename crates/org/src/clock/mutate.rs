// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Active clock mutations (`org clock in`, `org clock out`, `org clock cancel`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use org_tools_core::document::{OrgDocument, OrgEntry};
use org_tools_core::files::{canonicalize_clean, write_file_atomic};
use org_tools_core::locator::{resolve_in_document, OrgLocator};
use org_tools_core::logbook::now_timestamp;
use org_tools_core::planning::{date_to_days, day_of_week_abbrev};
use org_tools_core::rules::heading::parse_heading;
use org_tools_core::rules::timestamp::{is_valid_date, parse_timestamp, OrgTimestamp};
use org_tools_core::source::SourceFile;

pub(crate) use crate::commands::clock::{locator_file_path, parse_clock_locator};
use crate::commands::MutationTelemetry;
use crate::date;

// region Public Types & Constants

/// Environment variable overriding the wall-clock timestamp when `--at` is omitted.
pub const NOW_ENV_VAR: &str = "ORG_TOOLS_NOW";

/// Per-entry mutation record emitted by `org clock in|out|cancel`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClockMutationDetail {
    /// Path to the Org file containing the mutated entry.
    pub file: String,
    /// One-based heading line number of the affected entry.
    pub line: usize,
    /// Heading title of the affected entry.
    pub title: String,
    /// Machine-readable action (`"clock-in"`, `"clock-out"`, `"clock-cancel"`, `"auto-clock-out"`).
    pub action: String,
    /// Human-readable description of the clock change.
    pub detail: Option<String>,
    /// Elapsed clock duration in minutes when a clock was closed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_minutes: Option<i64>,
    /// Formatted `H:MM` duration when a clock was closed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
}

/// Structured result envelope for `org clock in|out|cancel` and MCP clock tools.
#[derive(serde::Serialize)]
pub(crate) struct ClockMutationResult {
    /// Canonical command name (`"clock in"`, `"clock out"`, `"clock cancel"`).
    pub command: String,
    /// Subcommand alias matching `command` for schema compatibility.
    pub subcommand: String,
    /// Number of files modified or scheduled for modification.
    pub files_modified: usize,
    /// Total number of entries affected across all files.
    pub entries_affected: usize,
    /// Whether no file writes were performed (`--dry-run`).
    pub dry_run: bool,
    /// Elapsed duration in minutes for `clock out`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_minutes: Option<i64>,
    /// Formatted `H:MM` duration for `clock out`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Per-entry mutation details.
    pub details: Vec<ClockMutationDetail>,
    /// Operational error messages collected during execution.
    pub errors: Vec<String>,
    /// Execution duration and file-scan telemetry.
    pub telemetry: MutationTelemetry,
}

/// Coordinates `(doc_idx, entry_idx, clock_idx)` of an active open clock (`clock.end.is_none()`).
type ActiveClockSlot = (usize, usize, usize);

/// Loaded workspace document tuple `(file_path, source_file, parsed_document)`.
type WorkspaceDoc = (PathBuf, SourceFile, OrgDocument);

/// Constructs a [`ClockMutationDetail`] for `(path, entry)`.
fn make_detail(
    (path, entry): (&Path, &OrgEntry),
    action: &str,
    detail: String,
    dur: Option<i64>,
) -> ClockMutationDetail {
    ClockMutationDetail {
        file: path.display().to_string(),
        line: entry.heading_line,
        title: entry.title.clone(),
        action: action.to_string(),
        detail: Some(detail),
        duration_minutes: dur,
        duration: dur.map(date::format_duration),
    }
}

// endregion

// region Timestamp Parsing & Formatting

/// Parses an explicit `--at` timestamp (`YYYY-MM-DDTHH:MM`, `YYYY-MM-DD HH:MM`, or `[YYYY-MM-DD Day HH:MM]`).
pub fn parse_clock_timestamp(raw: &str) -> Result<OrgTimestamp, String> {
    let t = raw.trim();
    let b = if t.starts_with('[') || t.starts_with('<') {
        t.to_string()
    } else {
        format!("[{}]", t.replace('T', " "))
    };
    let (mut ts, end) =
        parse_timestamp(&b, 0).ok_or_else(|| format!("invalid clock timestamp '{raw}'"))?;
    let valid_hm = ts.hour.is_some_and(|h| h < 24) && ts.minute.is_some_and(|m| m < 60);
    if end != b.len() || !is_valid_date(ts.year, ts.month, ts.day) || !valid_hm {
        return Err(format!("invalid clock timestamp '{raw}'"));
    }
    ts.active = false;
    ts.dayname = Some(day_of_week_abbrev(ts.year, ts.month, ts.day).to_string());
    Ok(ts)
}

/// Resolves the effective clock timestamp from `--at`, `ORG_TOOLS_NOW`, or local wall-clock time.
pub fn resolve_clock_timestamp(at: Option<&str>) -> Result<OrgTimestamp, String> {
    let env_v = std::env::var(NOW_ENV_VAR)
        .ok()
        .filter(|s| !s.trim().is_empty());
    let raw = at
        .map(str::to_string)
        .or(env_v)
        .unwrap_or_else(|| format!("[{}]", now_timestamp()));
    parse_clock_timestamp(&raw)
}

/// Computes elapsed minutes between `start` and `end`, rejecting `end < start`.
pub fn compute_elapsed_minutes(start: &OrgTimestamp, end: &OrgTimestamp) -> Result<i64, String> {
    let to_m = |t: &OrgTimestamp| {
        date_to_days(t.year, t.month, t.day) * 1440
            + i64::from(t.hour.unwrap_or(0)) * 60
            + i64::from(t.minute.unwrap_or(0))
    };
    let diff = to_m(end) - to_m(start);
    if diff < 0 {
        return Err(format!("clock end {end} is earlier than start {start}"));
    }
    Ok(diff)
}

// endregion

// region Source Text Manipulation

/// Extracts the leading indentation slice of `line`.
fn line_indent(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// Finds the 0-based line index immediately after planning lines and `:PROPERTIES:` drawer.
fn find_drawer_insert_index(lines: &[String], head_idx: usize, end_idx: usize) -> usize {
    let mut cur = head_idx + 1;
    while cur < end_idx {
        let t = lines[cur].trim();
        if !(t.starts_with("SCHEDULED:") || t.starts_with("DEADLINE:") || t.starts_with("CLOSED:"))
        {
            break;
        }
        cur += 1;
    }
    if cur < end_idx && lines[cur].trim().eq_ignore_ascii_case(":PROPERTIES:") {
        if let Some(ep) =
            ((cur + 1)..end_idx).find(|&i| lines[i].trim().eq_ignore_ascii_case(":END:"))
        {
            return ep + 1;
        }
    }
    cur
}

/// Inserts an open `CLOCK: [START]` line into `entry`'s `:LOGBOOK:` drawer (creating one if absent).
fn insert_clock_in_source(content: &str, entry: &OrgEntry, start_ts: &OrgTimestamp) -> String {
    let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    let head_idx = entry.heading_line - 1;
    let end_idx = (entry.content_end_line - 1).min(lines.len());
    let head_indent = line_indent(&lines[head_idx]).to_string();
    for idx in (head_idx + 1)..end_idx {
        if parse_heading(&lines[idx]).is_some() {
            break;
        }
        if lines[idx].trim().eq_ignore_ascii_case(":LOGBOOK:") {
            let ind = line_indent(&lines[idx]).to_string();
            lines.insert(idx + 1, format!("{ind}CLOCK: {start_ts}"));
            return lines.join("\n");
        }
    }
    let cur = find_drawer_insert_index(&lines, head_idx, end_idx);
    let drawer = [
        format!("{head_indent}:LOGBOOK:"),
        format!("{head_indent}CLOCK: {start_ts}"),
        format!("{head_indent}:END:"),
    ];
    lines.splice(cur..cur, drawer);
    lines.join("\n")
}

// endregion

// region Locator & Workspace Discovery

/// Loads `(path, SourceFile, OrgDocument)` tuples and finds active open clock slots.
fn load_docs_and_slots(
    files: &[PathBuf],
) -> Result<(Vec<WorkspaceDoc>, Vec<ActiveClockSlot>), String> {
    let (mut docs, mut slots) = (Vec::with_capacity(files.len()), Vec::new());
    for (di, file) in files.iter().enumerate() {
        let src = SourceFile::from_path(file)
            .map_err(|e| format!("error reading {}: {e}", file.display()))?;
        let doc = OrgDocument::from_source(&src);
        for (ei, entry) in doc.entries.iter().enumerate() {
            for (ci, c) in entry.clocks.iter().enumerate() {
                if c.end.is_none() {
                    slots.push((di, ei, ci));
                }
            }
        }
        docs.push((file.clone(), src, doc));
    }
    Ok((docs, slots))
}

/// Resolves `locator` to `(doc_idx, entry_idx)` across `docs`.
fn resolve_target_in_docs(
    docs: &[WorkspaceDoc],
    locator: &OrgLocator,
    raw_label: &str,
) -> Result<(usize, usize), String> {
    let want = locator_file_path(locator)
        .map(|p| (p, canonicalize_clean(p).unwrap_or_else(|_| p.to_path_buf())));
    for (di, (path, _, doc)) in docs.iter().enumerate() {
        if let Some((raw_p, ref wc)) = want {
            let dc = canonicalize_clean(path).unwrap_or_else(|_| path.clone());
            if &dc != wc && !path.ends_with(raw_p) {
                continue;
            }
        }
        if let Ok(res) = resolve_in_document(locator, doc) {
            if res.is_file_node() {
                return Err(format!("cannot clock on file-level node '{raw_label}'"));
            }
            return Ok((di, res.entry_index));
        }
    }
    Err(format!("could not resolve locator '{raw_label}'"))
}

/// Commits modified documents in `dirty` to disk (unless `dry_run`) and builds [`ClockMutationResult`].
fn finalize_clock_mutation(
    name: &str,
    docs: &[WorkspaceDoc],
    dirty: &HashSet<usize>,
    details: Vec<ClockMutationDetail>,
    (dur_mins, dry_run, started): (Option<i64>, bool, Instant),
) -> Result<(ClockMutationResult, Vec<PathBuf>), String> {
    let mut indices: Vec<usize> = dirty.iter().copied().collect();
    indices.sort_unstable();
    let mut modified = Vec::with_capacity(indices.len());
    for idx in indices {
        let (path, src, _) = &docs[idx];
        if !dry_run {
            write_file_atomic(path, &src.content)
                .map_err(|e| format!("error writing {}: {e}", path.display()))?;
        }
        modified.push(canonicalize_clean(path).unwrap_or_else(|_| path.clone()));
    }
    let scanned = docs.len();
    let res = ClockMutationResult {
        command: name.to_string(),
        subcommand: name.to_string(),
        files_modified: modified.len(),
        entries_affected: details.len(),
        dry_run,
        duration_minutes: dur_mins,
        duration: dur_mins.map(date::format_duration),
        details,
        errors: Vec::new(),
        telemetry: MutationTelemetry {
            duration_us: started.elapsed().as_micros() as u64,
            files_scanned: scanned,
            cache_hits: 0,
            cache_misses: scanned,
        },
    };
    Ok((res, modified))
}

// endregion

// region Clock In, Out & Cancel Execution

/// Closes `(di, ei, ci)` in `docs` at `end_ts` and returns its [`ClockMutationDetail`] and elapsed minutes.
fn apply_slot_close(
    docs: &mut [WorkspaceDoc],
    (di, ei, ci): ActiveClockSlot,
    end_ts: &OrgTimestamp,
    action: &str,
) -> Result<(ClockMutationDetail, i64), String> {
    let (path, src, doc) = &mut docs[di];
    let (entry, clock) = (&doc.entries[ei], &doc.entries[ei].clocks[ci]);
    let mins = compute_elapsed_minutes(&clock.start, end_ts)?;
    let (h, m) = (mins / 60, mins % 60);
    let mut lines: Vec<String> = src.content.split('\n').map(str::to_string).collect();
    let lidx = clock.line - 1;
    let indent = line_indent(&lines[lidx]).to_string();
    lines[lidx] = format!("{indent}CLOCK: {}--{end_ts} => {h:>2}:{m:02}", clock.start);
    let msg = format!(
        "{}--{end_ts} => {}",
        clock.start,
        date::format_duration(mins)
    );
    let detail = make_detail((path, entry), action, msg, Some(mins));
    *src = SourceFile::new(path.clone(), lines.join("\n"));
    *doc = OrgDocument::from_source(src);
    Ok((detail, mins))
}

/// Executes `clock in` over an explicit list of `.org` `files`.
pub(crate) fn execute_clock_in_on_files(
    files: &[PathBuf],
    raw_locator: &str,
    at: Option<&str>,
    auto_out: bool,
    dry_run: bool,
) -> Result<(ClockMutationResult, Vec<PathBuf>), String> {
    let started = Instant::now();
    let locator = parse_clock_locator(raw_locator)?;
    let at_ts = resolve_clock_timestamp(at)?;
    let (mut docs, running) = load_docs_and_slots(files)?;
    let (td, te) = resolve_target_in_docs(&docs, &locator, raw_locator)?;
    if let Some(&(d, e, _)) = running
        .iter()
        .find(|&&(d, e, _)| (d == td && e == te) || !auto_out)
    {
        let t = &docs[d].2.entries[e].title;
        let hint = if d == td && e == te {
            ""
        } else {
            "; pass --auto-out"
        };
        return Err(format!("clock is already running on '{t}'{hint}"));
    }
    let (mut dirty, mut details) = (HashSet::new(), Vec::new());
    for slot in running {
        let (det, _) = apply_slot_close(&mut docs, slot, &at_ts, "auto-clock-out")?;
        dirty.insert(slot.0);
        details.push(det);
    }
    let (path, src, doc) = &mut docs[td];
    let entry = &doc.entries[te];
    let updated = insert_clock_in_source(&src.content, entry, &at_ts);
    details.push(make_detail(
        (path, entry),
        "clock-in",
        at_ts.to_string(),
        None,
    ));
    *src = SourceFile::new(path.clone(), updated);
    *doc = OrgDocument::from_source(src);
    dirty.insert(td);
    finalize_clock_mutation("clock in", &docs, &dirty, details, (None, dry_run, started))
}

/// Selects the target [`ActiveClockSlot`] for `clock out` or `clock cancel`.
fn select_active_slot(
    docs: &[WorkspaceDoc],
    running: &[ActiveClockSlot],
    locator: Option<&OrgLocator>,
    raw: Option<&str>,
) -> Result<ActiveClockSlot, String> {
    if let (Some(loc), Some(r)) = (locator, raw) {
        let (di, ei) = resolve_target_in_docs(docs, loc, r)?;
        let msg = format!("no running clock on '{}'", docs[di].2.entries[ei].title);
        return running
            .iter()
            .copied()
            .find(|&(d, e, _)| d == di && e == ei)
            .ok_or(msg);
    }
    match running {
        [] => Err("no running clock found".to_string()),
        [single] => Ok(*single),
        many => Err(format!(
            "multiple running clocks ({}); specify --locator",
            many.len()
        )),
    }
}

/// Terminates an active clock by closing it (`is_cancel == false`) or cancelling it (`is_cancel == true`).
pub(crate) fn terminate_active_clock(
    files: &[PathBuf],
    raw_locator: Option<&str>,
    at: Option<&str>,
    is_cancel: bool,
    dry_run: bool,
) -> Result<(ClockMutationResult, Vec<PathBuf>), String> {
    let started = Instant::now();
    let parsed_loc = raw_locator.map(parse_clock_locator).transpose()?;
    let end_ts = (!is_cancel)
        .then(|| resolve_clock_timestamp(at))
        .transpose()?;
    let (mut docs, running) = load_docs_and_slots(files)?;
    let (di, ei, ci) = select_active_slot(&docs, &running, parsed_loc.as_ref(), raw_locator)?;
    let (name, detail, mins) = if let Some(ref end) = end_ts {
        let (det, m) = apply_slot_close(&mut docs, (di, ei, ci), end, "clock-out")?;
        ("clock out", det, Some(m))
    } else {
        let (path, src, doc) = &mut docs[di];
        let (entry, clock) = (&doc.entries[ei], &doc.entries[ei].clocks[ci]);
        let mut ls: Vec<&str> = src.content.split('\n').collect();
        let idx = clock.line - 1;
        let sole = idx >= 1
            && idx + 1 < ls.len()
            && ls[idx - 1].trim().eq_ignore_ascii_case(":LOGBOOK:")
            && ls[idx + 1].trim().eq_ignore_ascii_case(":END:");
        ls.drain(if sole {
            (idx - 1)..=(idx + 1)
        } else {
            idx..=idx
        });
        let det = make_detail((path, entry), "clock-cancel", clock.start.to_string(), None);
        *src = SourceFile::new(path.clone(), ls.join("\n"));
        *doc = OrgDocument::from_source(src);
        ("clock cancel", det, None)
    };
    let dirty = HashSet::from([di]);
    finalize_clock_mutation(name, &docs, &dirty, vec![detail], (mins, dry_run, started))
}

// endregion
