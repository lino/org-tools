// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared timestamp parser, calendar date conversions, and repeater/warning primitives.
//!
//! Spec: [§8.1 Timestamps](https://orgmode.org/manual/Timestamps.html),
//! [§8.3 Deadlines and Scheduling](https://orgmode.org/manual/Deadlines-and-Scheduling.html),
//! [§8.3.2 Repeated tasks](https://orgmode.org/manual/Repeated-tasks.html),
//! [Syntax: Timestamps](https://orgmode.org/worg/org-syntax.html#Timestamps).

// region Public Types

/// Parsed representation of an org-mode timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgTimestamp {
    /// Four-digit year.
    pub year: u16,
    /// Month (1–12).
    pub month: u8,
    /// Day of month (1–31).
    pub day: u8,
    /// Optional day-of-week abbreviation (e.g., `"Mon"`).
    pub dayname: Option<String>,
    /// Optional hour (0–23).
    pub hour: Option<u8>,
    /// Optional minute (0–59).
    pub minute: Option<u8>,
    /// Optional repeater string (e.g., `"+1w"`, `"++2m"`, `".+3d"`).
    pub repeater: Option<String>,
    /// Optional warning delay string (e.g., `"-3d"`, `"--2w"`).
    pub warning: Option<String>,
    /// `true` for active timestamps (`<…>`), `false` for inactive (`[…]`).
    pub active: bool,
}

/// Repeater shift mode per Org Manual §8.3.2 (`+`, `++`, `.+`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeaterKind {
    /// `+N_`: cumulative shift by one interval from the base timestamp date.
    Cumulative,
    /// `++N_`: catch-up shift in `N_` increments from the base date until strictly after `today`.
    CatchUp,
    /// `.+N_`: restart shift by one interval anchored at `today`.
    Restart,
}

/// Deadline lead-time warning mode per Org Manual §8.3.2 (`-`, `--`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// `-N_`: warn before every occurrence of the timestamp.
    All,
    /// `--N_`: warn only before the first (unrepeated base) occurrence.
    First,
}

/// Calendar or clock step unit shared by repeaters and warning delays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    /// Hourly unit (`h`).
    Hour,
    /// Daily unit (`d`).
    Day,
    /// Weekly unit (`w`, 7 days).
    Week,
    /// Monthly unit (`m`, with month-end clamping).
    Month,
    /// Yearly unit (`y`, with leap-day clamping).
    Year,
}

/// Parsed repeater specification (`+1w`, `++2m`, `.+3d`, `.+2d/4d`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeaterSpec {
    /// Shift strategy when advancing the timestamp.
    pub kind: RepeaterKind,
    /// Step magnitude in `unit`.
    pub value: u32,
    /// Calendar or clock unit.
    pub unit: TimeUnit,
    /// Optional `org-habit` maximum window magnitude (e.g., `4` in `.+2d/4d`).
    pub max_value: Option<u32>,
    /// Optional `org-habit` maximum window unit (e.g., `TimeUnit::Day` in `.+2d/4d`).
    pub max_unit: Option<TimeUnit>,
}

/// Parsed deadline warning specification (`-3d`, `--2w`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarningSpec {
    /// Whether warning applies to all occurrences or only the first.
    pub kind: WarningKind,
    /// Lead-time magnitude in `unit`.
    pub value: u32,
    /// Calendar or clock unit.
    pub unit: TimeUnit,
}

impl OrgTimestamp {
    /// Constructs an active date-only [`OrgTimestamp`] with its canonical 3-letter weekday.
    pub(crate) fn active_date(year: u16, month: u8, day: u8) -> Self {
        let dayname = Some(day_of_week_abbrev(year, month, day).to_string());
        let (hour, minute, repeater, warning, active) = (None, None, None, None, true);
        Self {
            year,
            month,
            day,
            dayname,
            hour,
            minute,
            repeater,
            warning,
            active,
        }
    }
}

impl std::fmt::Display for OrgTimestamp {
    /// Formats the timestamp using active (`<...>`) or inactive (`[...]`) Org syntax.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (open, close) = if self.active { ('<', '>') } else { ('[', ']') };
        let (y, m, d) = (self.year, self.month, self.day);
        let dn = self
            .dayname
            .as_ref()
            .map_or(String::new(), |n| format!(" {n}"));
        let hm = match (self.hour, self.minute) {
            (Some(h), Some(min)) => format!(" {h:02}:{min:02}"),
            _ => String::new(),
        };
        let rep = self
            .repeater
            .as_ref()
            .map_or(String::new(), |r| format!(" {r}"));
        let warn = self
            .warning
            .as_ref()
            .map_or(String::new(), |w| format!(" {w}"));
        write!(f, "{open}{y:04}-{m:02}-{d:02}{dn}{hm}{rep}{warn}{close}")
    }
}

