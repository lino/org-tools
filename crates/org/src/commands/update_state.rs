// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand handler for `org update set-state`, stdin auto-detection, repeater
//! auto-advance, and `:TRIGGER:` cascade execution.

use std::collections::HashSet;
use std::io::{IsTerminal, Read as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;

use org_tools_core::document::OrgDocument;
use org_tools_core::edna::run_file_trigger_cascade;
use org_tools_core::source::SourceFile;
use org_tools_core::state::{set_state_with_options, SetStateOptions, SetStateResult};

use super::update::{
    filter_entries_by_query, format_stdin_content_error, invalidate_warm_mabuse_caches,
    looks_like_org_content, parse_optional_query, resolve_final_batch_targets, FileEntryTargets,
};
use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{query, EXIT_ERROR, EXIT_OK};

// region Constants & Types

/// CLI arguments for `org update set-state`.
#[derive(Args)]
pub(crate) struct SetStateArgs {
    /// New TODO keyword (e.g., DONE, TODO, NEXT).
    pub(crate) state: String,
    /// Targets: file paths, directory paths, or org locator strings.
    pub(crate) targets: Vec<String>,
    /// Read additional locator targets from stdin.
    #[arg(long)]
    pub(crate) stdin: bool,
    /// Filter entries by query expression (e.g., "todo:TODO tags:work").
    #[arg(long)]
    pub(crate) query: Option<String>,
    /// Optional workspace directory for cross-file org-edna :TRIGGER: resolution.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
    /// Don't add/remove CLOSED: timestamp on state transitions.
    #[arg(long)]
    pub(crate) no_closed: bool,
    /// Disable repeater auto-advance and close repeating entries permanently.
    #[arg(long)]
    pub(crate) no_repeat: bool,
    /// Print what would be changed without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

// endregion

// region Stdin & Query Helpers

/// Applies [`set_state_with_options`] to `source`/`doc`, intersecting `indices` with `query_pred`.
fn apply_set_state(
    source: &SourceFile,
    doc: &OrgDocument,
    indices: Option<&[usize]>,
    query_pred: Option<&query::parser::Predicate>,
    state: &str,
    no_closed: bool,
    no_repeat: bool,
) -> Option<SetStateResult> {
    let effective = match query_pred {
        Some(pred) => {
            let matches = filter_entries_by_query(doc, pred);
            let filtered: Vec<usize> = match indices {
                Some(idx) => matches.into_iter().filter(|i| idx.contains(i)).collect(),
                None => matches,
            };
            if filtered.is_empty() {
                return None;
            }
            Some(filtered)
        }
        None => indices.map(|s| s.to_vec()),
    };
    let options = SetStateOptions {
        add_closed: !no_closed,
        advance_repeaters: !no_repeat,
        today: None,
    };
    set_state_with_options(source, doc, effective.as_deref(), state, options)
}

/// Processes piped Org-mode file content for `org update set-state`.
fn handle_piped_org_set_state(
    content: String,
    query_pred: Option<&query::parser::Predicate>,
    args: &SetStateArgs,
) -> i32 {
    let source = SourceFile::new("<stdin>", content);
    let doc = OrgDocument::from_source(&source);
    if !doc.todo_keywords.contains(&args.state) {
        eprintln!(
            "org: '{}' is not a recognized TODO keyword in <stdin>",
            args.state
        );
        return EXIT_ERROR;
    }
    match apply_set_state(
        &source,
        &doc,
        None,
        query_pred,
        &args.state,
        args.no_closed,
        args.no_repeat,
    ) {
        Some(r) => {
            let suffix = if r.changed == 1 { "y" } else { "ies" };
            if args.dry_run {
                eprintln!("Would change {} entr{suffix} in <stdin>", r.changed);
                print!("{}", source.content);
            } else {
                eprintln!("Changed {} entr{suffix} to {}", r.changed, args.state);
                print!("{}", r.content);
            }
        }
        None => {
            eprintln!("No entries to change");
            print!("{}", source.content);
        }
    }
    EXIT_OK
}

/// Reads piped stdin when no CLI targets or `--stdin` flag were supplied.
fn read_piped_state_stdin(
    targets: &mut Vec<String>,
    query_pred: Option<&query::parser::Predicate>,
    args: &SetStateArgs,
) -> Result<Option<i32>, i32> {
    if std::io::stdin().is_terminal() || !targets.is_empty() || args.stdin {
        return Ok(None);
    }
    let mut content = String::new();
    if let Err(e) = std::io::stdin().lock().read_to_string(&mut content) {
        eprintln!("org: error reading stdin: {e}");
        return Err(EXIT_ERROR);
    }
    if content
        .lines()
        .find(|l| !l.trim().is_empty())
        .is_some_and(looks_like_org_content)
    {
        return Ok(Some(handle_piped_org_set_state(content, query_pred, args)));
    }
    let lines: Vec<String> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(ToString::to_string)
        .collect();
    if let Some(bad) = lines.iter().find(|l| looks_like_org_content(l)) {
        eprintln!("{}", format_stdin_content_error(bad));
        return Err(EXIT_ERROR);
    }
    targets.extend(lines);
    Ok(None)
}

// endregion

// region Target Resolution & File Mutation

/// Collects per-entry [`MutationDetail`] records (`repeated` vs `state-changed`) from `r`.
fn collect_changed_state_details(
    file: &Path,
    before: &OrgDocument,
    r: &SetStateResult,
) -> Vec<MutationDetail> {
    let after_doc = OrgDocument::from_source(&SourceFile::new(file, r.content.clone()));
    let file_str = file.display().to_string();
    r.changed_indices
        .iter()
        .filter_map(|&idx| {
            let prev = before.entries.get(idx)?;
            let entry = after_doc.entries.get(idx)?;
            let action = if r.repeated_indices.contains(&idx) {
                "repeated"
            } else {
                "state-changed"
            };
            let prev_kw = prev.keyword.as_deref().unwrap_or("<none>");
            let next_kw = entry.keyword.as_deref().unwrap_or("<none>");
            Some(MutationDetail {
                file: file_str.clone(),
                line: entry.heading_line,
                action: action.to_string(),
                detail: Some(format!("{prev_kw} -> {next_kw}")),
            })
        })
        .collect()
}

/// Prints the human-mode per-file summary for `org update set-state`.
fn emit_set_state_human(file: &Path, changed: usize, state: &str, dry_run: bool) {
    let suffix = if changed == 1 { "y" } else { "ies" };
    if dry_run {
        println!("Would change {changed} entr{suffix} in {}", file.display());
    } else {
        println!(
            "Changed {changed} entr{suffix} to {state} in {}",
            file.display()
        );
    }
}

/// Writes a single-file state update (when no `:TRIGGER:` fires) and records details.
fn commit_single_file_state(
    file: &Path,
    doc: &OrgDocument,
    r: &SetStateResult,
    workspace: Option<&Path>,
    dry_run: bool,
    modified_paths: &mut HashSet<PathBuf>,
    mutation: &mut MutationResult,
) -> bool {
    let canon = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    if !dry_run {
        invalidate_warm_mabuse_caches(workspace, std::slice::from_ref(&canon));
        if let Err(e) = org_tools_core::files::write_file_atomic(file, &r.content) {
            let msg = format!("error writing {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return false;
        }
    }
    modified_paths.insert(canon);
    mutation.entries_affected += r.changed;
    mutation
        .details
        .extend(collect_changed_state_details(file, doc, r));
    true
}

/// Commits either a `:TRIGGER:` cascade or a single-file write for `r`.
fn commit_state_change(
    file: &Path,
    doc: &OrgDocument,
    r: SetStateResult,
    args: &SetStateArgs,
    modified_paths: &mut HashSet<PathBuf>,
    mutation: &mut MutationResult,
) -> bool {
    let ws = args.workspace.as_deref();
    let has_triggers = r
        .newly_done_indices
        .iter()
        .any(|&idx| doc.property(idx, "TRIGGER").is_some());
    if !has_triggers {
        return commit_single_file_state(file, doc, &r, ws, args.dry_run, modified_paths, mutation);
    }
    match run_file_trigger_cascade(file, doc, &r, ws, !args.no_closed, args.dry_run) {
        Ok((paths, cascade)) => {
            if !args.dry_run && !paths.is_empty() {
                invalidate_warm_mabuse_caches(ws, &paths);
            }
            modified_paths.extend(paths);
            mutation.entries_affected += r.changed + cascade.entries_affected;
            mutation
                .details
                .extend(collect_changed_state_details(file, doc, &r));
            mutation
                .details
                .extend(cascade.mutations.into_iter().map(|m| MutationDetail {
                    file: m.file.display().to_string(),
                    line: m.line,
                    action: m.action,
                    detail: m.detail,
                }));
            true
        }
        Err(msg) => {
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            false
        }
    }
}

/// Applies `set-state` (and optional `:TRIGGER:` cascades) to `file`.
fn process_set_state_file(
    file: &Path,
    indices: Option<&[usize]>,
    query_pred: Option<&query::parser::Predicate>,
    args: &SetStateArgs,
    modified_paths: &mut HashSet<PathBuf>,
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
    if !doc.todo_keywords.contains(&args.state) {
        let msg = format!(
            "'{}' is not a recognized TODO keyword in {}",
            args.state,
            file.display()
        );
        eprintln!("org: {msg}");
        mutation.errors.push(msg);
        return;
    }

    let Some(r) = apply_set_state(
        &source,
        &doc,
        indices,
        query_pred,
        &args.state,
        args.no_closed,
        args.no_repeat,
    ) else {
        return;
    };
    let changed = r.changed;
    if commit_state_change(file, &doc, r, args, modified_paths, mutation)
        && args.format == MutationOutputFormat::Human
    {
        emit_set_state_human(file, changed, &args.state, args.dry_run);
    }
}

// endregion

// region Entrypoint

/// Parses `--query` and gathers CLI/stdin targets for `org update set-state`.
fn prepare_set_state_plan(
    args: &mut SetStateArgs,
) -> Result<Option<(FileEntryTargets, Option<query::parser::Predicate>)>, i32> {
    let query_pred = parse_optional_query(args.query.as_deref())?;
    let mut targets = std::mem::take(&mut args.targets);
    if let Some(exit_code) = read_piped_state_stdin(&mut targets, query_pred.as_ref(), args)? {
        return if exit_code == EXIT_OK {
            Ok(None)
        } else {
            Err(exit_code)
        };
    }
    let file_list = resolve_final_batch_targets(
        &mut targets,
        args.stdin,
        args.workspace.as_deref(),
        "set TODO state",
    )?;
    Ok(Some((file_list, query_pred)))
}

/// Runs `org update set-state`.
pub(crate) fn run_set_state(mut args: SetStateArgs) -> i32 {
    let started = Instant::now();
    let (file_list, query_pred) = match prepare_set_state_plan(&mut args) {
        Ok(Some(plan)) => plan,
        Ok(None) => return EXIT_OK,
        Err(code) => return code,
    };

    let mut mutation = MutationResult::new("set-state", args.dry_run);
    let mut modified_paths: HashSet<PathBuf> = HashSet::new();
    for (file, indices) in &file_list {
        process_set_state_file(
            file,
            indices.as_deref(),
            query_pred.as_ref(),
            &args,
            &mut modified_paths,
            &mut mutation,
        );
    }
    mutation.files_modified = modified_paths.len();
    mutation.finish(started, file_list.len());

    if args.format == MutationOutputFormat::Json {
        output_mutation(&mutation, MutationOutputFormat::Json);
    } else if mutation.entries_affected == 0 && !args.dry_run && mutation.errors.is_empty() {
        println!("No entries to change");
    }
    if !mutation.errors.is_empty() {
        return EXIT_ERROR;
    }
    EXIT_OK
}

// endregion
