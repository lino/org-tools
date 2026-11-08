// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Core library for the `org` CLI suite.
//!
//! Provides configuration, source file handling, format/lint rule traits,
//! the runner pipeline, diagnostic output, file collection, and the
//! [`OrgDocument`](document::OrgDocument) heading tree model.

/// Subtree archiving and `#+ARCHIVE:` target resolution.
pub mod archive;
/// Persistent SQLite/FTS5 workspace index (`MabuseCache`).
pub mod cache;
/// `#+COLUMNS:` format parser and bottom-up property rollup evaluator.
pub mod columns;
/// Configuration loading and validation for `.org-tools.toml`.
pub mod config;
/// Statistics cookie (`[n/m]` and `[n%]`) computation and updates.
pub mod cookie;
/// Diagnostic, severity, span, and auto-fix types.
pub mod diagnostic;
/// Heading tree, planning, drawer, clock, and checklist document model.
pub mod document;
/// Pure-Rust `org-edna` `BLOCKER` and `TRIGGER` dependency engine.
pub mod edna;
/// Heading insertion, promotion, demotion, and checklist mutations.
pub mod entry;
/// Workspace slice extraction and GitHub Flavored Markdown export.
pub mod export_md;
/// Recursive `.org` file discovery respecting `.gitignore`.
pub mod files;
/// Non-overlapping span auto-fix application engine.
pub mod formatter;
/// `:ID:` UUID discovery, indexing, and link resolution.
pub mod id;
/// Canonical `OrgLocator` (`id:`, `file:`, `#custom-id`, `*Heading`) resolver.
pub mod locator;
/// Human-readable and JSON diagnostic formatters.
pub mod output;
/// Timestamp repeaters, deadline warnings, body timestamp scanning, and planning mutations.
pub mod planning;
/// Property drawer mutations (`F005`), tag alignment (`F007`), and Stempelamt UUIDv5 minting.
pub mod property;
/// Atomic subtree refiling across headings and files (`org refile`).
pub mod refile;
/// `org-roam` v2 node, alias, ref, and backlink queries.
pub mod roam;
/// Format (`F*`) and lint (`E*`, `W*`, `I*`) rule traits and implementations.
pub mod rules;
/// Rule execution pipeline across source files.
pub mod runner;
/// Source text buffer with line and column lookup tables.
pub mod source;
/// TODO state transitions, `CLOSED:` management, and repeater auto-advance.
pub mod state;
/// Pure-Rust `#+TBLFM:` spreadsheet formula parser and evaluator.
pub mod tblfm;

/// Behavioural test suite covering all four `ORGFMT-WP-12` planning and repeater slices.
#[cfg(test)]
mod wp12_planning_and_repeater_tests {
    use crate::config::{AgendaConfig, Config, DEFAULT_DEADLINE_WARNING_DAYS};
    use crate::document::OrgDocument;
    use crate::planning::{
        advance_timestamp_repeater, date_to_days, day_of_week_abbrev, days_to_date,
        effective_deadline_warning, effective_deadline_warning_days,
        extract_entry_active_body_timestamps, is_first_only_warning, parse_repeater, parse_warning,
        project_repeating_dates, set_planning_timestamp, shift_calendar_date, PlanningKind,
        RepeaterKind, RepeaterSpec, TimeUnit, WarningKind, WarningSpec,
    };
    use crate::property::resolve_tag_column;
    use crate::rules::timestamp::parse_timestamp;
    use crate::source::SourceFile;
    use crate::state::{set_state, set_state_with_options, SetStateOptions, StateChangeResult};

    /// Parses raw Org markup into a `(SourceFile, OrgDocument)` test fixture.
    fn parse_fixture(raw: &str) -> (SourceFile, OrgDocument) {
        let src = SourceFile::new("fixture.org", raw.to_string());
        let doc = OrgDocument::from_source(&src);
        (src, doc)
    }

