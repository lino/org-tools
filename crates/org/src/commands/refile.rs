// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand handler for `org refile` atomic same-file and cross-file subtree refiling.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;

use org_tools_core::document::OrgDocument;
use org_tools_core::files::{canonicalize_clean, collect_org_files, write_file_atomic};
use org_tools_core::locator::{resolve_in_document, resolve_locator, OrgLocator, ResolvedEntry};
use org_tools_core::property::resolve_tag_column;
use org_tools_core::refile::{
    refile_cross_file, refile_same_file, RefileDestination, RefileOptions, RefileOutcome,
};
use org_tools_core::source::SourceFile;

use super::update::invalidate_warm_workspace_caches;
use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Types

/// CLI arguments for `org refile`.
#[derive(Args)]
pub(crate) struct RefileArgs {
    /// Source entry locator (`id:<uuid>`, `<file>::#<custom-id>`, `<file>::*/Path`, `<file>::*Heading`, or `<file>::<line>`).
    pub(crate) source_locator: String,

    /// Destination locator or `.org` file path.
    #[arg(long = "to")]
    pub(crate) to: String,

    /// Files or directories to scan when resolving locators.
    #[arg(default_value = ".")]
    pub(crate) paths: Vec<PathBuf>,

    /// Optional workspace root directory override.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Refile the subtree as a top-level (`*`) heading at the end of the target file.
    #[arg(long)]
    pub(crate) as_root: bool,

    /// Preview the refile operation without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

// endregion

// region Flexible Locator Resolution

/// Parses a fileless shorthand (`custom-id:...`, `#...`, `olp:...`, `*Heading`) against `file`.
fn parse_default_file_shorthand(trimmed: &str, file: &Path) -> Option<Result<OrgLocator, String>> {
    if let Some(cid) = trimmed
        .strip_prefix("custom-id:")
        .or_else(|| trimmed.strip_prefix('#'))
    {
        return Some(
            OrgLocator::parse(&format!("{}::#{}", file.display(), cid.trim()))
                .map_err(|e| e.to_string()),
        );
    }
    if let Some(olp) = trimmed
        .strip_prefix("olp:")
        .or_else(|| trimmed.strip_prefix('*'))
    {
        let cleaned = olp.trim_start_matches('*').trim();
        return Some(
            OrgLocator::parse(&format!("{}::*/{cleaned}", file.display()))
                .map_err(|e| e.to_string()),
        );
    }
    None
}

/// Normalizes shorthand locator strings (`file.org::*Heading`, `file.org::olp:A/B`) into [`OrgLocator`].
pub(crate) fn parse_flexible_locator(
    raw: &str,
    default_file: Option<&Path>,
) -> Result<OrgLocator, String> {
    let trimmed = raw.trim();
    if let Ok(loc) = OrgLocator::parse(trimmed) {
        return Ok(loc);
    }
    if let Some((file_part, search_part)) = trimmed.split_once("::") {
        let file = if file_part.trim().is_empty() {
            default_file
                .ok_or_else(|| format!("missing file path in locator '{trimmed}'"))?
                .to_path_buf()
        } else {
            PathBuf::from(file_part.trim())
        };
        let search = search_part.trim();
        let cleaned = search
            .strip_prefix("olp:")
            .or_else(|| search.strip_prefix("*/"))
            .unwrap_or_else(|| search.trim_start_matches('*').trim_start());
        if !cleaned.is_empty() && !cleaned.starts_with('#') {
            return OrgLocator::parse(&format!("{}::*/{cleaned}", file.display()))
                .map_err(|e| e.to_string());
        }
    }
    if let Some(file) = default_file {
        if let Some(res) = parse_default_file_shorthand(trimmed, file) {
            return res;
        }
    }
    Err(format!("invalid locator format: {trimmed}"))
}

/// Resolves `locator` in `doc`, falling back to matching a single-segment `*Heading` at any depth.
pub(crate) fn resolve_flexible_in_document(
    locator: &OrgLocator,
    doc: &OrgDocument,
) -> Result<ResolvedEntry, String> {
    if let Ok(resolved) = resolve_in_document(locator, doc) {
        return Ok(resolved);
    }
    if let OrgLocator::OutlinePath { file, path } = locator {
        if path.len() == 1 {
            let target_lower = path[0].to_lowercase();
            if let Some((idx, entry)) = doc
                .entries
                .iter()
                .enumerate()
                .find(|(_, e)| e.title.to_lowercase() == target_lower)
            {
                return Ok(ResolvedEntry {
                    file: file.clone(),
                    line: entry.heading_line,
                    heading_text: entry.title.clone(),
                    level: entry.level,
                    entry_index: idx,
                });
            }
        }
    }
    Err(format!("entry not found: {locator}"))
}

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

/// Returns `true` when `raw` is a plain `.org` file path rather than a heading locator.
pub(crate) fn is_plain_org_path_target(raw: &str) -> bool {
    let trimmed = raw.trim();
    !trimmed.contains("::")
        && !trimmed.starts_with("id:")
        && !trimmed.starts_with("custom-id:")
        && !trimmed.starts_with("olp:")
        && !trimmed.starts_with('#')
        && !trimmed.starts_with('*')
        && Path::new(trimmed)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("org"))
}

