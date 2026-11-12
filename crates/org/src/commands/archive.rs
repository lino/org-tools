// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org archive` subcommand arguments and archive mutation handler.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, ValueHint};

use org_tools_core::document::OrgDocument;
use org_tools_core::files::{collect_org_files, write_file_atomic};
use org_tools_core::source::SourceFile;

use super::update::invalidate_warm_workspace_caches;
use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

/// CLI arguments for `org archive`.
#[derive(Args)]
pub(crate) struct ArchiveArgs {
    /// Files or directories to scan.
    #[arg(default_value = ".", value_hint = ValueHint::AnyPath)]
    pub(crate) paths: Vec<PathBuf>,

    /// Archive target (file::heading). Overrides #+ARCHIVE: setting.
    #[arg(long)]
    pub(crate) target: Option<String>,

    /// Filter to entries with these tags.
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,

    /// Print what would be archived without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// Appends archived subtrees to `.org_archive` first, then atomically updates the source file.
fn write_archived_files(
    file: &Path,
    result: &org_tools_core::archive::ArchiveResult,
) -> Result<(), String> {
    if let Some(parent) = result
        .archive_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("error creating directory {}: {e}", parent.display()))?;
        }
    }

    let mut append_content = String::new();
    if result.archive_path.exists() {
        if let Ok(existing) = std::fs::read_to_string(&result.archive_path) {
            if !existing.is_empty() && !existing.ends_with('\n') {
                append_content.push('\n');
            }
        }
    }
    append_content.push_str(&result.archive_content);

    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&result.archive_path)
        .and_then(|mut f| {
            f.write_all(append_content.as_bytes())?;
            f.sync_all()
        })
        .map_err(|e| {
            format!(
                "error writing archive {}: {e}. Source file was not modified.",
                result.archive_path.display()
            )
        })?;

    write_file_atomic(file, &result.source_content)
        .map_err(|e| format!("error updating source file {}: {e}", file.display()))
}

/// Prints the human-mode per-file summary for `org archive`.
fn emit_archive_human(
    file: &Path,
    archive_dest: &str,
    archived: usize,
    archivable: &[org_tools_core::archive::ArchiveEntry],
    dry_run: bool,
) {
    let suffix = if archived == 1 { "y" } else { "ies" };
    if dry_run {
        println!(
            "Would archive {archived} entr{suffix} from {} to {archive_dest}",
            file.display()
        );
        for entry in archivable {
            println!("  {} (line {})", entry.title, entry.start_line);
        }
    } else {
        println!(
            "Archived {archived} entr{suffix} from {} to {archive_dest}",
            file.display()
        );
    }
}

/// Processes archivable entries in a single Org file and records results in `mutation`.
fn process_archive_file(file: &Path, args: &ArchiveArgs, mutation: &mut MutationResult) {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("error reading {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    };
    let doc = OrgDocument::from_source(&source);
    let target_str = args
        .target
        .as_deref()
        .or_else(|| doc.file_keywords.get("ARCHIVE").map(String::as_str))
        .unwrap_or("");
    let target = org_tools_core::archive::parse_archive_target(target_str, file);
    let archivable = org_tools_core::archive::find_archivable_entries(&source, &doc, &args.tags);
    let Some(result) = org_tools_core::archive::build_archive(&source, &doc, &archivable, &target)
    else {
        return;
    };

    if !args.dry_run {
        if let Err(msg) = write_archived_files(file, &result) {
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    }

    mutation.files_modified += 1;
    mutation.entries_affected += result.archived;
    let file_str = file.display().to_string();
    let archive_dest = result.archive_path.display().to_string();
    for entry in &archivable {
        mutation.details.push(MutationDetail {
            file: file_str.clone(),
            line: entry.start_line.max(1),
            action: "archived".to_string(),
            detail: Some(format!("{} -> {archive_dest}", entry.title)),
        });
    }
    if args.format == MutationOutputFormat::Human {
        emit_archive_human(
            file,
            &archive_dest,
            result.archived,
            &archivable,
            args.dry_run,
        );
    }
}

/// Runs `org archive`.
pub(crate) fn run_archive(args: ArchiveArgs) -> i32 {
    let started = Instant::now();
    let files = collect_org_files(&args.paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }

    let mut mutation = MutationResult::new("archive", args.dry_run);
    for file in &files {
        process_archive_file(file, &args, &mut mutation);
    }
    mutation.finish(started, files.len());

    if !args.dry_run && mutation.files_modified > 0 {
        let modified: Vec<PathBuf> = mutation
            .details
            .iter()
            .map(|d| PathBuf::from(&d.file))
            .collect();
        invalidate_warm_workspace_caches(None, &modified);
    }

    if args.format == MutationOutputFormat::Json {
        output_mutation(&mutation, MutationOutputFormat::Json);
    } else if mutation.entries_affected == 0 && mutation.errors.is_empty() {
        println!("No entries to archive");
    }
    if !mutation.errors.is_empty() {
        return EXIT_ERROR;
    }
    EXIT_OK
}
