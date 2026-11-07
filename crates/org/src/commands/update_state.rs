// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org update set-state` arguments, stdin auto-detection, and `:TRIGGER:` cascade handler.

use std::collections::HashSet;
use std::io::{BufRead, IsTerminal, Read as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;

use org_tools_core::document::OrgDocument;
use org_tools_core::edna::run_file_trigger_cascade;
use org_tools_core::source::SourceFile;
use org_tools_core::state::SetStateResult;

use super::update::{resolve_batch_entry_targets, FileEntryTargets};
use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{date, query, EXIT_ERROR, EXIT_OK};

// region Constants & Types

/// Leading prefixes identifying Org-mode document lines rather than locator strings.
const ORG_CONTENT_PREFIXES: &[&str] = &[
    "* ",
    "#+",
    "- ",
    "| ",
    ": ",
    "DEADLINE:",
    "SCHEDULED:",
    "CLOSED:",
    ":PROPERTIES:",
    ":END:",
];

/// Error message emitted when raw Org content is piped where locators were expected.
const STDIN_ORG_CONTENT_HINT: &str =
    "hint: pipe locators (e.g. from `org query search`), not org file content";

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
    /// Print what would be changed without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

// endregion

// region Stdin & Query Helpers

/// Checks whether `line` looks like Org-mode content rather than a locator or file path.
fn looks_like_org_content(line: &str) -> bool {
    let trimmed = line.trim();
    ORG_CONTENT_PREFIXES.iter().any(|p| trimmed.starts_with(p))
}

/// Formats the standard diagnostic when stdin contains Org lines instead of locators.
fn format_stdin_content_error(bad: &str) -> String {
    format!(
        "org: stdin looks like org-mode content, not locator targets\nfirst offending line: {bad}\n{STDIN_ORG_CONTENT_HINT}"
    )
}

/// Reads locator strings from stdin (one per line), rejecting raw Org-mode content.
fn read_stdin_targets() -> Result<Vec<String>, String> {
    let lines: Vec<String> = std::io::stdin()
        .lock()
        .lines()
        .map_while(|l| l.ok())
        .filter(|l| !l.trim().is_empty())
        .collect();
    if let Some(bad) = lines.iter().find(|l| looks_like_org_content(l)) {
        return Err(format_stdin_content_error(bad));
    }
    Ok(lines)
}

/// Filters entry indices in `doc` matching `pred`.
fn filter_entries_by_query(doc: &OrgDocument, pred: &query::parser::Predicate) -> Vec<usize> {
    let today = date::current_date();
    let doc_refs = [doc];
    doc.entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query::predicate::matches(pred, entry, doc, &doc_refs, today))
        .map(|(idx, _)| idx)
        .collect()
}

/// Applies `set_state` to `source`/`doc`, intersecting `indices` with `query_pred` when set.
fn apply_set_state(
    source: &SourceFile,
    doc: &OrgDocument,
    indices: Option<&[usize]>,
    query_pred: Option<&query::parser::Predicate>,
    state: &str,
    no_closed: bool,
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
    org_tools_core::state::set_state(source, doc, effective.as_deref(), state, !no_closed)
}

/// Processes piped Org-mode file content for `org update set-state`.
fn handle_piped_org_set_state(
    content: String,
    state: &str,
    query_pred: Option<&query::parser::Predicate>,
    no_closed: bool,
    dry_run: bool,
) -> i32 {
    let source = SourceFile::new("<stdin>", content);
    let doc = OrgDocument::from_source(&source);
    if !doc.todo_keywords.contains(state) {
        eprintln!("org: '{state}' is not a recognized TODO keyword in <stdin>");
        return EXIT_ERROR;
    }
    match apply_set_state(&source, &doc, None, query_pred, state, no_closed) {
        Some(r) => {
            let suffix = if r.changed == 1 { "y" } else { "ies" };
            if dry_run {
                eprintln!("Would change {} entr{suffix} in <stdin>", r.changed);
                print!("{}", source.content);
            } else {
                eprintln!("Changed {} entr{suffix} to {state}", r.changed);
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
        return Ok(Some(handle_piped_org_set_state(
            content,
            &args.state,
            query_pred,
            args.no_closed,
            args.dry_run,
        )));
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

/// Collects per-entry [`MutationDetail`] records for entries whose TODO state changed.
fn collect_changed_state_details(
    file: &Path,
    before: &OrgDocument,
    after_content: &str,
) -> Vec<MutationDetail> {
    let after_doc = OrgDocument::from_source(&SourceFile::new(file, after_content.to_string()));
    let file_str = file.display().to_string();
    after_doc
        .entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            let prev = before.entries.get(idx)?;
            (prev.keyword != entry.keyword).then(|| MutationDetail {
                file: file_str.clone(),
                line: entry.heading_line,
                action: "state-changed".to_string(),
                detail: Some(format!(
                    "{} -> {}",
                    prev.keyword.as_deref().unwrap_or("<none>"),
                    entry.keyword.as_deref().unwrap_or("<none>")
                )),
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
    dry_run: bool,
    modified_paths: &mut HashSet<PathBuf>,
    mutation: &mut MutationResult,
) -> bool {
    if !dry_run {
        if let Err(e) = std::fs::write(file, &r.content) {
            let msg = format!("error writing {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return false;
        }
    }
    modified_paths.insert(file.canonicalize().unwrap_or_else(|_| file.to_path_buf()));
    mutation.entries_affected += r.changed;
    mutation
        .details
        .extend(collect_changed_state_details(file, doc, &r.content));
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
    let has_triggers = r
        .newly_done_indices
        .iter()
        .any(|&idx| doc.property(idx, "TRIGGER").is_some());
    if !has_triggers {
        return commit_single_file_state(file, doc, &r, args.dry_run, modified_paths, mutation);
    }
    let ws = args.workspace.as_deref();
    match run_file_trigger_cascade(file, doc, &r, ws, !args.no_closed, args.dry_run) {
        Ok((paths, cascade)) => {
            modified_paths.extend(paths);
            mutation.entries_affected += r.changed + cascade.entries_affected;
            mutation
                .details
                .extend(collect_changed_state_details(file, doc, &r.content));
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
    let query_pred = match args.query.as_deref() {
        Some(raw) => Some(query::parser::parse_query(raw).map_err(|e| {
            eprintln!("org: invalid query: {e}");
            EXIT_ERROR
        })?),
        None => None,
    };
    let mut targets = std::mem::take(&mut args.targets);
    if let Some(exit_code) = read_piped_state_stdin(&mut targets, query_pred.as_ref(), args)? {
        return if exit_code == EXIT_OK {
            Ok(None)
        } else {
            Err(exit_code)
        };
    }
    if args.stdin {
        targets.extend(read_stdin_targets().map_err(|e| {
            eprintln!("{e}");
            EXIT_ERROR
        })?);
    }
    if targets.is_empty() {
        eprintln!("org: no targets specified");
        return Err(EXIT_ERROR);
    }
    let file_list =
        resolve_batch_entry_targets(&targets, args.workspace.as_deref(), "set TODO state")?;
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
    } else if mutation.entries_affected == 0 && !args.dry_run {
        println!("No entries to change");
    }
    EXIT_OK
}

// endregion
