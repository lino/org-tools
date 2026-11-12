// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand entry module for `org update`, shared locator resolution, stdin helpers,
//! warm `WorkspaceCache` invalidation, and dispatch.

#[path = "update_dep.rs"]
mod update_dep;
#[path = "update_planning.rs"]
pub(crate) mod update_planning;

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueHint};

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::export_md::{discover_workspace_root, normalize_workspace_path};
use org_tools_core::files::collect_org_files;
use org_tools_core::gtd_roam::parse_piped_stdin_targets;
use org_tools_core::locator::{resolve_locator, OrgLocator, ResolvedEntry};
use org_tools_core::source::SourceFile;

use self::update_dep::{run_dep, DepCommand};
use self::update_planning::{run_set_deadline, run_set_scheduled, PlanningArgs};
use super::update_id::{run_add_id, AddIdArgs};
use super::update_property::{
    run_delete_property, run_set_property, run_set_tags, DeletePropertyArgs, SetPropertyArgs,
    SetTagsArgs,
};
use super::update_state::{run_set_state, SetStateArgs};
use super::update_table::{run_add_cookie, run_calc};
use super::update_todo::{run_add_todo, AddTodoArgs};
use super::MutationOutputFormat;
use crate::{date, query, EXIT_ERROR};

// region Constants & Subcommand Types

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

/// Sorted `(file_path, optional_entry_indices)` targets for batch mutation commands.
pub(crate) type FileEntryTargets = Vec<(PathBuf, Option<Vec<usize>>)>;

/// Subcommands for `org update`.
#[derive(Subcommand)]
pub(crate) enum UpdateCommand {
    /// Add :ID: properties to entries that lack them.
    AddId(Box<AddIdArgs>),
    /// Change the TODO state of entries.
    SetState(Box<SetStateArgs>),
    /// Set, update, shift, or clear SCHEDULED: planning timestamps.
    SetScheduled(Box<PlanningArgs>),
    /// Set, update, shift, or clear DEADLINE: planning timestamps.
    SetDeadline(Box<PlanningArgs>),
    /// Add a new TODO entry.
    AddTodo(Box<AddTodoArgs>),
    /// Set or insert a property in an entry's :PROPERTIES: drawer.
    SetProperty(SetPropertyArgs),
    /// Delete a property from an entry's :PROPERTIES: drawer.
    DeleteProperty(DeletePropertyArgs),
    /// Set or clear heading tags aligned to column 77.
    SetTags(SetTagsArgs),
    /// Wire or remove symmetric org-edna :BLOCKER: and :TRIGGER: dependencies.
    Dep {
        /// Dependency mutation subcommand (`add` or `remove`).
        #[command(subcommand)]
        command: DepCommand,
    },
    /// Update statistic cookies ([/] and [%]) on headings.
    AddCookie {
        /// Files or directories to scan.
        #[arg(default_value = ".", value_hint = ValueHint::AnyPath)]
        paths: Vec<PathBuf>,

        /// Insert cookies on headings that don't have one yet.
        #[arg(long, short)]
        recursive: bool,

        /// Print what would be changed without writing files.
        #[arg(long)]
        dry_run: bool,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: MutationOutputFormat,
    },
    /// Evaluate #+TBLFM: table formulas and fill in results.
    Calc {
        /// Files or directories to scan.
        #[arg(default_value = ".", value_hint = ValueHint::AnyPath)]
        paths: Vec<PathBuf>,

        /// Print what would be changed without writing files.
        #[arg(long)]
        dry_run: bool,

        /// Recalculate even if cells already have values.
        #[arg(long)]
        force: bool,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: MutationOutputFormat,
    },
}

// endregion

// region Stdin, Query & Cache Helpers

/// Checks whether `line` looks like Org-mode content rather than a locator or file path.
pub(crate) fn looks_like_org_content(line: &str) -> bool {
    let trimmed = line.trim();
    ORG_CONTENT_PREFIXES.iter().any(|p| trimmed.starts_with(p))
}

/// Reads locator strings from stdin (`human`, `json`, or `locator` format), rejecting raw Org-mode content.
pub(crate) fn read_stdin_targets() -> Result<Vec<String>, String> {
    let mut raw = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut raw)
        .map_err(|e| format!("org: error reading stdin: {e}"))?;
    parse_piped_stdin_targets(&raw)
}

