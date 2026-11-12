// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand handler for `org refile` atomic same-file and cross-file subtree refiling.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, ValueHint};

use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::files::{collect_org_files, write_file_atomic};
use org_tools_core::gtd_roam::maybe_sync_agenda_filetag_for_mutation;
pub(crate) use org_tools_core::gtd_roam::{
    is_plain_org_path_target, is_same_org_file, parse_flexible_locator,
    resolve_flexible_in_document,
};
use org_tools_core::locator::{resolve_locator, OrgLocator, ResolvedEntry};
use org_tools_core::property::resolve_tag_column;
use org_tools_core::refile::{
    refile_cross_file, refile_same_file, RefileDestination, RefileOptions, RefileOutcome,
};
use org_tools_core::source::SourceFile;

use super::roam::{open_roam_backend, RoamScopeArgs};
use super::update::{invalidate_warm_workspace_caches, read_stdin_targets};
use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Types

/// CLI arguments for `org refile`.
#[derive(Args)]
pub(crate) struct RefileArgs {
    /// Source entry locator (`id:<uuid>`, `<file>::#<custom-id>`, `<file>::*/Path`, `<file>::*Heading`, or `<file>::<line>`; reads stdin when omitted).
    pub(crate) source_locator: Option<String>,
    /// Destination locator, `org-roam` node title/alias/ID, or `.org` file path.
    #[arg(long = "to")]
    pub(crate) to: String,
    /// Files or directories to scan when resolving locators.
    #[arg(value_hint = ValueHint::AnyPath)]
    pub(crate) paths: Vec<PathBuf>,
    /// Optional workspace root directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Refile the subtree as a top-level (`*`) heading at the end of the target file.
    #[arg(long)]
    pub(crate) as_root: bool,
    /// Skip automatic `:agenda:` filetag synchronization on `org-roam` files.
    #[arg(long)]
    pub(crate) no_agenda_sync: bool,
    /// Preview the refile operation without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

// endregion

// region Flexible Locator Resolution

/// Resolves a flexible locator string across `search_paths` (checking `preferred_file` first for fileless shorthands).
pub(crate) fn resolve_flexible_locator(
    raw: &str,
    search_paths: &[PathBuf],
    preferred_file: Option<&Path>,
) -> Result<ResolvedEntry, String> {
    if let Ok(loc) = parse_flexible_locator(raw, preferred_file) {
        match &loc {
            OrgLocator::Id(_) => {
                return resolve_locator(&loc, search_paths).map_err(|e| e.to_string())
            }
            OrgLocator::CustomId { file, .. }
            | OrgLocator::OutlinePath { file, .. }
            | OrgLocator::LineRef { file, .. } => {
                if !file.is_file() {
                    return Err(format!("file not found: {}", file.display()));
                }
                let src = SourceFile::from_path(file).map_err(|e| e.to_string())?;
                return resolve_flexible_in_document(&loc, &OrgDocument::from_source(&src));
            }
        }
    }
    for file in collect_org_files(search_paths) {
        if let Ok(loc) = parse_flexible_locator(raw, Some(&file)) {
            if let Ok(src) = SourceFile::from_path(&file) {
                if let Ok(res) = resolve_flexible_in_document(&loc, &OrgDocument::from_source(&src))
                {
                    return Ok(res);
                }
            }
        }
    }
    Err(format!("entry not found: {}", raw.trim()))
}

// endregion

// region Target Resolution & Atomic Persistence

/// Builds ordered search roots from `--workspace`, positional `paths`, roam vaults, and `current_dir()`.
fn build_refile_search_paths(args: &RefileArgs) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = args.workspace.iter().chain(&args.paths).cloned().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Ok(cfg) = Config::load_for_dir(args.workspace.as_deref().unwrap_or(&cwd)) {
        for (_, vp) in cfg.roam_directories() {
            if vp.exists() && !roots.contains(&vp) {
                roots.push(vp);
            }
        }
    }
    if !roots.contains(&cwd) {
        roots.push(cwd);
    }
    roots
}

