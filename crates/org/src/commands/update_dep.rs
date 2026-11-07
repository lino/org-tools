// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org update dep add|remove` subcommand definitions and symmetric `:BLOCKER:` /
//! `:TRIGGER:` dependency wiring with workspace cycle prevention.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, Subcommand};

use org_tools_core::document::OrgDocument;
use org_tools_core::edna::{
    discover_workspace_root, merge_blocker_id, merge_trigger_id, remove_blocker_id,
    remove_trigger_id, would_create_blocker_cycle, WorkspaceOverlay,
};
use org_tools_core::locator::{resolve_locator, OrgLocator, ResolvedEntry};
use org_tools_core::property::{delete_property, set_property};
use org_tools_core::source::SourceFile;

use crate::commands::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Types

/// Default TODO state wired into `:TRIGGER: ids("...") todo!("<STATE>")` on `dep add`.
const DEFAULT_TRIGGER_STATE: &str = "NEXT";

/// Subcommands for `org update dep`.
#[derive(Subcommand)]
pub(crate) enum DepCommand {
    /// Wire symmetric :BLOCKER: and :TRIGGER: dependencies between two entries.
    Add(DepAddArgs),
    /// Remove symmetric :BLOCKER: and :TRIGGER: dependencies between two entries.
    Remove(DepRemoveArgs),
}

/// CLI arguments for `org update dep add`.
#[derive(Args)]
pub(crate) struct DepAddArgs {
    /// Locator of the upstream blocking entry (`<BLOCKER_LOCATOR>`).
    pub(crate) blocker: String,

    /// Locator of the downstream blocked entry (`<BLOCKED_LOCATOR>`).
    pub(crate) blocked: String,

    /// Downstream TODO state triggered when `<blocker>` completes.
    #[arg(long, default_value = DEFAULT_TRIGGER_STATE)]
    pub(crate) trigger_state: String,

    /// Optional workspace root directory for cross-file resolution and cycle checks.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Preview dependency mutations without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// CLI arguments for `org update dep remove`.
#[derive(Args)]
pub(crate) struct DepRemoveArgs {
    /// Locator of the upstream blocking entry (`<BLOCKER_LOCATOR>`).
    pub(crate) blocker: String,

    /// Locator of the downstream blocked entry (`<BLOCKED_LOCATOR>`).
    pub(crate) blocked: String,

