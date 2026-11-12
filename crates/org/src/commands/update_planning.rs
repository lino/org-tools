// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand handlers for `org update set-scheduled` and `org update set-deadline`
//! and reusable planning mutation primitives.

use std::io::{IsTerminal, Read as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, ValueHint};

use org_tools_core::document::OrgDocument;
use org_tools_core::gtd_roam::parse_piped_stdin_targets;
use org_tools_core::planning::{set_planning_timestamp, PlanningKind};
use org_tools_core::source::SourceFile;

use super::{
    filter_entries_by_query, invalidate_warm_workspace_caches, looks_like_org_content,
    parse_optional_query, resolve_final_batch_targets, FileEntryTargets,
};
use crate::commands::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{date, query, EXIT_ERROR, EXIT_OK};

// region CLI Arguments & Naming

/// CLI arguments shared by `org update set-scheduled` and `org update set-deadline`.
#[derive(Debug, Clone, Args)]
pub(crate) struct PlanningArgs {
    /// Planning timestamp specification (or first target when `--clear` is passed).
    #[arg(allow_hyphen_values = true)]
    pub(crate) spec_or_target: Option<String>,

    /// Target locators, files, or directories.
    pub(crate) targets: Vec<String>,

    /// Remove the planning timestamp instead of setting or shifting it.
    #[arg(long)]
    pub(crate) clear: bool,

    /// Read additional locator targets from stdin (one per line).
    #[arg(long)]
    pub(crate) stdin: bool,

    /// Filter entries by query expression (e.g., `"todo:TODO tags:work"`).
    #[arg(long)]
    pub(crate) query: Option<String>,

    /// Optional workspace directory for locator resolution and warm cache invalidation.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,

    /// Preview planning mutations without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// Returns the canonical `MutationResult.command` identifier for `kind`.
pub(crate) const fn planning_command_name(kind: PlanningKind) -> &'static str {
    match kind {
        PlanningKind::Scheduled => "set-scheduled",
        PlanningKind::Deadline => "set-deadline",
    }
}

/// Returns the `MutationDetail.action` label for `kind` and whether the timestamp was cleared.
const fn planning_action_label(kind: PlanningKind, cleared: bool) -> &'static str {
    match (kind, cleared) {
        (PlanningKind::Scheduled, false) => "set-scheduled",
        (PlanningKind::Scheduled, true) => "cleared-scheduled",
        (PlanningKind::Deadline, false) => "set-deadline",
        (PlanningKind::Deadline, true) => "cleared-deadline",
    }
}

/// Returns the human-readable verb phrase used in locator resolution diagnostics.
const fn planning_target_verb(kind: PlanningKind) -> &'static str {
    match kind {
        PlanningKind::Scheduled => "set SCHEDULED timestamp",
        PlanningKind::Deadline => "set DEADLINE timestamp",
    }
}

// endregion

// region Core & Reusable Planning Mutators

/// Validates `spec` against [`set_planning_timestamp`] prior to scanning or mutating files.
pub(crate) fn validate_planning_spec(
    kind: PlanningKind,
    spec: Option<&str>,
    today: (u16, u8, u8),
) -> Result<(), String> {
    let probe_src = SourceFile::new("<probe>", "* Probe\n".to_string());
    let probe_doc = OrgDocument::from_source(&probe_src);
    set_planning_timestamp(&probe_src, &probe_doc, 0, kind, spec, today).map(|_| ())
}

/// Formats the optional `old -> new` or single-timestamp summary for `MutationDetail`.
fn format_planning_detail(old_ts: Option<&str>, new_ts: Option<&str>) -> Option<String> {
    match (old_ts, new_ts) {
        (Some(o), Some(n)) => Some(format!("{o} -> {n}")),
        (None, Some(n)) => Some(n.to_string()),
        (Some(o), None) => Some(o.to_string()),
        (None, None) => None,
    }
}

/// Resolves sorted, deduplicated entry indices in `doc` matching `indices` and `query_pred`.
fn effective_entry_indices(
    doc: &OrgDocument,
    indices: Option<&[usize]>,
    query_pred: Option<&query::parser::Predicate>,
) -> Vec<usize> {
    let mut base: Vec<usize> = match indices {
        Some(raw) => raw.to_vec(),
        None => (0..doc.entries.len()).collect(),
    };
    if let Some(pred) = query_pred {
        let matched = filter_entries_by_query(doc, pred);
        base.retain(|idx| matched.contains(idx));
    }
    base.sort_unstable();
    base.dedup();
    base
}

