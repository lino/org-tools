// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared timestamp parser for org-mode timestamps.
//!
//! Spec: [§8.1 Timestamps](https://orgmode.org/manual/Timestamps.html),
//! [Syntax: Timestamps](https://orgmode.org/worg/org-syntax.html#Timestamps)
//!
//! Active: `<YYYY-MM-DD DAY>` or `<YYYY-MM-DD DAY HH:MM>`
//! Inactive: `[YYYY-MM-DD DAY]` or `[YYYY-MM-DD DAY HH:MM]`
//! Repeaters: `+Ny`, `+Nm`, `+Nw`, `+Nd`, `+Nh`, `++N_`, `.+N_`
//! Warning: `-Nd` etc.

/// Parsed representation of an org-mode timestamp.
#[derive(Debug, PartialEq)]
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

/// Attempts to parse an org timestamp starting at position `pos` in `text`.
/// Returns the parsed timestamp and the byte position after it.
pub fn parse_timestamp(text: &str, pos: usize) -> Option<(OrgTimestamp, usize)> {
    let rest = &text[pos..];
    let (active, open, close) = if rest.starts_with('<') {
        (true, '<', '>')
    } else if rest.starts_with('[') {
        (false, '[', ']')
    } else {
        return None;
    };

    let end = rest.find(close)?;
    let inner = &rest[1..end];

    // Parse: YYYY-MM-DD [DAY] [HH:MM] [repeater] [warning]
    let parts: Vec<&str> = inner.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    // Parse date: YYYY-MM-DD
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }
    let year: u16 = date_parts[0].parse().ok()?;
    let month: u8 = date_parts[1].parse().ok()?;
    let day: u8 = date_parts[2].parse().ok()?;

    let mut dayname = None;
    let mut hour = None;
    let mut minute = None;
    let mut repeater = None;
    let mut warning = None;

    for &part in &parts[1..] {
        if part.contains(':') && part.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            // Time: HH:MM or HH:MM-HH:MM
            let time_str = part.split('-').next().unwrap_or(part);
            let time_parts: Vec<&str> = time_str.split(':').collect();
            if time_parts.len() == 2 {
                hour = time_parts[0].parse().ok();
                minute = time_parts[1].parse().ok();
            }
        } else if part.starts_with('+') || part.starts_with(".+") {
            repeater = Some(part.to_string());
        } else if part.starts_with('-') && part.len() > 1 && part[1..].chars().next().is_some_and(|c| c.is_ascii_digit()) {
            warning = Some(part.to_string());
        } else if part.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            dayname = Some(part.to_string());
        }
    }

    let _ = (open, close); // Suppress unused warnings.

    Some((
        OrgTimestamp {
            year,
            month,
            day,
            dayname,
            hour,
            minute,
            repeater,
            warning,
            active,
        },
        pos + end + 1,
    ))
}

/// Returns true if the date is calendrically valid.
pub fn is_valid_date(year: u16, month: u8, day: u8) -> bool {
    if month == 0 || month > 12 || day == 0 {
        return false;
    }
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    day <= days_in_month
}

/// Returns true if the year is a leap year.
fn is_leap_year(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

/// Returns true if the repeater string is valid (e.g., `+1w`, `++2m`, `.+3d`).
pub fn is_valid_repeater(s: &str) -> bool {
    let rest = if let Some(r) = s.strip_prefix(".+") {
        r
    } else if let Some(r) = s.strip_prefix("++") {
        r
    } else if let Some(r) = s.strip_prefix('+') {
        r
    } else {
        return false;
    };

    if rest.is_empty() {
        return false;
    }

    let unit = rest.as_bytes()[rest.len() - 1];
    let number = &rest[..rest.len() - 1];

    matches!(unit, b'y' | b'm' | b'w' | b'd' | b'h') && number.parse::<u32>().is_ok()
}

/// Returns true if the warning delay string is valid (e.g., `-3d`, `--2w`).
pub fn is_valid_warning(s: &str) -> bool {
    let rest = if let Some(r) = s.strip_prefix("--") {
        r
    } else if let Some(r) = s.strip_prefix('-') {
        r
    } else {
        return false;
    };

    if rest.is_empty() {
        return false;
    }

    let unit = rest.as_bytes()[rest.len() - 1];
    let number = &rest[..rest.len() - 1];

    matches!(unit, b'y' | b'm' | b'w' | b'd' | b'h') && number.parse::<u32>().is_ok()
}

/// Finds all timestamps in a line and returns their parsed forms with byte offsets.
pub fn find_timestamps(line: &str) -> Vec<(OrgTimestamp, usize, usize)> {
    let mut results = Vec::new();
    let mut pos = 0;
    while pos < line.len() {
        let ch = line.as_bytes()[pos];
        if ch == b'<' || ch == b'[' {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_active_date() {
        let (ts, end) = parse_timestamp("<2024-01-15 Mon>", 0).unwrap();
        assert!(ts.active);
        assert_eq!(ts.year, 2024);
        assert_eq!(ts.month, 1);
        assert_eq!(ts.day, 15);
        assert_eq!(ts.dayname.as_deref(), Some("Mon"));
        assert_eq!(end, 16);
    }

    #[test]
    fn parse_inactive_with_time() {
        let (ts, _) = parse_timestamp("[2024-01-15 Mon 09:30]", 0).unwrap();
        assert!(!ts.active);
        assert_eq!(ts.hour, Some(9));
        assert_eq!(ts.minute, Some(30));
    }

    #[test]
    fn parse_with_repeater() {
        let (ts, _) = parse_timestamp("<2024-01-15 Mon +1w>", 0).unwrap();
        assert_eq!(ts.repeater.as_deref(), Some("+1w"));
    }

    #[test]
    fn parse_with_warning() {
        let (ts, _) = parse_timestamp("<2024-01-15 Mon +1w -3d>", 0).unwrap();
        assert_eq!(ts.repeater.as_deref(), Some("+1w"));
        assert_eq!(ts.warning.as_deref(), Some("-3d"));
    }

    #[test]
    fn valid_dates() {
        assert!(is_valid_date(2024, 2, 29)); // Leap year.
        assert!(is_valid_date(2024, 12, 31));
        assert!(!is_valid_date(2023, 2, 29)); // Not a leap year.
        assert!(!is_valid_date(2024, 13, 1));
        assert!(!is_valid_date(2024, 0, 1));
        assert!(!is_valid_date(2024, 2, 30));
    }

    #[test]
    fn valid_repeaters() {
        assert!(is_valid_repeater("+1w"));
        assert!(is_valid_repeater("++2m"));
        assert!(is_valid_repeater(".+3d"));
        assert!(!is_valid_repeater("+1x"));
        assert!(!is_valid_repeater("1w"));
        assert!(!is_valid_repeater("+"));
    }

    #[test]
    fn find_multiple_timestamps() {
        let line = "SCHEDULED: <2024-01-15 Mon> DEADLINE: <2024-02-01 Thu>";
        let ts = find_timestamps(line);
        assert_eq!(ts.len(), 2);
        assert_eq!(ts[0].0.day, 15);
        assert_eq!(ts[1].0.day, 1);
    }

    #[test]
    fn no_timestamps() {
        assert!(find_timestamps("just text").is_empty());
    }
}
