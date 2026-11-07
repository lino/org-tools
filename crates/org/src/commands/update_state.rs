// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org update set-state` arguments, stdin auto-detection, and state transition handler.

use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Read as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;

use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::locator::{resolve_locator, OrgLocator};
use org_tools_core::source::SourceFile;

use super::update::FileEntryTargets;
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
    ORG_CONTENT_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
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
        return Err(format!(
            "org: stdin looks like org-mode content, not locator targets\n\
             first offending line: {bad}\n\
             hint: pipe locators (e.g. from `org query search`), not org file content"
        ));
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
) -> Option<org_tools_core::state::SetStateResult> {
    let effective_indices = match query_pred {
        Some(pred) => {
            let query_matches = filter_entries_by_query(doc, pred);
            let filtered = match indices {
                Some(idx) => query_matches
                    .into_iter()
                    .filter(|i| idx.contains(i))
                    .collect::<Vec<_>>(),
                None => query_matches,
            };
            if filtered.is_empty() {
                return None;
            }
            Some(filtered)
        }
        None => indices.map(|s| s.to_vec()),
    };
    org_tools_core::state::set_state(source, doc, effective_indices.as_deref(), state, !no_closed)
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
            EXIT_OK
        }
        None => {
            eprintln!("No entries to change");
            print!("{}", source.content);
            EXIT_OK
        }
    }
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
    let first_meaningful = content.lines().find(|l| !l.trim().is_empty());
    if first_meaningful.is_some_and(looks_like_org_content) {
        let code = handle_piped_org_set_state(
            content,
            &args.state,
            query_pred,
            args.no_closed,
            args.dry_run,
        );
        return Ok(Some(code));
    }

    let lines: Vec<String> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(ToString::to_string)
        .collect();
    if let Some(bad) = lines.iter().find(|l| looks_like_org_content(l)) {
        eprintln!(
            "org: stdin looks like org-mode content, not locator targets\n\
             first offending line: {bad}\n\
             hint: pipe locators (e.g. from `org query search`), not org file content"
        );
        return Err(EXIT_ERROR);
    }
    targets.extend(lines);
    Ok(None)
}

// endregion

// region Target Resolution & File Mutation

/// Resolves `set-state` target arguments into sorted `(file, entry_indices)` pairs.
fn resolve_state_targets(targets: &[String]) -> Result<FileEntryTargets, i32> {
    let mut file_targets: HashMap<PathBuf, Option<Vec<usize>>> = HashMap::new();

    for target in targets {
        if let Ok(locator) = OrgLocator::parse(target) {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let resolved = resolve_locator(&locator, &[cwd]).map_err(|e| {
                eprintln!("org: {e}");
                EXIT_ERROR
            })?;
            if resolved.is_file_node() {
                eprintln!("org: cannot set TODO state on file-level node {target}");
                return Err(EXIT_ERROR);
            }
            let entry = file_targets
                .entry(resolved.file)
                .or_insert(Some(Vec::new()));
            if let Some(ref mut existing) = entry {
                existing.push(resolved.entry_index);
            }
        } else {
            for file in collect_org_files(&[PathBuf::from(target)]) {
                file_targets.insert(file, None);
            }
        }
    }

    let mut file_list: Vec<_> = file_targets.into_iter().collect();
    file_list.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(file_list)
}

/// Collects per-entry [`MutationDetail`] records for entries whose TODO state changed.
fn collect_changed_state_details(
    file: &Path,
    before: &OrgDocument,
    after_content: &str,
) -> Vec<MutationDetail> {
    let after_source = SourceFile::new(file, after_content.to_string());
    let after_doc = OrgDocument::from_source(&after_source);
    let file_str = file.display().to_string();

    after_doc
        .entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            let prev = before.entries.get(idx)?;
            if prev.keyword == entry.keyword {
                return None;
            }
            let from_kw = prev.keyword.as_deref().unwrap_or("<none>");
            let to_kw = entry.keyword.as_deref().unwrap_or("<none>");
            Some(MutationDetail {
                file: file_str.clone(),
                line: entry.heading_line,
                action: "state-changed".to_string(),
                detail: Some(format!("{from_kw} -> {to_kw}")),
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

/// Applies `set-state` to a single file and records results into `mutation`.
fn process_set_state_file(
    file: &Path,
    indices: Option<&[usize]>,
    query_pred: Option<&query::parser::Predicate>,
    args: &SetStateArgs,
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
    if !args.dry_run {
        if let Err(e) = std::fs::write(file, &r.content) {
            let msg = format!("error writing {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    }

    mutation.files_modified += 1;
    mutation.entries_affected += r.changed;
    mutation
        .details
        .extend(collect_changed_state_details(file, &doc, &r.content));
    if args.format == MutationOutputFormat::Human {
        emit_set_state_human(file, r.changed, &args.state, args.dry_run);
    }
}

/// Parses an optional `--query` predicate expression.
fn parse_optional_query(expr: Option<&str>) -> Result<Option<query::parser::Predicate>, i32> {
    match expr {
        Some(raw) => query::parser::parse_query(raw).map(Some).map_err(|e| {
            eprintln!("org: invalid query: {e}");
            EXIT_ERROR
        }),
        None => Ok(None),
    }
}

/// Gathers CLI and piped locator targets for `org update set-state`.
fn gather_set_state_targets(
    args: &mut SetStateArgs,
    query_pred: Option<&query::parser::Predicate>,
) -> Result<Option<Vec<String>>, i32> {
    let mut targets = std::mem::take(&mut args.targets);
    if let Some(exit_code) = read_piped_state_stdin(&mut targets, query_pred, args)? {
        if exit_code != EXIT_OK {
            return Err(exit_code);
        }
        return Ok(None);
    }
    if args.stdin {
        let stdin_targets = read_stdin_targets().map_err(|e| {
            eprintln!("{e}");
            EXIT_ERROR
        })?;
        targets.extend(stdin_targets);
    }
    if targets.is_empty() {
        eprintln!("org: no targets specified");
        return Err(EXIT_ERROR);
    }
    Ok(Some(targets))
}

// endregion

// region Entrypoint

/// Parses `--query` and resolves `set-state` file targets, handling piped Org stdin early.
fn prepare_set_state_plan(
    args: &mut SetStateArgs,
) -> Result<Option<(FileEntryTargets, Option<query::parser::Predicate>)>, i32> {
    let query_pred = parse_optional_query(args.query.as_deref())?;
    let Some(targets) = gather_set_state_targets(args, query_pred.as_ref())? else {
        return Ok(None);
    };
    let file_list = resolve_state_targets(&targets)?;
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
    for (file, indices) in &file_list {
        process_set_state_file(
            file,
            indices.as_deref(),
            query_pred.as_ref(),
            &args,
            &mut mutation,
        );
    }
    mutation.finish(started, file_list.len());

    if args.format == MutationOutputFormat::Json {
        output_mutation(&mutation, MutationOutputFormat::Json);
    } else if mutation.entries_affected == 0 && !args.dry_run {
        println!("No entries to change");
    }
    EXIT_OK
}

// endregion