/// Mutates matching entries in `source` in-memory and returns `(final_content, details)`.
fn mutate_source_planning(
    file_label: &str,
    source: &SourceFile,
    indices: Option<&[usize]>,
    query_pred: Option<&query::parser::Predicate>,
    kind: PlanningKind,
    spec: Option<&str>,
    today: (u16, u8, u8),
) -> Result<(String, Vec<MutationDetail>), String> {
    let mut cur_src = SourceFile::new(source.path.clone(), source.content.clone());
    let mut cur_doc = OrgDocument::from_source(&cur_src);
    let targets = effective_entry_indices(&cur_doc, indices, query_pred);
    let mut details = Vec::new();

    for idx in targets {
        let res = set_planning_timestamp(&cur_src, &cur_doc, idx, kind, spec, today)?;
        if res.changed {
            let cleared = res.new_timestamp.is_none();
            let detail =
                format_planning_detail(res.old_timestamp.as_deref(), res.new_timestamp.as_deref());
            details.push(MutationDetail {
                file: file_label.to_string(),
                line: res.line,
                action: planning_action_label(kind, cleared).to_string(),
                detail,
            });
            cur_src = SourceFile::new(cur_src.path.clone(), res.content);
            cur_doc = OrgDocument::from_source(&cur_src);
        }
    }
    Ok((cur_src.content, details))
}

/// Computes planning mutations for `file` in memory without touching disk.
fn plan_file_planning(
    file: &Path,
    indices: Option<&[usize]>,
    query_pred: Option<&query::parser::Predicate>,
    kind: PlanningKind,
    spec: Option<&str>,
    today: (u16, u8, u8),
) -> Result<(String, Vec<MutationDetail>), String> {
    let source = SourceFile::from_path(file)
        .map_err(|e| format!("error reading {}: {e}", file.display()))?;
    let file_label = file.display().to_string();
    mutate_source_planning(&file_label, &source, indices, query_pred, kind, spec, today)
}

/// Computes in-memory planning updates across `file_list` prior to atomic disk commits.
#[allow(clippy::type_complexity)]
fn plan_batch_planning(
    file_list: &FileEntryTargets,
    query_pred: Option<&query::parser::Predicate>,
    kind: PlanningKind,
    spec: Option<&str>,
    today: (u16, u8, u8),
    dry_run: bool,
) -> Result<(MutationResult, Vec<(PathBuf, String)>), String> {
    validate_planning_spec(kind, spec, today)?;
    let mut mutation = MutationResult::new(planning_command_name(kind), dry_run);
    let mut pending_writes = Vec::new();

    for (file, indices) in file_list {
        match plan_file_planning(file, indices.as_deref(), query_pred, kind, spec, today) {
            Ok((next_content, details)) if !details.is_empty() => {
                mutation.entries_affected += details.len();
                mutation.details.extend(details);
                pending_writes.push((file.clone(), next_content));
            }
            Ok(_) => {}
            Err(msg) => {
                eprintln!("org: {msg}");
                mutation.errors.push(msg);
            }
        }
    }
    Ok((mutation, pending_writes))
}

/// Writes `pending_writes` atomically (unless `dry_run`) and returns canonicalized modified paths.
fn commit_planning_writes(
    mutation: &mut MutationResult,
    pending_writes: Vec<(PathBuf, String)>,
    dry_run: bool,
) -> Vec<PathBuf> {
    let mut modified_paths = Vec::with_capacity(pending_writes.len());
    for (file, next_content) in pending_writes {
        if !dry_run {
            if let Err(e) = org_tools_core::files::write_file_atomic(&file, &next_content) {
                let msg = format!("error writing {}: {e}", file.display());
                eprintln!("org: {msg}");
                mutation.errors.push(msg);
                continue;
            }
        }
        modified_paths.push(org_tools_core::files::canonicalize_clean(&file).unwrap_or(file));
    }
    mutation.files_modified = modified_paths.len();
    modified_paths
}

/// Applies a validated planning mutation across `file_list` and returns `(MutationResult, modified_paths)`.
pub(crate) fn apply_planning_targets(
    file_list: &FileEntryTargets,
    query_pred: Option<&query::parser::Predicate>,
    kind: PlanningKind,
    spec: Option<&str>,
    today: (u16, u8, u8),
    dry_run: bool,
    cache_ws: Option<Option<&Path>>,
) -> Result<(MutationResult, Vec<PathBuf>), String> {
    let started = Instant::now();
    let (mut mutation, pending_writes) =
        plan_batch_planning(file_list, query_pred, kind, spec, today, dry_run)?;
    if let Some(ws) = cache_ws {
        if !dry_run && !pending_writes.is_empty() {
            let pre_paths: Vec<PathBuf> = pending_writes.iter().map(|(p, _)| p.clone()).collect();
            invalidate_warm_workspace_caches(ws, &pre_paths);
        }
    }
    let modified_paths = commit_planning_writes(&mut mutation, pending_writes, dry_run);
    mutation.finish(started, file_list.len());
    Ok((mutation, modified_paths))
}