// endregion

// region Timestamp Parsing

/// Applies a single inner whitespace-delimited timestamp token onto `ts`.
fn apply_timestamp_part(ts: &mut OrgTimestamp, part: &str) {
    let first = part.as_bytes().first().copied();
    let dash_tail = part.trim_start_matches('-').bytes().next();
    if part.contains(':') && first.is_some_and(|b| b.is_ascii_digit()) {
        if let Some((hs, ms)) = part.split('-').next().unwrap_or(part).split_once(':') {
            if !ms.contains(':') {
                (ts.hour, ts.minute) = (hs.parse().ok(), ms.parse().ok());
            }
        }
    } else if part.starts_with('+') || part.starts_with(".+") {
        ts.repeater = Some(part.to_string());
    } else if part.starts_with('-') && dash_tail.is_some_and(|b| b.is_ascii_digit()) {
        ts.warning = Some(part.to_string());
    } else if first.is_some_and(|b| b.is_ascii_alphabetic()) {
        ts.dayname = Some(part.to_string());
    }
}

/// Parses a `YYYY-MM-DD` date token without requiring calendrical validity.
fn parse_ymd_triplet(raw: &str) -> Option<(u16, u8, u8)> {
    let [ys, ms, ds] = raw.split('-').collect::<Vec<_>>().try_into().ok()?;
    Some((ys.parse().ok()?, ms.parse().ok()?, ds.parse().ok()?))
}

/// Parses an active or inactive Org timestamp at `pos`, returning `(timestamp, end_pos)`.
pub fn parse_timestamp(text: &str, pos: usize) -> Option<(OrgTimestamp, usize)> {
    let rest = &text[pos..];
    let (active, close) = match rest.as_bytes().first()? {
        b'<' => (true, '>'),
        b'[' => (false, ']'),
        _ => return None,
    };
    let end = rest.find(close)?;
    let parts: Vec<&str> = rest[1..end].split_whitespace().collect();
    let (year, month, day) = parse_ymd_triplet(parts.first()?)?;
    let mut ts = OrgTimestamp {
        active,
        ..OrgTimestamp::active_date(year, month, day)
    };
    ts.dayname = None;
    for &part in &parts[1..] {
        apply_timestamp_part(&mut ts, part);
    }
    Some((ts, pos + end + 1))
}

/// Finds all timestamps in a line and returns their parsed forms with byte offsets.
pub fn find_timestamps(line: &str) -> Vec<(OrgTimestamp, usize, usize)> {
    let (mut results, mut pos) = (Vec::new(), 0);
    while pos < line.len() {
        if matches!(line.as_bytes()[pos], b'<' | b'[') {
            if let Some((ts, end_pos)) = parse_timestamp(line, pos) {
                results.push((ts, pos, end_pos));
                pos = end_pos;
                continue;
            }
        }
        pos += 1;
    }
    results
}

// endregion

// region Repeater, Warning & Calendar Primitives

/// Parses `<digits><h|d|w|m|y>` into `(value, TimeUnit)`.
pub(crate) fn parse_value_and_unit(rest: &str) -> Option<(u32, TimeUnit)> {
    if rest.len() < 2 {
        return None;
    }
    let (num_str, unit_str) = rest.split_at(rest.len() - 1);
    if !num_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let unit = match unit_str.as_bytes()[0] {
        b'h' => TimeUnit::Hour,
        b'd' => TimeUnit::Day,
        b'w' => TimeUnit::Week,
        b'm' => TimeUnit::Month,
        b'y' => TimeUnit::Year,
        _ => return None,
    };
    Some((num_str.parse::<u32>().ok()?, unit))
}