/// Resolves `--to <TARGET>` against `org-roam` node titles, aliases, or `:ID:` across vaults.
fn resolve_roam_refile_destination(
    to_raw: &str,
    as_root: bool,
    workspace: Option<&Path>,
    no_cache: bool,
) -> Option<(PathBuf, RefileDestination)> {
    let scope = RoamScopeArgs {
        paths: Vec::new(),
        workspace: workspace.map(Path::to_path_buf),
        roam_dir: Vec::new(),
    };
    let ws = open_roam_backend(&scope, no_cache).ok()?;
    let node = ws.resolve(to_raw.trim()).ok()?.into_iter().next()?;
    let file = PathBuf::from(&node.file);
    if node.level == 0 || as_root {
        return Some((file, RefileDestination::FileRoot));
    }
    let src = SourceFile::from_path(&file).ok()?;
    let idx = OrgDocument::from_source(&src)
        .entries
        .iter()
        .position(|e| e.heading_line == node.line)?;
    Some((file, RefileDestination::UnderEntry(idx)))
}

/// Resolves `--to <TARGET>` into `(dest_file_path, RefileDestination)`.
fn resolve_refile_destination(
    to_raw: &str,
    as_root: bool,
    search_paths: &[PathBuf],
    source_file: &Path,
    workspace: Option<&Path>,
    no_cache: bool,
) -> Result<(PathBuf, RefileDestination), String> {
    if is_plain_org_path_target(to_raw) {
        let mut candidate = PathBuf::from(to_raw.trim());
        if !candidate.is_file() {
            if let Some(ws) = workspace {
                let joined = ws.join(&candidate);
                if joined.is_file() {
                    candidate = joined;
                }
            }
        }
        if !candidate.is_file() {
            return Err(format!("destination file not found: {}", to_raw.trim()));
        }
        return Ok((candidate, RefileDestination::FileRoot));
    }
    if let Ok(res) = resolve_flexible_locator(to_raw, search_paths, Some(source_file)) {
        let dest = if res.is_file_node() || as_root {
            RefileDestination::FileRoot
        } else {
            RefileDestination::UnderEntry(res.entry_index)
        };
        return Ok((res.file, dest));
    }
    if let Some(roam_dest) = resolve_roam_refile_destination(to_raw, as_root, workspace, no_cache) {
        return Ok(roam_dest);
    }
    Err(format!("entry not found: {}", to_raw.trim()))
}

/// Computes the [`RefileOutcome`] and affected file list for `(source_file, source_idx)` -> `(dest_file, dest)`.
pub(crate) fn execute_refile_pass(
    source_file: &Path,
    source_idx: usize,
    dest_file: &Path,
    dest: RefileDestination,
    opts: RefileOptions,
) -> Result<(RefileOutcome, Vec<PathBuf>), String> {
    let src_sf = SourceFile::from_path(source_file)
        .map_err(|e| format!("error reading {}: {e}", source_file.display()))?;
    let src_doc = OrgDocument::from_source(&src_sf);

    if is_same_org_file(source_file, dest_file) {
        let outcome = refile_same_file(&src_sf, &src_doc, source_idx, dest, opts)?;
        Ok((outcome, vec![source_file.to_path_buf()]))
    } else {
        let dest_sf = SourceFile::from_path(dest_file)
            .map_err(|e| format!("error reading {}: {e}", dest_file.display()))?;
        let dest_doc = OrgDocument::from_source(&dest_sf);
        let outcome = refile_cross_file(
            &src_sf, &src_doc, source_idx, &dest_sf, &dest_doc, dest, opts,
        )?;
        Ok((
            outcome,
            vec![source_file.to_path_buf(), dest_file.to_path_buf()],
        ))
    }
}

/// Atomically writes the updated destination and source contents to disk.
pub(crate) fn commit_refile_writes(
    source_file: &Path,
    dest_file: &Path,
    outcome: &RefileOutcome,
) -> Result<(), String> {
    write_file_atomic(dest_file, &outcome.dest_content)
        .map_err(|e| format!("error writing {}: {e}", dest_file.display()))?;
    if !outcome.same_file {
        write_file_atomic(source_file, &outcome.source_content)
            .map_err(|e| format!("error writing {}: {e}", source_file.display()))?;
    }
    Ok(())
}

