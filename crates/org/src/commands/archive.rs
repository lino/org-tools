// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org archive` subcommand arguments and archive mutation handler.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;

use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::source::SourceFile;

use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

/// CLI arguments for `org archive`.
#[derive(Args)]
pub(crate) struct ArchiveArgs {
    /// Files or directories to scan.
    #[arg(default_value = ".")]
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

/// Writes both the pruned source file and appended `.org_archive` file to disk.
fn write_archived_files(
    file: &Path,
    result: &org_tools_core::archive::ArchiveResult,
) -> Result<(), String> {
    std::fs::write(file, &result.source_content)
        .map_err(|e| format!("error writing {}: {e}", file.display()))?;
    let mut archive_content = if result.archive_path.exists() {
        std::fs::read_to_string(&result.archive_path).unwrap_or_default()
    } else {
        String::new()
    };
    archive_content.push_str(&result.archive_content);
    std::fs::write(&result.archive_path, &archive_content)
        .map_err(|e| format!("error writing {}: {e}", result.archive_path.display()))
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

    if args.format == MutationOutputFormat::Json {
        output_mutation(&mutation, MutationOutputFormat::Json);
    } else if mutation.entries_affected == 0 {
        println!("No entries to archive");
    }
    EXIT_OK
}
