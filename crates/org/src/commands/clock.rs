// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org clock` subcommand definitions and report/status handlers.

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};

use org_tools_core::files::collect_org_files;

use super::parse_org_files;
use crate::{clock, date, EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

/// Output format for clock commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ClockOutputFormat {
    /// Human-readable output.
    Human,
    /// JSON output.
    Json,
}

/// Subcommands for `org clock`.
#[derive(Subcommand)]
pub(crate) enum ClockCommand {
    /// Show clock time report.
    Report {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Start date (YYYY-MM-DD).
        #[arg(long)]
        from: Option<String>,

        /// End date (YYYY-MM-DD).
        #[arg(long)]
        to: Option<String>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: ClockOutputFormat,

        /// Group time by entry, tag, or day.
        #[arg(long, value_enum, default_value = "entry")]
        group_by: clock::report::GroupBy,

        /// Filter to entries with these tags.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    /// Show running clocks.
    Status {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: ClockOutputFormat,
    },
}

/// Parses an optional `YYYY-MM-DD` date bound flag, logging an error on invalid input.
fn parse_date_bound(raw: Option<&str>, flag_name: &str) -> Result<Option<(u16, u8, u8)>, i32> {
    let Some(text) = raw else {
        return Ok(None);
    };
    match date::parse_date(text) {
        Some(parsed) => Ok(Some(parsed)),
        None => {
            eprintln!("org: invalid {flag_name} date (expected YYYY-MM-DD)");
            Err(EXIT_ERROR)
        }
    }
}

/// Executes `org clock report`.
fn run_clock_report(
    paths: &[PathBuf],
    from: Option<&str>,
    to: Option<&str>,
    format: ClockOutputFormat,
    group_by: clock::report::GroupBy,
    tags: &[String],
) -> i32 {
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    let from_date = match parse_date_bound(from, "--from") {
        Ok(d) => d,
        Err(code) => return code,
    };
    let to_date = match parse_date_bound(to, "--to") {
        Ok(d) => d,
        Err(code) => return code,
    };

    let docs = parse_org_files(&files);
    let rows = clock::report::build_report(&docs, from_date, to_date, group_by, tags);
    match format {
        ClockOutputFormat::Human => print!("{}", clock::report::render_human(&rows, group_by)),
        ClockOutputFormat::Json => println!("{}", clock::report::render_json(&rows, group_by)),
    }
    if rows.is_empty() {
        EXIT_ISSUES
    } else {
        EXIT_OK
    }
}

/// Executes `org clock status`.
fn run_clock_status(paths: &[PathBuf], format: ClockOutputFormat) -> i32 {
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    let docs = parse_org_files(&files);
    let running = clock::status::find_running_clocks(&docs);
    match format {
        ClockOutputFormat::Human => print!("{}", clock::status::render_human(&running)),
        ClockOutputFormat::Json => println!("{}", clock::status::render_json(&running)),
    }
    if running.is_empty() {
        EXIT_ISSUES
    } else {
        EXIT_OK
    }
}

/// Runs the `clock` subcommand (`report` or `status`).
pub(crate) fn run_clock(command: ClockCommand) -> i32 {
    match command {
        ClockCommand::Report {
            paths,
            from,
            to,
            format,
            group_by,
            tags,
        } => run_clock_report(
            &paths,
            from.as_deref(),
            to.as_deref(),
            format,
            group_by,
            &tags,
        ),
        ClockCommand::Status { paths, format } => run_clock_status(&paths, format),
    }
}