/// Parses an optional CLI `--query` expression, reporting diagnostics on invalid syntax.
pub(crate) fn parse_optional_query(
    raw: Option<&str>,
) -> Result<Option<query::parser::Predicate>, i32> {
    match raw {
        Some(expr) => query::parser::parse_query(expr).map(Some).map_err(|e| {
            eprintln!("org: invalid query: {e}");
            EXIT_ERROR
        }),
        None => Ok(None),
    }
}

/// Filters entry indices in `doc` matching `pred`.
pub(crate) fn filter_entries_by_query(
    doc: &OrgDocument,
    pred: &query::parser::Predicate,
) -> Vec<usize> {
    let today = date::current_date();
    let doc_refs = [doc];
    doc.entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| query::predicate::matches(pred, entry, doc, &doc_refs, today))
        .map(|(idx, _)| idx)
        .collect()
}

/// Finds the nearest non-temp ancestor of `file` containing `.cache/org-cache.db`.
fn find_ancestor_with_cache(file: &Path) -> Option<PathBuf> {
    let temp_root = std::env::temp_dir();
    let temp_norm = normalize_workspace_path(&temp_root);
    let norm_file = normalize_workspace_path(file);
    for ancestor in norm_file.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() || ancestor == temp_root || ancestor == temp_norm {
            break;
        }
        if WorkspaceCache::resolve_db_path(ancestor).is_file() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

/// Invalidates `modified_paths` in any existing `.cache/org-cache.db` without auto-creating cold caches.
pub(crate) fn invalidate_warm_workspace_caches(
    explicit_ws: Option<&Path>,
    modified_paths: &[PathBuf],
) {
    if modified_paths.is_empty() {
        return;
    }
    let mut candidate_roots: HashSet<PathBuf> = HashSet::new();
    if let Some(ws) = explicit_ws {
        candidate_roots.insert(normalize_workspace_path(ws));
    }
    for file in modified_paths {
        candidate_roots.insert(discover_workspace_root(explicit_ws, file));
        if let Some(found) = find_ancestor_with_cache(file) {
            candidate_roots.insert(found);
        }
    }
    for ws_root in candidate_roots {
        if WorkspaceCache::resolve_db_path(&ws_root).is_file() {
            let _ = WorkspaceCache::sync_written_files(&ws_root, modified_paths);
            if let Ok((mut cache, _)) = WorkspaceCache::open_and_sync(&ws_root) {
                let _ = cache.invalidate_paths(modified_paths);
            }
        }
    }
}

// endregion

// region Target & Locator Resolution

/// Checks whether `file_part::search` uses `<file>::<Heading>` outline shorthand.
fn is_heading_shorthand(file_part: &str, search: &str) -> bool {
    !file_part.is_empty()
        && !search.is_empty()
        && !search.starts_with('#')
        && !search.starts_with("*/")
        && search.parse::<usize>().is_err()
}

/// Parses `target` as an [`OrgLocator`] (supporting `<file>::<Heading>` outline shorthand), or `None` for plain paths.
fn parse_target_locator(target: &str) -> Option<Result<OrgLocator, String>> {
    let trimmed = target.trim();
    if !trimmed.starts_with("id:") && !trimmed.contains("::") {
        return None;
    }
    if let Ok(locator) = OrgLocator::parse(trimmed) {
        return Some(Ok(locator));
    }
    if let Some((file_part, rest)) = trimmed.split_once("::") {
        let search = rest.trim();
        if is_heading_shorthand(file_part, search) {
            let rewritten = format!("{file_part}::*/{search}");
            if let Ok(locator) = OrgLocator::parse(&rewritten) {
                return Some(Ok(locator));
            }
        }
    }
    Some(OrgLocator::parse(trimmed).map_err(|e| e.to_string()))
}

/// Inserts a resolved `entry_index` into `file_targets`, keeping `None` when the entire file is already selected.
fn record_resolved_entry_target(
    file_targets: &mut HashMap<PathBuf, Option<Vec<usize>>>,
    file: PathBuf,
    entry_index: usize,
) {
    let slot = file_targets.entry(file).or_insert_with(|| Some(Vec::new()));
    if let Some(indices) = slot {
        if !indices.contains(&entry_index) {
            indices.push(entry_index);
        }
    }
}

/// Resolves mixed CLI locator and file/directory arguments into sorted [`FileEntryTargets`].
pub(crate) fn resolve_batch_entry_targets(
    targets: &[String],
    workspace: Option<&Path>,
    action_desc: &str,
) -> Result<FileEntryTargets, i32> {
    let mut file_targets: HashMap<PathBuf, Option<Vec<usize>>> = HashMap::new();
    let mut search_paths: Vec<PathBuf> = workspace.into_iter().map(Path::to_path_buf).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    search_paths.push(cwd.clone());
    if let Ok(cfg) = Config::load_for_dir(workspace.unwrap_or(&cwd)) {
        for (_, vp) in cfg.roam_directories() {
            if vp.exists() && !search_paths.contains(&vp) {
                search_paths.push(vp);
            }
        }
    }

    for target in targets {
        if let Some(loc_res) = parse_target_locator(target) {
            let locator = loc_res.map_err(|e| {
                eprintln!("org: {e}");
                EXIT_ERROR
            })?;
            let resolved = resolve_locator(&locator, &search_paths).map_err(|e| {
                eprintln!("org: {e}");
                EXIT_ERROR
            })?;
            if resolved.is_file_node() {
                eprintln!("org: cannot {action_desc} on file-level node {target}");
                return Err(EXIT_ERROR);
            }
            record_resolved_entry_target(&mut file_targets, resolved.file, resolved.entry_index);
        } else {
            let path = PathBuf::from(target);
            if !path.exists() {
                eprintln!("org: file not found: {target}");
                return Err(EXIT_ERROR);
            }
            for file in collect_org_files(&[path]) {
                file_targets.insert(file, None);
            }
        }
    }

    if file_targets.is_empty() {
        eprintln!("org: no .org files found");
        return Err(EXIT_ERROR);
    }
    let mut file_list: Vec<_> = file_targets.into_iter().collect();
    file_list.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(file_list)
}

/// Appends `--stdin` locators when requested, rejects empty targets, and resolves [`FileEntryTargets`].
pub(crate) fn resolve_final_batch_targets(
    targets: &mut Vec<String>,
    read_stdin: bool,
    workspace: Option<&Path>,
    action_desc: &str,
) -> Result<FileEntryTargets, i32> {
    if read_stdin {
        targets.extend(read_stdin_targets().map_err(|e| {
            eprintln!("{e}");
            EXIT_ERROR
        })?);
    }
    if targets.is_empty() {
        eprintln!("org: no targets specified");
        return Err(EXIT_ERROR);
    }
    resolve_batch_entry_targets(targets, workspace, action_desc)
}

/// Resolves a single Org locator string into its parsed entry, source file, and document.
pub(crate) fn resolve_single_target(
    locator_str: &str,
) -> Result<(ResolvedEntry, SourceFile, OrgDocument), i32> {
    let locator = OrgLocator::parse(locator_str).map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut search_paths = vec![cwd.clone()];
    if let Ok(cfg) = Config::load_for_dir(&cwd) {
        for (_, vp) in cfg.roam_directories() {
            if vp.exists() && !search_paths.contains(&vp) {
                search_paths.push(vp);
            }
        }
    }
    let resolved = resolve_locator(&locator, &search_paths).map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;
    let source = SourceFile::from_path(&resolved.file).map_err(|e| {
        eprintln!("org: error reading {}: {e}", resolved.file.display());
        EXIT_ERROR
    })?;
    let doc = OrgDocument::from_source(&source);
    Ok((resolved, source, doc))
}

// endregion

// region Subcommand Dispatch

/// Dispatches property, tag, dependency, cookie, and table `org update` subcommands.
fn run_metadata_or_table_update(command: UpdateCommand) -> i32 {
    match command {
        UpdateCommand::SetProperty(args) => run_set_property(args),
        UpdateCommand::DeleteProperty(args) => run_delete_property(args),
        UpdateCommand::SetTags(args) => run_set_tags(args),
        UpdateCommand::Dep { command } => run_dep(command),
        UpdateCommand::AddCookie {
            paths,
            recursive,
            dry_run,
            format,
        } => run_add_cookie(paths, recursive, dry_run, format),
        UpdateCommand::Calc {
            paths,
            dry_run,
            force: _,
            format,
        } => run_calc(paths, dry_run, format),
        _ => EXIT_ERROR,
    }
}

/// Runs the `update` subcommand.
pub(crate) fn run_update(command: UpdateCommand) -> i32 {
    match command {
        UpdateCommand::AddId(args) => run_add_id(*args),
        UpdateCommand::SetState(args) => run_set_state(*args),
        UpdateCommand::SetScheduled(args) => run_set_scheduled(*args),
        UpdateCommand::SetDeadline(args) => run_set_deadline(*args),
        UpdateCommand::AddTodo(args) => run_add_todo(*args),
        other => run_metadata_or_table_update(other),
    }
}

// endregion
