// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org update add-cookie` and `org update calc` handlers for statistic cookies and `#+TBLFM:`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::source::SourceFile;

use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK, EXIT_REQUIRES_EMACS};

/// Collects per-line [`MutationDetail`] records for statistic cookies updated in `file`.
fn collect_cookie_details(file: &Path, before: &str, after: &str) -> Vec<MutationDetail> {
    let file_str = file.display().to_string();
    let details: Vec<MutationDetail> = before
        .lines()
        .zip(after.lines())
        .enumerate()
        .filter(|(_, (old_l, new_l))| old_l != new_l)
        .map(|(idx, (_, new_l))| MutationDetail {
            file: file_str.clone(),
            line: idx + 1,
            action: "updated-cookie".to_string(),
            detail: Some(new_l.trim().to_string()),
        })
        .collect();

    if details.is_empty() {
        vec![MutationDetail {
            file: file_str,
            line: 1,
            action: "updated-cookie".to_string(),
            detail: None,
        }]
    } else {
        details
    }
}

/// Prints the human-mode per-file summary for `org update add-cookie`.
fn emit_cookie_file_human(file: &Path, updated: usize, dry_run: bool) {
    let suffix = if updated == 1 { "" } else { "s" };
    if dry_run {
        println!(
            "Would update {updated} cookie{suffix} in {}",
            file.display()
        );
    } else {
        println!("Updated {updated} cookie{suffix} in {}", file.display());
    }
}

/// Applies statistic cookie updates to a single file and records results in `mutation`.
fn process_add_cookie_file(
    file: &Path,
    recursive: bool,
    dry_run: bool,
    format: MutationOutputFormat,
    mutation: &mut MutationResult,
) {
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
    let Some(result) = org_tools_core::cookie::update_cookies(&source, &doc, recursive) else {
        return;
    };

    if !dry_run {
        if let Err(e) = std::fs::write(file, &result.content) {
            let msg = format!("error writing {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    }

    mutation.files_modified += 1;
    mutation.entries_affected += result.updated;
    mutation.details.extend(collect_cookie_details(
        file,
        &source.content,
        &result.content,
    ));

    if format == MutationOutputFormat::Human {
        emit_cookie_file_human(file, result.updated, dry_run);
    }
}

/// Runs `org update add-cookie`.
pub(crate) fn run_add_cookie(
    paths: Vec<PathBuf>,
    recursive: bool,
    dry_run: bool,
    format: MutationOutputFormat,
) -> i32 {
    let started = Instant::now();
    let files = collect_org_files(&paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }

    let mut mutation = MutationResult::new("add-cookie", dry_run);
    for file in &files {
        process_add_cookie_file(file, recursive, dry_run, format, &mut mutation);
    }
    mutation.finish(started, files.len());

    if format == MutationOutputFormat::Json {
        output_mutation(&mutation, MutationOutputFormat::Json);
    } else if mutation.entries_affected == 0 && !dry_run {
        println!("All cookies are up to date");
    }
    EXIT_OK
}

/// Evaluates `#+TBLFM:` formulas in a single file and returns whether Emacs was required.
fn process_calc_file(file: &Path, dry_run: bool, mutation: &mut MutationResult) -> bool {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            mutation.errors.push(format!("{}: {e}", file.display()));
            return false;
        }
    };
    let doc = OrgDocument::from_source(&source);
    let result = org_tools_core::tblfm::calc_file(&source.content, &doc.table_constants);

    let mut has_emacs_error = false;
    for err in &result.errors {
        if matches!(err, org_tools_core::tblfm::TblfmError::RequiresEmacs(_)) {
            has_emacs_error = true;
        }
        mutation.errors.push(format!("{}: {err}", file.display()));
    }

    if result.cells_updated > 0 {
        let first_table_line = source
            .content
            .lines()
            .position(|l| l.trim_start().starts_with('|'))
            .map_or(1, |idx| idx + 1);
        mutation.files_modified += 1;
        mutation.entries_affected += result.cells_updated;
        mutation.details.push(MutationDetail {
            file: file.display().to_string(),
            line: first_table_line,
            action: format!("update {} cells", result.cells_updated),
            detail: None,
        });

        if !dry_run {
            if let Err(e) = std::fs::write(file, &result.content) {
                mutation
                    .errors
                    .push(format!("{}: write error: {e}", file.display()));
            }
        }
    }
    has_emacs_error
}

/// Runs `org update calc`.
pub(crate) fn run_calc(paths: Vec<PathBuf>, dry_run: bool, format: MutationOutputFormat) -> i32 {
    let started = Instant::now();
    let files = collect_org_files(&paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }

    let mut mutation = MutationResult::new("calc", dry_run);
    let mut has_emacs_error = false;
    for file in &files {
        if process_calc_file(file, dry_run, &mut mutation) {
            has_emacs_error = true;
        }
    }
    mutation.finish(started, files.len());
    output_mutation(&mutation, format);

    if has_emacs_error {
        return EXIT_REQUIRES_EMACS;
    }
    EXIT_OK
}