    /// Optional workspace root directory for cross-file locator resolution.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Preview dependency removal without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// Resolved endpoint metadata for `<blocker>` or `<blocked>`.
struct ResolvedDepEndpoint {
    /// Resolved file and entry location.
    resolved: ResolvedEntry,
    /// Verified `:ID:` value on the heading entry.
    id: String,
}

// endregion

// region Workspace & Endpoint Resolution

/// Builds search roots for locator resolution and workspace graph loading.
fn build_search_roots(
    explicit_ws: Option<&Path>,
    loc_a: &OrgLocator,
    loc_b: &OrgLocator,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(ws) = explicit_ws {
        roots.push(ws.to_path_buf());
    }
    for loc in [loc_a, loc_b] {
        let file_opt = match loc {
            OrgLocator::CustomId { file, .. }
            | OrgLocator::OutlinePath { file, .. }
            | OrgLocator::LineRef { file, .. } => Some(file.as_path()),
            OrgLocator::Id(_) => None,
        };
        if let Some(f) = file_opt {
            let discovered = discover_workspace_root(explicit_ws, f);
            if !roots.contains(&discovered) {
                roots.push(discovered);
            }
        }
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if !roots.contains(&cwd) {
        roots.push(cwd);
    }
    roots
}

/// Resolves both endpoints and verifies each target is a heading carrying an `:ID:` property.
fn resolve_endpoints(
    blocker_raw: &str,
    blocked_raw: &str,
    explicit_ws: Option<&Path>,
) -> Result<(ResolvedDepEndpoint, ResolvedDepEndpoint, WorkspaceOverlay), i32> {
    let blocker_loc = OrgLocator::parse(blocker_raw).map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;
    let blocked_loc = OrgLocator::parse(blocked_raw).map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;

    let search_roots = build_search_roots(explicit_ws, &blocker_loc, &blocked_loc);
    let blocker_res = resolve_locator(&blocker_loc, &search_roots).map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;
    let blocked_res = resolve_locator(&blocked_loc, &search_roots).map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;

    let ws_root = discover_workspace_root(explicit_ws, &blocker_res.file);
    let mut overlay = WorkspaceOverlay::load_workspace(&ws_root);
    overlay.ensure_loaded(&blocker_res.file);
    overlay.ensure_loaded(&blocked_res.file);

    let blocker_ep = extract_verified_endpoint(&overlay, blocker_res, blocker_raw)?;
    let blocked_ep = extract_verified_endpoint(&overlay, blocked_res, blocked_raw)?;
    Ok((blocker_ep, blocked_ep, overlay))
}

/// Verifies `resolved` is a heading entry with a drawer `:ID:` property.
fn extract_verified_endpoint(
    overlay: &WorkspaceOverlay,
    resolved: ResolvedEntry,
    raw_locator: &str,
) -> Result<ResolvedDepEndpoint, i32> {
    if resolved.is_file_node() {
        eprintln!(
            "org: dependency target '{raw_locator}' must be a heading entry, not a file node"
        );
        return Err(EXIT_ERROR);
    }
    let Some(content) = overlay.content(&resolved.file) else {
        eprintln!("org: unable to read {}", resolved.file.display());
        return Err(EXIT_ERROR);
    };
    let doc =
        OrgDocument::from_source(&SourceFile::new(resolved.file.clone(), content.to_string()));
    let id = doc
        .entries
        .get(resolved.entry_index)
        .and_then(|e| {
            e.properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("ID"))
                .map(|(_, v)| v.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            eprintln!(
                "org: entry '{raw_locator}' has no :ID: property (run `org update add-id` first)"
            );
            EXIT_ERROR
        })?;
    Ok(ResolvedDepEndpoint { resolved, id })
}

/// Checks whether adding `blocker -> blocked` creates a `:BLOCKER:` cycle in `overlay`.
fn ensure_acyclic_addition(
    overlay: &WorkspaceOverlay,
    blocker_id: &str,
    blocked_id: &str,
) -> Result<(), i32> {
    let docs = overlay.parse_documents();
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let Some((bkr_doc, bkr_idx)) = find_heading_by_id(&docs, blocker_id) else {
        return Ok(());
    };
    let Some((bkd_doc, bkd_idx)) = find_heading_by_id(&docs, blocked_id) else {
        return Ok(());
    };
    if let Some(cycle) =
        would_create_blocker_cycle(&doc_refs, (bkr_doc, bkr_idx), (bkd_doc, bkd_idx))
    {
        eprintln!(
            "org: refusing to create circular :BLOCKER: cycle: {}",
            cycle.join(" -> ")
        );
        return Err(EXIT_ERROR);
    }
    Ok(())
}

/// Locates `(&OrgDocument, entry_idx)` by heading `:ID:`.
fn find_heading_by_id<'a>(docs: &'a [OrgDocument], id: &str) -> Option<(&'a OrgDocument, usize)> {
    for doc in docs {
        if let Some(idx) = doc.find_by_id(id) {
            return Some((doc, idx));
        }
    }
    None
}

// endregion

// region Symmetric Property Mutations

/// Updates or deletes `prop_key` on `entry_id` inside `overlay` and records a [`MutationDetail`].
fn mutate_endpoint_property(
    overlay: &mut WorkspaceOverlay,
    file: &Path,
    entry_id: &str,
    prop_key: &str,
    new_val: Option<String>,
    details: &mut Vec<MutationDetail>,
) -> Result<bool, i32> {
    let raw = overlay.content(file).unwrap_or_default().to_string();
    let src = SourceFile::new(file.to_path_buf(), raw);
    let doc = OrgDocument::from_source(&src);
    let Some(idx) = doc.find_by_id(entry_id) else {
        eprintln!(
            "org: could not re-locate id:{entry_id} in {}",
            file.display()
        );
        return Err(EXIT_ERROR);
    };

    let (res, action, detail) = match new_val {
        Some(val) => {
            let r = set_property(&src, &doc, idx, prop_key, &val).map_err(|e| {
                eprintln!("org: {e}");
                EXIT_ERROR
            })?;
            (r, "property-set", Some(format!("{prop_key}={val}")))
        }
        None => {
            let r = delete_property(&src, &doc, idx, prop_key).map_err(|e| {
                eprintln!("org: {e}");
                EXIT_ERROR
            })?;
            (r, "property-deleted", Some(prop_key.to_string()))
        }
    };

    if res.changed {
        overlay.set_content(file, res.content);
        details.push(MutationDetail {
            file: file.display().to_string(),
            line: res.line,
            action: action.to_string(),
            detail,
        });
    }
    Ok(res.changed)
}

/// Reads the current drawer property `prop_key` for `entry_id` in `file` from `overlay`.
fn current_endpoint_prop(
    overlay: &WorkspaceOverlay,
    file: &Path,
    entry_id: &str,
    prop_key: &str,
) -> Option<String> {
    let raw = overlay.content(file)?;
    let doc = OrgDocument::from_source(&SourceFile::new(file.to_path_buf(), raw.to_string()));
    let idx = doc.find_by_id(entry_id)?;
    doc.entries[idx]
        .properties
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(prop_key))
        .map(|(_, v)| v.clone())
}

// endregion

// region Entrypoint

/// Executes `org update dep add`.
fn run_dep_add(args: DepAddArgs) -> i32 {
    let started = Instant::now();
    let (blocker, blocked, mut overlay) =
        match resolve_endpoints(&args.blocker, &args.blocked, args.workspace.as_deref()) {
            Ok(res) => res,
            Err(code) => return code,
        };
    if let Err(code) = ensure_acyclic_addition(&overlay, &blocker.id, &blocked.id) {
        return code;
    }

    let mut details = Vec::new();
    let cur_blk = current_endpoint_prop(&overlay, &blocked.resolved.file, &blocked.id, "BLOCKER");
    let merged_blk = merge_blocker_id(cur_blk.as_deref(), &blocker.id);
    let c1 = match mutate_endpoint_property(
        &mut overlay,
        &blocked.resolved.file,
        &blocked.id,
        "BLOCKER",
        Some(merged_blk),
        &mut details,
    ) {
        Ok(c) => usize::from(c),
        Err(code) => return code,
    };

    let cur_trg = current_endpoint_prop(&overlay, &blocker.resolved.file, &blocker.id, "TRIGGER");
    let merged_trg = merge_trigger_id(cur_trg.as_deref(), &blocked.id, &args.trigger_state);
    let c2 = match mutate_endpoint_property(
        &mut overlay,
        &blocker.resolved.file,
        &blocker.id,
        "TRIGGER",
        Some(merged_trg),
        &mut details,
    ) {
        Ok(c) => usize::from(c),
        Err(code) => return code,
    };

    finalize_dep_mutation(
        "dep-add",
        args.dry_run,
        args.format,
        started,
        &overlay,
        c1 + c2,
        details,
    )
}

/// Executes `org update dep remove`.
fn run_dep_remove(args: DepRemoveArgs) -> i32 {
    let started = Instant::now();
    let (blocker, blocked, mut overlay) =
        match resolve_endpoints(&args.blocker, &args.blocked, args.workspace.as_deref()) {
            Ok(res) => res,
            Err(code) => return code,
        };

    let mut details = Vec::new();
    let cur_blk = current_endpoint_prop(&overlay, &blocked.resolved.file, &blocked.id, "BLOCKER");
    let next_blk = remove_blocker_id(cur_blk.as_deref(), &blocker.id);
    let c1 = match mutate_endpoint_property(
        &mut overlay,
        &blocked.resolved.file,
        &blocked.id,
        "BLOCKER",
        next_blk,
        &mut details,
    ) {
        Ok(c) => usize::from(c),
        Err(code) => return code,
    };

    let cur_trg = current_endpoint_prop(&overlay, &blocker.resolved.file, &blocker.id, "TRIGGER");
    let next_trg = remove_trigger_id(cur_trg.as_deref(), &blocked.id);
    let c2 = match mutate_endpoint_property(
        &mut overlay,
        &blocker.resolved.file,
        &blocker.id,
        "TRIGGER",
        next_trg,
        &mut details,
    ) {
        Ok(c) => usize::from(c),
        Err(code) => return code,
    };

    finalize_dep_mutation(
        "dep-remove",
        args.dry_run,
        args.format,
        started,
        &overlay,
        c1 + c2,
        details,
    )
}

/// Commits `overlay` (unless `dry_run`) and prints the [`MutationResult`].
fn finalize_dep_mutation(
    cmd_name: &str,
    dry_run: bool,
    format: MutationOutputFormat,
    started: Instant,
    overlay: &WorkspaceOverlay,
    entries_affected: usize,
    details: Vec<MutationDetail>,
) -> i32 {
    let mut mutation = MutationResult::new(cmd_name, dry_run);
    let modified_count = overlay.modified_files().len();
    if !dry_run {
        if let Err(msg) = overlay.commit_to_disk() {
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return EXIT_ERROR;
        }
    }
    mutation.files_modified = modified_count;
    mutation.entries_affected = entries_affected;
    mutation.details = details;
    mutation.finish(started, overlay.parse_documents().len());
    output_mutation(&mutation, format);
    EXIT_OK
}

/// Dispatches `org update dep` subcommands (`add` and `remove`).
pub(crate) fn run_dep(command: DepCommand) -> i32 {
    match command {
        DepCommand::Add(args) => run_dep_add(args),
        DepCommand::Remove(args) => run_dep_remove(args),
    }
}

// endregion
