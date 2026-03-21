// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared date utilities for org-tools CLI commands.

/// Get today's date as (year, month, day).
pub fn current_date() -> (u16, u8, u8) {
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs() as i64;
    let days = secs / 86400;
    days_to_date(days)
}

/// Convert (year, month, day) to a day count using Howard Hinnant's algorithm.
pub fn date_to_days(year: u16, month: u8, day: u8) -> i64 {
    let y = if month <= 2 {
        year as i64 - 1
    } else {
        year as i64
    };
    let m = if month <= 2 {
        month as i64 + 9
    } else {
        month as i64 - 3
    };
    let era = y / 400;
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe
}

/// Convert a day count back to (year, month, day) using Howard Hinnant's algorithm.
pub fn days_to_date(z: i64) -> (u16, u8, u8) {
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u16, m as u8, d)
}

/// Day of week (0=Mon, 6=Sun) using Tomohiko Sakamoto's algorithm.
#[allow(dead_code)]
pub fn day_of_week(year: u16, month: u8, day: u8) -> u8 {
    let y = year as i64;
    let m = month as i64;
    let d = day as i64;
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let dow = (y + y / 4 - y / 100 + y / 400 + t[(m - 1) as usize] + d) % 7;
    ((dow + 6) % 7) as u8
}

/// Parse a date string in YYYY-MM-DD format.
pub fn parse_date(s: &str) -> Option<(u16, u8, u8)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: u16 = parts[0].parse().ok()?;
    let month: u8 = parts[1].parse().ok()?;
    let day: u8 = parts[2].parse().ok()?;
    if (1..=12).contains(&month) && (1..=31).contains(&day) {
        Some((year, month, day))
    } else {
        None
    }
}

/// Format minutes as HH:MM.
pub fn format_duration(minutes: i64) -> String {
    let h = minutes / 60;
    let m = minutes % 60;
    format!("{h}:{m:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_roundtrip() {
        for (y, m, d) in [(2024, 1, 1), (2024, 6, 15), (2024, 12, 31), (2026, 3, 21)] {
            let days = date_to_days(y, m, d);
            assert_eq!(days_to_date(days), (y, m, d));
        }
    }

    #[test]
    fn parse_date_valid() {
        assert_eq!(parse_date("2024-01-15"), Some((2024, 1, 15)));
        assert_eq!(parse_date("2026-12-31"), Some((2026, 12, 31)));
    }

    #[test]
    fn parse_date_invalid() {
        assert_eq!(parse_date("not-a-date"), None);
        assert_eq!(parse_date("2024-13-01"), None);
    }

    #[test]
    fn format_duration_values() {
        assert_eq!(format_duration(90), "1:30");
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(605), "10:05");
    }

    #[test]
    fn day_of_week_known() {
        assert_eq!(day_of_week(2024, 1, 1), 0); // Monday
        assert_eq!(day_of_week(2024, 6, 15), 5); // Saturday
    }
}