/// Builds ordered search roots from `--workspace`, positional `paths`, and `current_dir()`.
fn build_refile_search_paths(args: &RefileArgs) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(ref ws) = args.workspace {
        roots.push(ws.clone());
    }
    for path in &args.paths {
        if !roots.contains(path) {
            roots.push(path.clone());
        }
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if !roots.contains(&cwd) {
        roots.push(cwd);
    }
    roots
}

/// Resolves `--to <TARGET>` into `(dest_file_path, RefileDestination)`.
fn resolve_refile_destination(
    to_raw: &str,
    as_root: bool,
    search_paths: &[PathBuf],
    source_file: &Path,
    workspace: Option<&Path>,
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

    let resolved = resolve_flexible_locator(to_raw, search_paths, Some(source_file))?;
    if resolved.is_file_node() || as_root {
        Ok((resolved.file, RefileDestination::FileRoot))
    } else {
        Ok((
            resolved.file,
            RefileDestination::UnderEntry(resolved.entry_index),
        ))
    }
}

/// Returns `true` when `path_a` and `path_b` resolve to the same canonical file on disk.
pub(crate) fn is_same_org_file(path_a: &Path, path_b: &Path) -> bool {
    if path_a == path_b {
        return true;
    }
    match (canonicalize_clean(path_a), canonicalize_clean(path_b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
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

// endregion

// region Entrypoint

/// Builds the [`MutationResult`] envelope for a completed or dry-run refile operation.
fn build_refile_mutation_result(
    dest_file: &Path,
    outcome: &RefileOutcome,
    modified_count: usize,
    dry_run: bool,
    started: Instant,
) -> MutationResult {
    let mut mutation = MutationResult::new("refile", dry_run);
    mutation.files_modified = modified_count;
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
    mutation.finish(started, modified_count);
    mutation
}

/// Executes `org refile` and returns the CLI exit code.
pub(crate) fn run_refile(args: RefileArgs, no_cache: bool) -> i32 {
    let started = Instant::now();
    let search_paths = build_refile_search_paths(&args);
    let source_res = match resolve_flexible_locator(&args.source_locator, &search_paths, None) {
        Ok(res) => res,
        Err(msg) => {
            eprintln!("org: {msg}");
            return EXIT_ERROR;
        }
    };
    if source_res.is_file_node() {
        eprintln!(
            "org: cannot refile file-level node '{}'; specify a heading locator",
            args.source_locator
        );
        return EXIT_ERROR;
    }
    let (dest_file, dest_target) = match resolve_refile_destination(
        &args.to,
        args.as_root,
        &search_paths,
        &source_res.file,
        args.workspace.as_deref(),
    ) {
        Ok(pair) => pair,
        Err(msg) => {
            eprintln!("org: {msg}");
            return EXIT_ERROR;
        }
    };
    let opts = RefileOptions {
        as_root: args.as_root,
        tag_column: resolve_tag_column(None),
    };
    let (outcome, modified_files) = match execute_refile_pass(
        &source_res.file,
        source_res.entry_index,
        &dest_file,
        dest_target,
        opts,
    ) {
        Ok(res) => res,
        Err(msg) => {
            eprintln!("org: {msg}");
            return EXIT_ERROR;
        }
    };
    if !args.dry_run {
        if let Err(msg) = commit_refile_writes(&source_res.file, &dest_file, &outcome) {
            eprintln!("org: {msg}");
            return EXIT_ERROR;
        }
        if !no_cache {
            invalidate_warm_workspace_caches(args.workspace.as_deref(), &modified_files);
        }
    }
    let mutation = build_refile_mutation_result(
        &dest_file,
        &outcome,
        modified_files.len(),
        args.dry_run,
        started,
    );
    output_mutation(&mutation, args.format);
    EXIT_OK
}

// endregion
