// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org export` subcommand definitions and iCalendar / JSCalendar export handlers.

use std::path::PathBuf;

use clap::Subcommand;

use org_tools_core::files::collect_org_files;

use super::parse_org_files;
use crate::{date, export, EXIT_ERROR, EXIT_OK};

/// Subcommands for `org export`.
#[derive(Subcommand)]
pub(crate) enum ExportCommand {
    /// Export to iCalendar (.ics) format.
    Ical {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output file (default: stdout).
        #[arg(long, short)]
        output: Option<PathBuf>,

        /// Start date filter (YYYY-MM-DD).
        #[arg(long)]
        from: Option<String>,

        /// End date filter (YYYY-MM-DD).
        #[arg(long)]
        to: Option<String>,

        /// Filter to entries with these tags.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    /// Export to JSCalendar (JSON) format.
    Jscal {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output file (default: stdout).
        #[arg(long, short)]
        output: Option<PathBuf>,

        /// Start date filter (YYYY-MM-DD).
        #[arg(long)]
        from: Option<String>,

        /// End date filter (YYYY-MM-DD).
        #[arg(long)]
        to: Option<String>,

        /// Filter to entries with these tags.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
}

/// Writes exported calendar text to `output` if provided, or prints it to stdout.
fn emit_exported_payload(payload: &str, output: Option<&PathBuf>, trailing_newline: bool) -> i32 {
    if let Some(path) = output {
        if let Err(e) = std::fs::write(path, payload) {
            eprintln!("org: error writing {}: {}", path.display(), e);
            return EXIT_ERROR;
        }
        println!("Exported to {}", path.display());
    } else if trailing_newline {
        println!("{payload}");
    } else {
        print!("{payload}");
    }
    EXIT_OK
}

/// Executes either `ical` or `jscal` calendar export.
fn execute_calendar_export(
    paths: &[PathBuf],
    output: Option<&PathBuf>,
    from: Option<&str>,
    to: Option<&str>,
    tags: &[String],
    jscal_mode: bool,
) -> i32 {
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    let from_date = from.and_then(date::parse_date);
    let to_date = to.and_then(date::parse_date);
    let docs = parse_org_files(&files);

    if jscal_mode {
        let json = export::jscal::export_jscal(&docs, from_date, to_date, tags);
        emit_exported_payload(&json, output, true)
    } else {
        let ical = export::ical::export_ical(&docs, from_date, to_date, tags);
        emit_exported_payload(&ical, output, false)
    }
}

/// Runs the `export` subcommand (`ical` or `jscal`).
pub(crate) fn run_export(command: ExportCommand) -> i32 {
    match command {
        ExportCommand::Ical {
            paths,
            output,
            from,
            to,
            tags,
        } => execute_calendar_export(
            &paths,
            output.as_ref(),
            from.as_deref(),
            to.as_deref(),
            &tags,
            false,
        ),
        ExportCommand::Jscal {
            paths,
            output,
            from,
            to,
            tags,
        } => execute_calendar_export(
            &paths,
            output.as_ref(),
            from.as_deref(),
            to.as_deref(),
            &tags,
            true,
        ),
    }
}