/// Applies automatic `:agenda:` filetag upkeep to `outcome`'s destination and source buffers.
fn sync_refile_agenda_tags(
    source_file: &Path,
    dest_file: &Path,
    outcome: &mut RefileOutcome,
    workspace: Option<&Path>,
) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cfg = Config::load_for_dir(workspace.unwrap_or(&cwd)).unwrap_or_default();
    if let Some(s) = maybe_sync_agenda_filetag_for_mutation(dest_file, &outcome.dest_content, &cfg)
    {
        outcome.dest_content = s;
    }
    if !outcome.same_file {
        if let Some(s) =
            maybe_sync_agenda_filetag_for_mutation(source_file, &outcome.source_content, &cfg)
        {
            outcome.source_content = s;
        }
    }
}

// endregion

// region Entrypoint

/// Resolves the source locator from CLI `args` or piped `stdin`.
fn resolve_source_locator_arg(args: &RefileArgs) -> Result<String, i32> {
    if let Some(ref loc) = args.source_locator {
        return Ok(loc.clone());
    }
    if !std::io::stdin().is_terminal() {
        let targets = read_stdin_targets().map_err(|e| {
            eprintln!("{e}");
            EXIT_ERROR
        })?;
        if let Some(first) = targets.into_iter().next() {
            return Ok(first);
        }
    }
    eprintln!("org: missing source locator (specify <SOURCE_LOCATOR> or pipe a locator on stdin)");
    Err(EXIT_ERROR)
}

/// Executes `org refile` and returns the CLI exit code.
pub(crate) fn run_refile(args: RefileArgs, no_cache: bool) -> i32 {
    let started = Instant::now();
    let Ok(source_locator) = resolve_source_locator_arg(&args) else {
        return EXIT_ERROR;
    };
    let search_paths = build_refile_search_paths(&args);
    let Ok(source_res) =
        resolve_flexible_locator(&source_locator, &search_paths, None).map_err(|m| {
            eprintln!("org: {m}");
        })
    else {
        return EXIT_ERROR;
    };
    if source_res.is_file_node() {
        eprintln!(
            "org: cannot refile file-level node '{source_locator}'; specify a heading locator"
        );
        return EXIT_ERROR;
    }
    let Ok((dest_file, dest_target)) = resolve_refile_destination(
        &args.to,
        args.as_root,
        &search_paths,
        &source_res.file,
        args.workspace.as_deref(),
        no_cache,
    )
    .map_err(|m| eprintln!("org: {m}")) else {
        return EXIT_ERROR;
    };
    let opts = RefileOptions {
        as_root: args.as_root,
        tag_column: resolve_tag_column(None),
    };
    let Ok((mut outcome, modified_files)) = execute_refile_pass(
        &source_res.file,
        source_res.entry_index,
        &dest_file,
        dest_target,
        opts,
    )
    .map_err(|m| eprintln!("org: {m}")) else {
        return EXIT_ERROR;
    };
    if !args.no_agenda_sync {
        sync_refile_agenda_tags(
            &source_res.file,
            &dest_file,
            &mut outcome,
            args.workspace.as_deref(),
        );
    }
    if !args.dry_run {
        if let Err(msg) = commit_refile_writes(&source_res.file, &dest_file, &outcome) {
            eprintln!("org: {msg}");
            return EXIT_ERROR;
        }
        if !no_cache {
            invalidate_warm_workspace_caches(args.workspace.as_deref(), &modified_files);
        }
    }
    let mut mutation = MutationResult::new("refile", args.dry_run);
    mutation.files_modified = modified_files.len();
    mutation.entries_affected = 1;
    mutation.details.push(MutationDetail {
        file: dest_file.display().to_string(),
        line: outcome.new_line,
        action: "refiled".to_string(),
        detail: Some(format!(
            "{} -> {} (level {} -> {})",
            outcome.title, outcome.new_locator, outcome.old_level, outcome.new_level
        )),
    });
    mutation.finish(started, modified_files.len());
    output_mutation(&mutation, args.format);
    EXIT_OK
}

// endregion