/// Parses a repeater token (`+Ny`, `+Nm`, `+Nw`, `+Nd`, `+Nh`, `++N_`, `.+N_`, `.+Nd/Md`).
pub fn parse_repeater(raw: &str) -> Option<RepeaterSpec> {
    let (kind, rest) = if let Some(r) = raw.strip_prefix(".+") {
        (RepeaterKind::Restart, r)
    } else if let Some(r) = raw.strip_prefix("++") {
        (RepeaterKind::CatchUp, r)
    } else {
        (RepeaterKind::Cumulative, raw.strip_prefix('+')?)
    };
    let (min_raw, max_pair) = match rest.split_once('/') {
        Some((a, b)) => (a, Some(parse_value_and_unit(b).filter(|&(v, _)| v > 0)?)),
        None => (rest, None),
    };
    let (value, unit) = parse_value_and_unit(min_raw)?;
    let (max_value, max_unit) = (max_pair.map(|p| p.0), max_pair.map(|p| p.1));
    Some(RepeaterSpec {
        kind,
        value,
        unit,
        max_value,
        max_unit,
    })
}

/// Parses a deadline warning token (`-Ny`, `-Nm`, `-Nw`, `-Nd`, `-Nh`, `--N_`).
pub fn parse_warning(raw: &str) -> Option<WarningSpec> {
    let (kind, rest) = if let Some(r) = raw.strip_prefix("--") {
        (WarningKind::First, r)
    } else {
        (WarningKind::All, raw.strip_prefix('-')?)
    };
    let (value, unit) = parse_value_and_unit(rest)?;
    Some(WarningSpec { kind, value, unit })
}

/// Returns true if the repeater string is valid (e.g., `+1w`, `++2m`, `.+3d`).
pub fn is_valid_repeater(s: &str) -> bool {
    parse_repeater(s).is_some()
}

/// Returns true if the warning delay string is valid (e.g., `-3d`, `--2w`).
pub fn is_valid_warning(s: &str) -> bool {
    parse_warning(s).is_some()
}

/// Returns the number of days in `month` (`1..=12`) for `year`.
#[allow(unknown_lints, clippy::manual_is_multiple_of)]
pub(crate) fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Returns true if the date is calendrically valid.
pub fn is_valid_date(year: u16, month: u8, day: u8) -> bool {
    let max_day = days_in_month(year, month);
    max_day > 0 && day >= 1 && day <= max_day
}

/// Converts a civil `(year, month, day)` date into days since `1970-01-01`.
pub fn date_to_days(year: u16, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Converts days since `1970-01-01` back into a civil `(year, month, day)` triple.
pub fn days_to_date(z: i64) -> (u16, u8, u8) {
    let z = z.saturating_add(719_468);
    let era = (if z >= 0 { z } else { z.saturating_sub(146_096) }) / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (if month <= 2 { y + 1 } else { y }).clamp(0, i64::from(u16::MAX)) as u16;
    (year, month, d)
}

/// Returns the canonical 3-letter English weekday abbreviation (`"Mon"`..`"Sun"`).
pub fn day_of_week_abbrev(year: u16, month: u8, day: u8) -> &'static str {
    const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    DAY_NAMES[(date_to_days(year, month, day) + 3).rem_euclid(7) as usize]
}

/// Shifts `(year, month, day)` by `delta` units of `unit`, clamping month-ends and leap days.
pub fn shift_calendar_date(
    year: u16,
    month: u8,
    day: u8,
    delta: i32,
    unit: TimeUnit,
) -> (u16, u8, u8) {
    match unit {
        TimeUnit::Hour => days_to_date(date_to_days(year, month, day) + i64::from(delta) / 24),
        TimeUnit::Day => days_to_date(date_to_days(year, month, day) + i64::from(delta)),
        TimeUnit::Week => days_to_date(date_to_days(year, month, day) + i64::from(delta) * 7),
        TimeUnit::Month => {
            let total =
                i64::from(year) * 12 + i64::from(month.saturating_sub(1)) + i64::from(delta);
            let ny = total.div_euclid(12).clamp(0, i64::from(u16::MAX)) as u16;
            let nm = (total.rem_euclid(12) + 1) as u8;
            (ny, nm, day.min(days_in_month(ny, nm)))
        }
        TimeUnit::Year => {
            let ny = (i64::from(year) + i64::from(delta)).clamp(0, i64::from(u16::MAX)) as u16;
            (ny, month, day.min(days_in_month(ny, month)))
        }
    }
}