    // region Slice 1: Repeaters, Warnings & Calendar Math

    /// Verifies repeater/warning parsing across all units (`h`, `d`, `w`, `m`, `y`) and config defaults.
    #[test]
    fn slice1_parse_repeaters_warnings_and_config_defaults() {
        assert_eq!(DEFAULT_DEADLINE_WARNING_DAYS, 14);
        assert_eq!(AgendaConfig::default().deadline_warning_days, 14);
        assert_eq!(Config::default().agenda.deadline_warning_days, 14);

        let r_cases = [
            ("+6h", RepeaterKind::Cumulative, 6, TimeUnit::Hour),
            ("+3d", RepeaterKind::Cumulative, 3, TimeUnit::Day),
            ("+2w", RepeaterKind::Cumulative, 2, TimeUnit::Week),
            ("++1m", RepeaterKind::CatchUp, 1, TimeUnit::Month),
            (".+5y", RepeaterKind::Restart, 5, TimeUnit::Year),
        ];
        for (raw, kind, value, unit) in r_cases {
            assert_eq!(
                parse_repeater(raw),
                Some(RepeaterSpec { kind, value, unit })
            );
        }
        assert!(parse_repeater("+").is_none() && parse_repeater("+++1d").is_none());

        let w_cases = [
            ("-36h", WarningKind::All, 36, TimeUnit::Hour),
            ("-0d", WarningKind::All, 0, TimeUnit::Day),
            ("--2w", WarningKind::First, 2, TimeUnit::Week),
            ("-1m", WarningKind::All, 1, TimeUnit::Month),
            ("--1y", WarningKind::First, 1, TimeUnit::Year),
        ];
        for (raw, kind, value, unit) in w_cases {
            assert_eq!(parse_warning(raw), Some(WarningSpec { kind, value, unit }));
        }
        assert!(parse_warning("-").is_none() && parse_warning("---2d").is_none());
    }

    /// Verifies calendar conversions, month-end/leap-day clamping, and effective deadline warnings.
    #[test]
    fn slice1_calendar_clamping_and_effective_deadline_warnings() {
        assert_eq!(date_to_days(1970, 1, 1), 0);
        assert_eq!(days_to_date(0), (1970, 1, 1));
        assert_eq!(day_of_week_abbrev(1970, 1, 1), "Thu");
        assert_eq!(
            shift_calendar_date(2025, 1, 31, 1, TimeUnit::Month),
            (2025, 2, 28)
        );
        assert_eq!(
            shift_calendar_date(2024, 1, 31, 1, TimeUnit::Month),
            (2024, 2, 29)
        );
        assert_eq!(
            shift_calendar_date(2024, 2, 29, 1, TimeUnit::Year),
            (2025, 2, 28)
        );

        let (plain, _) = parse_timestamp("<2026-03-31 Tue>", 0).unwrap();
        assert_eq!(effective_deadline_warning_days(&plain, 14), 14);
        assert!(!is_first_only_warning(&plain));

        let (zero_w, _) = parse_timestamp("<2026-03-31 Tue -0d>", 0).unwrap();
        assert_eq!(effective_deadline_warning_days(&zero_w, 14), 0);

        let (hour_w, _) = parse_timestamp("<2026-03-31 Tue -48h>", 0).unwrap();
        assert_eq!(effective_deadline_warning_days(&hour_w, 14), 2);

        let (month_w, _) = parse_timestamp("<2026-03-31 Tue +1m --1m>", 0).unwrap();
        assert_eq!(effective_deadline_warning(&month_w, 14), (31, true));
        assert!(is_first_only_warning(&month_w));

        let (non_rep_first, _) = parse_timestamp("<2026-03-31 Tue --5d>", 0).unwrap();
        assert_eq!(effective_deadline_warning(&non_rep_first, 14), (5, false));
    }

