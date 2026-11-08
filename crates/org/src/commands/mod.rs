// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Modular CLI subcommand handlers and shared mutation/document helpers.

pub(crate) mod archive;
pub(crate) mod cache;
pub(crate) mod clock;
pub(crate) mod export;
pub(crate) mod fmt;
pub(crate) mod mcp;
pub(crate) mod query;
pub(crate) mod query_columns;
pub(crate) mod query_gtd;
pub(crate) mod roam;
pub(crate) mod schema;
pub(crate) mod show;
pub(crate) mod update;
pub(crate) mod update_id;
pub(crate) mod update_property;
pub(crate) mod update_state;
pub(crate) mod update_table;
pub(crate) mod update_todo;

use std::io::Read as _;
use std::path::PathBuf;
use std::time::Instant;

use clap::ValueEnum;

use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::source::SourceFile;

/// Output format for mutation commands (`update`, `archive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum MutationOutputFormat {
    /// Human-readable output.
    Human,
    /// JSON output.
    Json,
}

/// Execution telemetry included in uniform JSON mutation envelopes.
#[derive(serde::Serialize)]
pub(crate) struct MutationTelemetry {
    /// Wall-clock command execution duration in microseconds.
    pub(crate) duration_us: u64,
    /// Number of Org source files inspected during the mutation pass.
    pub(crate) files_scanned: usize,
    /// Number of document cache hits.
    pub(crate) cache_hits: usize,
    /// Number of document cache misses.
    pub(crate) cache_misses: usize,
}

/// Structured result from a mutation command.
#[derive(serde::Serialize)]
pub(crate) struct MutationResult {
    /// Canonical subcommand identifier.
    pub(crate) command: String,
    /// Number of files modified or scheduled for modification.
    pub(crate) files_modified: usize,
    /// Total number of entries or cells affected across all files.
    pub(crate) entries_affected: usize,
    /// Whether no file writes were performed (`--dry-run`).
    pub(crate) dry_run: bool,
    /// Per-entry or per-file mutation details.
    pub(crate) details: Vec<MutationDetail>,
    /// Operational error messages collected during execution.
    pub(crate) errors: Vec<String>,
    /// Execution duration and file-scan telemetry.
    pub(crate) telemetry: MutationTelemetry,
}

impl MutationResult {
    /// Creates an empty [`MutationResult`] envelope for `command`.
    pub(crate) fn new(command: &str, dry_run: bool) -> Self {
        Self {
            command: command.to_string(),
            files_modified: 0,
            entries_affected: 0,
            dry_run,
            details: Vec::new(),
            errors: Vec::new(),
            telemetry: MutationTelemetry {
                duration_us: 0,
                files_scanned: 0,
                cache_hits: 0,
                cache_misses: 0,
            },
        }
    }

    /// Populates wall-clock duration and scanned-file telemetry prior to output.
    pub(crate) fn finish(&mut self, started: Instant, files_scanned: usize) {
        self.telemetry.duration_us = started.elapsed().as_micros() as u64;
        self.telemetry.files_scanned = files_scanned;
    }
}

/// A single action within a mutation result.
#[derive(serde::Serialize)]
pub(crate) struct MutationDetail {
    /// Path to the Org file containing the mutated entry.
    pub(crate) file: String,
    /// One-based line number of the affected heading or table.
    pub(crate) line: usize,
    /// Machine-readable mutation action label.
    pub(crate) action: String,
    /// Optional human-readable detail describing the mutation.
    pub(crate) detail: Option<String>,
}

/// Prints a single [`MutationDetail`] line in human-readable mode.
fn print_mutation_detail_human(detail: &MutationDetail, dry_run: bool) {
    let extra = detail
        .detail
        .as_deref()
        .map(|d| format!(" ({d})"))
        .unwrap_or_default();
    if dry_run {
        println!(
            "Would {}: {}:{}{}",
            detail.action, detail.file, detail.line, extra
        );
    } else {
        println!(
            "{}: {}:{}{}",
            detail.action, detail.file, detail.line, extra
        );
    }
}

/// Renders a [`MutationResult`] as pretty JSON or human-readable text on stdout/stderr.
pub(crate) fn output_mutation(result: &MutationResult, format: MutationOutputFormat) {
    match format {
        MutationOutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(result).unwrap_or_default()
            );
        }
        MutationOutputFormat::Human => {
            for detail in &result.details {
                print_mutation_detail_human(detail, result.dry_run);
            }
            for err in &result.errors {
                eprintln!("org: {err}");
            }
            if result.entries_affected > 0 {
                let verb = if result.dry_run {
                    "Would affect"
                } else {
                    "Affected"
                };
                println!(
                    "{verb} {} entries in {} files",
                    result.entries_affected, result.files_modified
                );
            }
        }
    }
}

/// Loads all [`OrgDocument`] trees discovered under `paths`, logging read errors to stderr.
pub(crate) fn load_docs(paths: &[PathBuf]) -> Vec<OrgDocument> {
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return Vec::new();
    }
    parse_org_files(&files)
}

/// Parses an already-collected slice of `.org` file paths into [`OrgDocument`] values.
pub(crate) fn parse_org_files(files: &[PathBuf]) -> Vec<OrgDocument> {
    let mut docs = Vec::with_capacity(files.len());
    for file in files {
        match SourceFile::from_path(file) {
            Ok(source) => docs.push(OrgDocument::from_source(&source)),
            Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
        }
    }
    docs
}

/// Reads Org-mode text from standard input into an in-memory `<stdin>` [`SourceFile`].
pub(crate) fn read_stdin_source() -> Result<SourceFile, String> {
    let mut content = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut content)
        .map_err(|e| format!("org: error reading stdin: {e}"))?;
    Ok(SourceFile::new("<stdin>", content))
}
