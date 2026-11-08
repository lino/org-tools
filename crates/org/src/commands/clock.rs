// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand definitions and report, status, and mutation handlers for `org clock`.

use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};

use org_tools_core::files::{canonicalize_clean, collect_org_files};
use org_tools_core::locator::OrgLocator;

use super::parse_org_files;
use super::update::invalidate_warm_mabuse_caches;
use crate::clock::mutate::{
    execute_clock_in_on_files, terminate_active_clock, ClockMutationResult,
};
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
    /// Start an active clock on a target heading entry.
    In {
        /// Target heading locator (`id:<uuid>`, `<file>::#<id>`, `<file>::*Heading`, or `<file>::<line>`).
        locator: String,

        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Explicit start timestamp (`YYYY-MM-DDTHH:MM`, `YYYY-MM-DD HH:MM`, or `[YYYY-MM-DD Day HH:MM]`).
        #[arg(long)]
        at: Option<String>,

        /// Automatically close any existing running clock at the same timestamp before clocking in.
        #[arg(long)]
        auto_out: bool,

        /// Preview changes without modifying files on disk.
        #[arg(long)]
        dry_run: bool,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: ClockOutputFormat,
    },
    /// Close an active running clock and record its elapsed duration.
    Out {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Optional locator targeting a specific entry with a running clock.
        #[arg(long)]
        locator: Option<String>,

        /// Explicit end timestamp (`YYYY-MM-DDTHH:MM`, `YYYY-MM-DD HH:MM`, or `[YYYY-MM-DD Day HH:MM]`).
        #[arg(long)]
        at: Option<String>,

        /// Preview changes without modifying files on disk.
        #[arg(long)]
        dry_run: bool,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: ClockOutputFormat,
    },
    /// Cancel an active running clock without recording elapsed duration.
    Cancel {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Optional locator targeting a specific entry with a running clock.
        #[arg(long)]
        locator: Option<String>,

        /// Preview changes without modifying files on disk.
        #[arg(long)]
        dry_run: bool,

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

/// Parses a clock locator string, supporting `<file>::<Heading>` outline shorthand.
pub(crate) fn parse_clock_locator(raw: &str) -> Result<OrgLocator, String> {
    let trimmed = raw.trim();
    if let Ok(loc) = OrgLocator::parse(trimmed) {
        return Ok(loc);
    }
    if let Some((fp, rest)) = trimmed.split_once("::") {
        let s = rest.trim();
        let ok = !fp.is_empty() && !s.is_empty() && !s.starts_with('#') && !s.starts_with("*/");
        if ok && s.parse::<usize>().is_err() {
            if let Ok(loc) = OrgLocator::parse(&format!("{fp}::*/{s}")) {
                return Ok(loc);
            }
        }
    }
    Err(format!("invalid locator '{raw}'"))
}

/// Extracts the explicit file path from a file-scoped [`OrgLocator`], if any.
pub(crate) fn locator_file_path(loc: &OrgLocator) -> Option<&Path> {
    match loc {
        OrgLocator::CustomId { file, .. }
        | OrgLocator::OutlinePath { file, .. }
        | OrgLocator::LineRef { file, .. } => Some(file.as_path()),
        OrgLocator::Id(_) => None,
    }
}

/// Collects deduplicated `.org` files from `paths`, adapting default `"."` when `locator` points elsewhere.
fn collect_clock_files(
    paths: &[PathBuf],
    locator: Option<&OrgLocator>,
) -> Result<Vec<PathBuf>, String> {
    let mut files = collect_org_files(paths);
    if let Some(lf) = locator.and_then(locator_file_path).filter(|p| p.exists()) {
        let lc = canonicalize_clean(lf).unwrap_or_else(|_| lf.to_path_buf());
        let has_lc = |list: &[PathBuf]| {
            list.iter()
                .any(|f| canonicalize_clean(f).unwrap_or_else(|_| f.clone()) == lc)
        };
        if !has_lc(&files) {
            if paths == [PathBuf::from(".")] {
                let dir = lf
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(lf);
                files = collect_org_files(&[dir.to_path_buf()]);
            }
            if !has_lc(&files) {
                files.push(lf.to_path_buf());
            }
        }
    }
    if files.is_empty() {
        return Err("no .org files found".to_string());
    }
    Ok(files)
}

/// Emits a [`ClockMutationResult`] in human-readable or JSON format and invalidates warm caches.
fn finish_clock_mutation(
    outcome: Result<(ClockMutationResult, Vec<PathBuf>), String>,
    dry_run: bool,
    format: ClockOutputFormat,
) -> i32 {
    let (result, modified_paths) = match outcome {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    if !dry_run {
        invalidate_warm_mabuse_caches(None, &modified_paths);
    }
    match format {
        ClockOutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).unwrap_or_default()
            );
        }
        ClockOutputFormat::Human => {
            let prefix = if result.dry_run { "Would " } else { "" };
            for d in &result.details {
                let extra = d
                    .detail
                    .as_deref()
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default();
                println!(
                    "{prefix}{}: {}:{} {}{extra}",
                    d.action, d.file, d.line, d.title
                );
            }
        }
    }
    EXIT_OK
}

/// Executes `org clock out` or `org clock cancel`.
fn run_clock_termination(
    paths: &[PathBuf],
    locator: Option<&str>,
    at: Option<&str>,
    cancel: bool,
    dry_run: bool,
    format: ClockOutputFormat,
) -> i32 {
    let outcome = locator
        .map(parse_clock_locator)
        .transpose()
        .and_then(|p| collect_clock_files(paths, p.as_ref()))
        .and_then(|f| terminate_active_clock(&f, locator, at, cancel, dry_run));
    finish_clock_mutation(outcome, dry_run, format)
}

/// Runs the `clock` subcommand (`report`, `status`, `in`, `out`, or `cancel`).
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
        ClockCommand::In {
            locator,
            paths,
            at,
            auto_out,
            dry_run,
            format,
        } => {
            let outcome = parse_clock_locator(&locator)
                .and_then(|p| collect_clock_files(&paths, Some(&p)))
                .and_then(|f| {
                    execute_clock_in_on_files(&f, &locator, at.as_deref(), auto_out, dry_run)
                });
            finish_clock_mutation(outcome, dry_run, format)
        }
        ClockCommand::Out {
            paths,
            locator,
            at,
            dry_run,
            format,
        } => run_clock_termination(
            &paths,
            locator.as_deref(),
            at.as_deref(),
            false,
            dry_run,
            format,
        ),
        ClockCommand::Cancel {
            paths,
            locator,
            dry_run,
            format,
        } => run_clock_termination(&paths, locator.as_deref(), None, true, dry_run, format),
    }
}