    /// Verifies `+`, `++`, `.+` repeater advancement (including hourly wrap) and range projection.
    #[test]
    fn slice1_advance_repeater_and_project_dates() {
        let today = (2026, 3, 5);
        let (cum, _) = parse_timestamp("<2026-02-01 Sun 09:30 +1w -2d>", 0).unwrap();
        assert_eq!(
            advance_timestamp_repeater(&cum, today).unwrap().to_string(),
            "<2026-02-08 Sun 09:30 +1w -2d>"
        );

        let (catch, _) = parse_timestamp("<2026-02-01 Sun ++2w>", 0).unwrap();
        assert_eq!(
            advance_timestamp_repeater(&catch, today)
                .unwrap()
                .to_string(),
            "<2026-03-15 Sun ++2w>"
        );

        let (restart, _) = parse_timestamp("<2026-01-31 Sat .+1m>", 0).unwrap();
        assert_eq!(
            advance_timestamp_repeater(&restart, (2025, 1, 31))
                .unwrap()
                .to_string(),
            "<2025-02-28 Fri .+1m>"
        );

        let (hourly, _) = parse_timestamp("<2026-03-01 Sun 20:15 +6h>", 0).unwrap();
        assert_eq!(
            advance_timestamp_repeater(&hourly, today)
                .unwrap()
                .to_string(),
            "<2026-03-02 Mon 02:15 +6h>"
        );
        assert_eq!(
            project_repeating_dates(&hourly, (2026, 3, 1), 3),
            vec![(2026, 3, 2), (2026, 3, 3)]
        );

        let (zero_rep, _) = parse_timestamp("<2026-03-01 Sun +0d>", 0).unwrap();
        assert!(advance_timestamp_repeater(&zero_rep, today).is_none());
        assert!(project_repeating_dates(&zero_rep, (2026, 3, 1), 7).is_empty());
        assert!(project_repeating_dates(&cum, (2026, 3, 1), 0).is_empty());
        assert_eq!(
            project_repeating_dates(&cum, (2026, 2, 1), 15),
            vec![(2026, 2, 8), (2026, 2, 15)]
        );
    }

    // endregion

    // region Slice 2: Active Body Timestamp Scanner

    /// Verifies active body scanner extracts own heading/body `<...>` while skipping excluded zones.
    #[test]
    fn slice2_extract_entry_active_body_timestamps_filters_excluded_zones() {
        let raw = concat!(
            "* TODO Parent <2026-04-01 Wed 10:00>\n",
            "SCHEDULED: <2026-04-02 Thu>\n",
            ":PROPERTIES:\n",
            ":CREATED: <2026-04-03 Fri>\n",
            ":END:\n",
            ":LOGBOOK:\n",
            "- Note <2026-04-04 Sat>\n",
            ":END:\n",
            "Inactive [2026-04-05 Sun] and invalid <2026-02-30 Mon>.\n",
            "Verbatim =<2026-04-06 Mon>= and code ~<2026-04-07 Tue>~.\n",
            "Equation a = 1 <2026-04-08 Wed> b = 2.\n",
            "#+BEGIN_SRC org\n",
            "<2026-04-09 Thu>\n",
            "#+END_SRC\n",
            "#+BEGIN_QUOTE\n",
            "<2026-04-10 Fri>\n",
            "#+END_QUOTE\n",
            "** TODO Child <2026-04-11 Sat>\n",
            "<2026-04-12 Sun>\n",
        );
        let (src, doc) = parse_fixture(raw);
        let parent_ts = extract_entry_active_body_timestamps(&src, &doc, 0);
        let formatted: Vec<(usize, String)> = parent_ts
            .into_iter()
            .map(|b| (b.line, b.timestamp.to_string()))
            .collect();
        assert_eq!(
            formatted,
            vec![
                (1, "<2026-04-01 Wed 10:00>".to_string()),
                (11, "<2026-04-08 Wed>".to_string()),
            ]
        );

        let child_ts = extract_entry_active_body_timestamps(&src, &doc, 1);
        assert_eq!(child_ts.len(), 2);
        assert!(extract_entry_active_body_timestamps(&src, &doc, 99).is_empty());
    }