/// Computes `(lead_days, first_occurrence_only)` for `ts` using `default_days` as fallback.
pub fn effective_deadline_warning(ts: &OrgTimestamp, default_days: u32) -> (i64, bool) {
    let Some(spec) = ts.warning.as_deref().and_then(parse_warning) else {
        return (i64::from(default_days), false);
    };
    let days = match spec.unit {
        TimeUnit::Hour => i64::from(spec.value / 24),
        TimeUnit::Day => i64::from(spec.value),
        TimeUnit::Week => i64::from(spec.value) * 7,
        TimeUnit::Month | TimeUnit::Year => {
            let delta = -(spec.value.min(i32::MAX as u32) as i32);
            let (wy, wm, wd) = shift_calendar_date(ts.year, ts.month, ts.day, delta, spec.unit);
            date_to_days(ts.year, ts.month, ts.day) - date_to_days(wy, wm, wd)
        }
    };
    let first_only = spec.kind == WarningKind::First && ts.repeater.is_some();
    (days, first_only)
}

/// Computes the calendar-day warning lead distance before `ts` (falling back to `default_days`).
pub fn effective_deadline_warning_days(ts: &OrgTimestamp, default_days: u32) -> i64 {
    effective_deadline_warning(ts, default_days).0
}

/// Returns `true` when `ts` has both a repeater and a `--N_` ([`WarningKind::First`]) warning.
pub fn is_first_only_warning(ts: &OrgTimestamp) -> bool {
    effective_deadline_warning(ts, 0).1
}

/// Checks whether byte index `i` is a valid Org opening emphasis/verbatim boundary.
fn valid_verbatim_open(bytes: &[u8], i: usize) -> bool {
    let pre = i == 0 || bytes[i - 1].is_ascii_whitespace() || b"-({'\"".contains(&bytes[i - 1]);
    pre && !bytes[i + 1].is_ascii_whitespace()
}

/// Checks whether byte index `j` is a valid Org closing emphasis/verbatim boundary for `marker`.
fn valid_verbatim_close(bytes: &[u8], j: usize, marker: u8) -> bool {
    let post = j + 1 == bytes.len() || bytes[j + 1].is_ascii_whitespace();
    let punct = j + 1 < bytes.len() && b"-.,;:!?')}]\"".contains(&bytes[j + 1]);
    bytes[j] == marker && !bytes[j - 1].is_ascii_whitespace() && (post || punct)
}

/// Finds inline Org `=...=` verbatim and `~...~` code byte spans `[start, end)` on `line`.
pub(crate) fn inline_verbatim_spans(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let (mut spans, mut i) = (Vec::new(), 0);
    while i + 2 <= bytes.len() {
        let m = bytes[i];
        if matches!(m, b'=' | b'~') && valid_verbatim_open(bytes, i) {
            let close = (i + 2..bytes.len()).find(|&j| valid_verbatim_close(bytes, j, m));
            if let Some(end_idx) = close {
                spans.push((i, end_idx + 1));
                i = end_idx + 1;
                continue;
            }
        }
        i += 1;
    }
    spans
}

// endregion

// region Tests

/// Unit tests for timestamp parsing, date validation, and repeater/warning validation.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies parsing active/inactive dates, times, repeaters, and `--` warning delays.
    #[test]
    fn parse_active_inactive_repeater_and_warning() {
        let (ts, end) = parse_timestamp("<2024-01-15 Mon +1w -3d>", 0).unwrap();
        assert!(ts.active && end == 24 && (ts.year, ts.month, ts.day) == (2024, 1, 15));
        assert_eq!(ts.repeater.as_deref(), Some("+1w"));
        assert_eq!(ts.warning.as_deref(), Some("-3d"));
        let (ina, _) = parse_timestamp("[2024-01-15 Mon 09:30]", 0).unwrap();
        assert!(!ina.active && (ina.hour, ina.minute) == (Some(9), Some(30)));
        let (first, _) = parse_timestamp("<2024-01-15 Mon ++1m --2w>", 0).unwrap();
        assert_eq!(first.warning.as_deref(), Some("--2w"));
    }

    /// Verifies leap-year date validation, repeater/warning validation, and multi-timestamp scan.
    #[test]
    fn valid_dates_repeaters_and_find() {
        assert!(is_valid_date(2024, 2, 29) && !is_valid_date(2023, 2, 29));
        assert!(is_valid_repeater("+1w") && is_valid_repeater("++2m") && is_valid_repeater(".+3d"));
        assert!(!is_valid_repeater("+1x") && !is_valid_repeater("+++1d"));
        let sample = "SCHEDULED: <2024-01-15 Mon> DEADLINE: <2024-02-01 Thu>";
        assert_eq!(find_timestamps(sample).len(), 2);
        assert!(find_timestamps("just text").is_empty());
    }
}

// endregion