// endregion

// region Stdin & Plan Preparation

/// Separates `args` into `(optional_spec, raw_targets)` while honoring `--clear`.
fn unpack_spec_and_targets(args: &mut PlanningArgs) -> Result<(Option<String>, Vec<String>), i32> {
    let mut targets = std::mem::take(&mut args.targets);
    if args.clear {
        if let Some(first_target) = args.spec_or_target.take() {
            targets.insert(0, first_target);
        }
        return Ok((None, targets));
    }
    let Some(spec) = args.spec_or_target.take() else {
        eprintln!("org: missing planning timestamp specification (or pass --clear)");
        return Err(EXIT_ERROR);
    };
    Ok((Some(spec), targets))
}

/// Applies a planning mutation to piped `<stdin>` Org content and prints the resulting buffer.
fn handle_piped_org_planning(
    content: String,
    kind: PlanningKind,
    spec: Option<&str>,
    query_pred: Option<&query::parser::Predicate>,
    today: (u16, u8, u8),
    dry_run: bool,
) -> Result<(), i32> {
    let source = SourceFile::new("<stdin>", content);
    match mutate_source_planning("<stdin>", &source, None, query_pred, kind, spec, today) {
        Ok((next_content, details)) => {
            if dry_run || details.is_empty() {
                print!("{}", source.content);
            } else {
                print!("{next_content}");
            }
            Ok(())
        }
        Err(err) => {
            eprintln!("org: {err}");
            Err(EXIT_ERROR)
        }
    }
}

/// Reads piped stdin when neither CLI targets nor `--stdin` were supplied; returns `Ok(true)` if handled.
fn read_piped_planning_stdin(
    targets: &mut Vec<String>,
    kind: PlanningKind,
    spec: Option<&str>,
    query_pred: Option<&query::parser::Predicate>,
    today: (u16, u8, u8),
    args: &PlanningArgs,
) -> Result<bool, i32> {
    if std::io::stdin().is_terminal() || !targets.is_empty() || args.stdin {
        return Ok(false);
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
        handle_piped_org_planning(content, kind, spec, query_pred, today, args.dry_run)?;
        return Ok(true);
    }
    let parsed = parse_piped_stdin_targets(&content).map_err(|e| {
        eprintln!("{e}");
        EXIT_ERROR
    })?;
    targets.extend(parsed);
    Ok(false)
}

/// Resolves spec, `--query`, and CLI/stdin targets into an executable batch plan.
#[allow(clippy::type_complexity)]
fn prepare_planning_plan(
    kind: PlanningKind,
    args: &mut PlanningArgs,
    today: (u16, u8, u8),
) -> Result<
    Option<(
        Option<String>,
        FileEntryTargets,
        Option<query::parser::Predicate>,
    )>,
    i32,
> {
    let (spec, mut targets) = unpack_spec_and_targets(args)?;
    validate_planning_spec(kind, spec.as_deref(), today).map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;
    let query_pred = parse_optional_query(args.query.as_deref())?;
    if read_piped_planning_stdin(
        &mut targets,
        kind,
        spec.as_deref(),
        query_pred.as_ref(),
        today,
        args,
    )? {
        return Ok(None);
    }
    let file_list = resolve_final_batch_targets(
        &mut targets,
        args.stdin,
        args.workspace.as_deref(),
        planning_target_verb(kind),
    )?;
    Ok(Some((spec, file_list, query_pred)))
}

// endregion

// region Subcommand Entrypoints

/// Executes `org update set-scheduled` or `org update set-deadline`.
fn run_planning_command(kind: PlanningKind, mut args: PlanningArgs) -> i32 {
    let today = date::current_date();
    let (spec, file_list, query_pred) = match prepare_planning_plan(kind, &mut args, today) {
        Ok(Some(plan)) => plan,
        Ok(None) => return EXIT_OK,
        Err(code) => return code,
    };
    let (mutation, _) = match apply_planning_targets(
        &file_list,
        query_pred.as_ref(),
        kind,
        spec.as_deref(),
        today,
        args.dry_run,
        Some(args.workspace.as_deref()),
    ) {
        Ok(res) => res,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    output_mutation(&mutation, args.format);
    if !mutation.errors.is_empty() {
        return EXIT_ERROR;
    }
    EXIT_OK
}

/// Runs `org update set-scheduled`.
pub(crate) fn run_set_scheduled(args: PlanningArgs) -> i32 {
    run_planning_command(PlanningKind::Scheduled, args)
}

/// Runs `org update set-deadline`.
pub(crate) fn run_set_deadline(args: PlanningArgs) -> i32 {
    run_planning_command(PlanningKind::Deadline, args)
}

// endregion