    /// Verifies malformed `<2026-03-00>` timestamps never underflow in `parse_timestamp` or `date_to_days`.
    #[test]
    fn regression_day_zero_timestamp_never_panics() {
        let (ts, end) = parse_timestamp("<2026-03-00>", 0).unwrap();
        assert_eq!((ts.year, ts.month, ts.day, end), (2026, 3, 0, 12));
        let _ = date_to_days(2026, 3, 0);
        let _ = date_to_days(2026, 0, 0);
        let (src, doc) = parse_fixture("* Heading\n<2026-03-00>\n");
        assert!(extract_entry_active_body_timestamps(&src, &doc, 0).is_empty());
    }

    // endregion

    // region Slice 3: Planning Timestamp Mutations

    /// Verifies inserting and relative-shifting `SCHEDULED:` / `DEADLINE:` while preserving siblings and indent.
    #[test]
    fn slice3_set_planning_timestamp_insert_and_relative_shift() {
        let today = (2026, 3, 5);
        let (s0, d0) = parse_fixture("* TODO Release prep\nBody line.\n");
        let m1 =
            set_planning_timestamp(&s0, &d0, 0, PlanningKind::Scheduled, Some("."), today).unwrap();
        assert!(m1.changed && m1.old_timestamp.is_none());
        assert_eq!(m1.new_timestamp.as_deref(), Some("<2026-03-05 Thu>"));
        assert_eq!(
            m1.content,
            "* TODO Release prep\nSCHEDULED: <2026-03-05 Thu>\nBody line.\n"
        );

        let (s1, d1) = parse_fixture(
            "** TODO Subtask\n  CLOSED: [2026-03-01 Sun 08:00] SCHEDULED: <2026-03-05 Thu>\n",
        );
        let m2 = set_planning_timestamp(
            &s1,
            &d1,
            0,
            PlanningKind::Deadline,
            Some("+2w 17:00 ++1m --3d"),
            today,
        )
        .unwrap();
        assert_eq!(
            m2.content,
            "** TODO Subtask\n  CLOSED: [2026-03-01 Sun 08:00] SCHEDULED: <2026-03-05 Thu> DEADLINE: <2026-03-19 Thu 17:00 ++1m --3d>\n"
        );
    }

    /// Verifies clearing `SCHEDULED:` / `DEADLINE:` and rejecting out-of-bounds or invalid date specs.
    #[test]
    fn slice3_set_planning_timestamp_clear_and_reject_invalid_specs() {
        let today = (2026, 3, 5);
        let raw = "** TODO Subtask\n  CLOSED: [2026-03-01 Sun 08:00] SCHEDULED: <2026-03-05 Thu> DEADLINE: <2026-03-19 Thu>\n";
        let (s2, d2) = parse_fixture(raw);
        let m3 = set_planning_timestamp(&s2, &d2, 0, PlanningKind::Scheduled, Some("clear"), today)
            .unwrap();
        assert_eq!(m3.old_timestamp.as_deref(), Some("<2026-03-05 Thu>"));
        assert!(m3.new_timestamp.is_none() && !m3.content.contains("SCHEDULED:"));
        assert!(m3.content.contains("CLOSED:") && m3.content.contains("DEADLINE:"));

        let (s3, d3) = parse_fixture("* TODO Solo\nDEADLINE: <2026-03-10 Tue>\nNext.\n");
        let m4 = set_planning_timestamp(&s3, &d3, 0, PlanningKind::Deadline, None, today).unwrap();
        assert_eq!(m4.content, "* TODO Solo\nNext.\n");

        let sched = PlanningKind::Scheduled;
        assert!(set_planning_timestamp(&s3, &d3, 5, sched, Some("today"), today).is_err());
        assert!(set_planning_timestamp(&s3, &d3, 0, sched, Some("2025-02-29"), today).is_err());
        assert!(
            set_planning_timestamp(&s3, &d3, 0, sched, Some("[2026-03-05 Thu]"), today).is_err()
        );
    }

    // endregion

    // region Slice 4: State Auto-Advance & Repeaters

    /// Verifies repeater auto-advance across custom `#+TODO:`, heading/planning/body, hourly wrap, and `F007` tags.
    #[test]
    fn slice4_state_auto_advance_custom_keywords_tags_and_hourly() {
        let raw = concat!(
            "#+TODO: NEXT WAIT | DONE\n",
            "* WAIT [#B] Standup <2026-03-01 Sun 22:00 +6h> :team:\n",
            "CLOSED: [2026-02-28 Sat 09:00] SCHEDULED: <2026-03-01 Sun +1w> DEADLINE: <2026-02-15 Sun ++2w -3d>\n",
            "Follow-up <2026-03-01 Sun .+4d> and verbatim =<2026-03-01 Sun +1d>=.\n",
            "* NEXT Tomorrow habit\n",
            "SCHEDULED: <2026-03-06 Fri .+1d>\n",
        );
        let (src, doc) = parse_fixture(raw);
        let opts = SetStateOptions {
            today: Some((2026, 3, 5)),
            ..Default::default()
        };
        let res: StateChangeResult =
            set_state_with_options(&src, &doc, None, "DONE", opts).unwrap();
        assert_eq!(res.repeated_indices, vec![0, 1]);
        assert!(res.newly_done_indices.is_empty() && !res.content.contains("CLOSED:"));

        let h0 = res.content.lines().nth(1).unwrap();
        assert!(h0.starts_with("* NEXT [#B] Standup <2026-03-02 Mon 04:00 +6h> "));
        assert!(h0.ends_with(" :team:"));
        assert_eq!(
            unicode_width::UnicodeWidthStr::width(h0),
            resolve_tag_column(None)
        );
        assert!(res
            .content
            .contains("SCHEDULED: <2026-03-08 Sun +1w> DEADLINE: <2026-03-15 Sun ++2w -3d>"));
        assert!(res
            .content
            .contains("Follow-up <2026-03-09 Mon .+4d> and verbatim =<2026-03-01 Sun +1d>=."));
        assert!(res
            .content
            .contains("* NEXT Tomorrow habit\nSCHEDULED: <2026-03-06 Fri .+1d>"));
    }

    /// Verifies `advance_repeaters: false`, `set_state` wrapper defaults, and `CLOSED:` cleanup on reopen.
    #[test]
    fn slice4_state_no_repeat_override_wrapper_and_closed_cleanup() {
        let raw = "* TODO [#B] Standup\nSCHEDULED: <2026-03-01 Sun +1w>\n";
        let (src, doc) = parse_fixture(raw);
        let no_rep = SetStateOptions {
            add_closed: true,
            advance_repeaters: false,
            today: Some((2026, 3, 5)),
        };
        let forced = set_state_with_options(&src, &doc, Some(&[0]), "DONE", no_rep).unwrap();
        assert!(forced.repeated_indices.is_empty() && forced.newly_done_indices == vec![0]);
        assert!(
            forced.content.contains("* DONE [#B] Standup") && forced.content.contains("CLOSED:")
        );

        let wrapper = set_state(&src, &doc, Some(&[0]), "DONE", true).unwrap();
        assert_eq!(wrapper.repeated_indices, vec![0]);

        let (s_done, d_done) = parse_fixture("* DONE Task\nCLOSED: [2026-03-01 Sun 10:00]\n");
        let reopened = set_state(&s_done, &d_done, None, "TODO", true).unwrap();
        assert!(reopened.content.contains("* TODO Task") && !reopened.content.contains("CLOSED:"));
        assert!(set_state(&s_done, &d_done, None, "DONE", true).is_none());
    }

    // endregion
}
