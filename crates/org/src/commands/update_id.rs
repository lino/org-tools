// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org update add-id` arguments, target resolution, and deterministic UUIDv5 / UUIDv4 minting.

mod file_drawer;
mod opts;
mod record;
mod sync_cache;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::id::{self, IdGenerator};
use org_tools_core::locator::OrgLocator;
use org_tools_core::property::{self, DeterministicIdOptions};
use org_tools_core::source::SourceFile;

use self::file_drawer::apply_file_level_id;
use self::opts::{build_deterministic_opts, build_id_generator, resolve_target_identity};
pub(crate) use self::record::{
    collect_heading_results, filter_max_level, AddIdOutput, AddIdRecord,
};
use self::sync_cache::sync_written_targets;
use super::update::{resolve_single_target, FileEntryTargets};
use super::{MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Types

/// CLI arguments for `org update add-id`.
#[derive(Args)]
pub(crate) struct AddIdArgs {
    /// Targets: file paths, directory paths, or org locator strings.
    pub(crate) targets: Vec<String>,

    /// Target the level-0 file :PROPERTIES: drawer.
    #[arg(long)]
    pub(crate) file: bool,

    /// Target headings within the specified files.
    #[arg(long)]
    pub(crate) headings: bool,

    /// Maximum heading level to receive an ID when targeting headings.
    #[arg(long)]
    pub(crate) max_level: Option<usize>,

    /// Explicit target locator string.
    #[arg(long)]
    pub(crate) locator: Option<String>,

    /// Include descendant entries when targeting via locator.
    #[arg(long, short)]
    pub(crate) recursive: bool,

    /// Print what would be changed without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Generate IDs from a template with {placeholders}.
    #[arg(long, conflicts_with_all = ["id_command", "deterministic", "uuidv5_seed"])]
    pub(crate) id_format: Option<String>,

    /// Pipe entry metadata (JSON) to a command that outputs the ID.
    #[arg(long, conflicts_with_all = ["id_format", "deterministic", "uuidv5_seed"])]
    pub(crate) id_command: Option<String>,

    /// Mint deterministic RFC 9562 UUIDv5 `:ID:` and `:ID_SEED:` properties.
    #[arg(long)]
    pub(crate) deterministic: bool,

    /// Deterministic seed URN for RFC 9562 UUIDv5 generation.
    #[arg(long)]
    pub(crate) uuidv5_seed: Option<String>,

    /// Write `:ID_SEED: <URN>` alongside deterministic `:ID:` properties.
    #[arg(long)]
    pub(crate) write_seed: bool,

    /// Root namespace UUID override for deterministic UUIDv5 minting.
    #[arg(long)]
    pub(crate) namespace: Option<String>,

    /// URN prefix override for deterministic seed derivation.
    #[arg(long)]
    pub(crate) urn_prefix: Option<String>,

    /// Explicit deterministic seed URN override.
    #[arg(long)]
    pub(crate) seed: Option<String>,

    /// Scope slug for deterministic seed derivation.
    #[arg(long)]
    pub(crate) scope: Option<String>,

    /// Artifact kind for deterministic seed derivation.
    #[arg(long)]
    pub(crate) kind: Option<String>,

    /// Optional author handle for deterministic seed derivation.
    #[arg(long)]
    pub(crate) author: Option<String>,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

// endregion

// region Target Resolution

/// Inserts resolved locator indices for `add-id` into `file_targets`.
fn insert_add_id_locator_target(
    file_targets: &mut HashMap<PathBuf, Option<Vec<usize>>>,
    target: &str,
    recursive: bool,
) -> Result<(), i32> {
    let (resolved, _source, doc) = resolve_single_target(target)?;
    if resolved.is_file_node() {
        if recursive {
            file_targets.insert(resolved.file, None);
        } else {
            file_targets
                .entry(resolved.file)
                .or_insert(Some(Vec::new()));
        }
        return Ok(());
    }
    let indices = if recursive {
        id::collect_subtree(&doc, resolved.entry_index)
    } else {
        vec![resolved.entry_index]
    };
    if let Some(ref mut existing) = file_targets
        .entry(resolved.file)
        .or_insert(Some(Vec::new()))
    {
        existing.extend(indices);
    }
    Ok(())
}

/// Resolves `add-id` CLI target arguments into sorted `(file, entry_indices)` pairs.
fn resolve_add_id_targets(targets: &[String], recursive: bool) -> Result<FileEntryTargets, i32> {
    let mut file_targets: HashMap<PathBuf, Option<Vec<usize>>> = HashMap::new();

    for target in targets {
        if OrgLocator::parse(target).is_ok() {
            insert_add_id_locator_target(&mut file_targets, target, recursive)?;
        } else {
            let files = collect_org_files(&[PathBuf::from(target)]);
            if files.is_empty() {
                eprintln!("org: no .org files found in {target}");
                return Err(EXIT_ERROR);
            }
            for file in files {
                file_targets.insert(file, None);
            }
        }
    }

    let mut file_list: Vec<_> = file_targets.into_iter().collect();
    file_list.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(file_list)
}

// endregion

// region File Processing

/// Prints human-mode per-file summary for `org update add-id`.
fn emit_add_id_file_human(file: &Path, count: usize, dry_run: bool) {
    let suffix = if count == 1 { "" } else { "s" };
    if dry_run {
        println!("Would add {count} ID{suffix} to {}", file.display());
    } else {
        println!("Added {count} ID{suffix} to {}", file.display());
    }
}

/// Applies file-level `:ID:` insertion when `--file` is configured or file has no headings.
#[allow(clippy::too_many_arguments)]
fn process_file_level(
    file: &Path,
    source: &SourceFile,
    doc: &OrgDocument,
    current_content: &mut String,
    args: &AddIdArgs,
    seen: &mut HashSet<String>,
    records: &mut Vec<AddIdRecord>,
    mutation: &mut MutationResult,
) -> bool {
    let do_file = args.file || (args.locator.is_some() && doc.entries.is_empty());
    if !do_file {
        return false;
    }
    let Ok((id, seed)) = resolve_target_identity(args, seen) else {
        return false;
    };
    let (new_c, rec) = apply_file_level_id(source, doc, &id, seed.as_deref());
    let mut modified = false;
    if rec.created {
        modified = true;
        mutation.details.push(MutationDetail {
            file: file.display().to_string(),
            line: rec.line,
            action: "added-id".to_string(),
            detail: Some(rec.id.clone()),
        });
    }
    *current_content = new_c;
    records.push(rec);
    modified
}

/// Applies heading `:ID:` insertion across filtered heading indices.
#[allow(clippy::too_many_arguments)]
fn process_heading_level(
    file: &Path,
    doc: &OrgDocument,
    raw_indices: Option<&[usize]>,
    current_content: &mut String,
    args: &AddIdArgs,
    generator: &IdGenerator,
    det_opts: Option<&DeterministicIdOptions>,
    records: &mut Vec<AddIdRecord>,
    details: &mut Vec<MutationDetail>,
) -> bool {
    let do_headings = args.headings || (!args.file && args.locator.is_none());
    if !do_headings || doc.entries.is_empty() {
        return false;
    }
    let all_indices: Vec<usize> = (0..doc.entries.len()).collect();
    let target_indices = raw_indices.unwrap_or(&all_indices);
    let filtered = filter_max_level(doc, target_indices, args.max_level);

    let cur_source = SourceFile::new(file, current_content.clone());
    let cur_doc = OrgDocument::from_source(&cur_source);
    let heading_res = if let Some(opts) = det_opts {
        property::add_deterministic_ids(&cur_source, &cur_doc, Some(&filtered), opts)
            .ok()
            .flatten()
            .map(|r| r.content)
    } else {
        id::add_ids(&cur_source, &cur_doc, Some(&filtered), generator)
            .ok()
            .flatten()
            .map(|r| r.content)
    };

    if let Some(new_heading_c) = heading_res {
        collect_heading_results(file, &cur_doc, &new_heading_c, records, details);
        *current_content = new_heading_c;
        return true;
    }
    false
}

/// Processes file-level and heading `add-id` mutations for a single file.
#[allow(clippy::too_many_arguments)]
fn process_file(
    file: &Path,
    raw_indices: Option<&[usize]>,
    args: &AddIdArgs,
    generator: &IdGenerator,
    det_opts: Option<&DeterministicIdOptions>,
    seen: &mut HashSet<String>,
    records: &mut Vec<AddIdRecord>,
    mutation: &mut MutationResult,
) -> Option<PathBuf> {
    let source = SourceFile::from_path(file).ok()?;
    let doc = OrgDocument::from_source(&source);
    let mut current_content = source.content.clone();
    let initial_details = mutation.details.len();

    let f_mod = process_file_level(
        file,
        &source,
        &doc,
        &mut current_content,
        args,
        seen,
        records,
        mutation,
    );
    let h_mod = process_heading_level(
        file,
        &doc,
        raw_indices,
        &mut current_content,
        args,
        generator,
        det_opts,
        records,
        &mut mutation.details,
    );
    let modified = f_mod || h_mod;
    let file_affected = mutation.details.len() - initial_details;
    mutation.entries_affected += file_affected;

    if file_affected > 0 && args.format == MutationOutputFormat::Human {
        emit_add_id_file_human(file, file_affected, args.dry_run);
    }

    if modified {
        if !args.dry_run {
            let _ = org_tools_core::files::write_file_atomic(file, &current_content);
        }
        mutation.files_modified += 1;
        return Some(file.to_path_buf());
    }
    None
}

// endregion

// region Output Rendering & Entrypoint

/// Renders JSON output for `org update add-id`.
fn emit_json_output(mutation: MutationResult, cache_synced: bool, records: Vec<AddIdRecord>) {
    let out = AddIdOutput {
        command: "add-id".to_string(),
        files_modified: mutation.files_modified,
        entries_affected: mutation.entries_affected,
        dry_run: mutation.dry_run,
        details: mutation.details,
        errors: mutation.errors,
        telemetry: mutation.telemetry,
        cache_synced,
        records,
    };
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}

/// Runs `org update add-id`.
pub(crate) fn run_add_id(args: AddIdArgs) -> i32 {
    let started = Instant::now();
    let mut targets = args.targets.clone();
    if let Some(ref loc) = args.locator {
        targets.push(loc.clone());
    }
    if targets.is_empty() {
        eprintln!("org: no targets specified");
        return EXIT_ERROR;
    }

    let det_opts = match build_deterministic_opts(&args) {
        Ok(opts) => opts,
        Err(code) => return code,
    };
    let generator = build_id_generator(&args);
    let file_list = match resolve_add_id_targets(&targets, args.recursive) {
        Ok(list) => list,
        Err(code) => return code,
    };

    let mut mutation = MutationResult::new("add-id", args.dry_run);
    let mut records = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut modified_paths = Vec::new();

    for (file, indices) in &file_list {
        if let Some(modified) = process_file(
            file,
            indices.as_deref(),
            &args,
            &generator,
            det_opts.as_ref(),
            &mut seen_ids,
            &mut records,
            &mut mutation,
        ) {
            modified_paths.push(modified);
        }
    }
    mutation.finish(started, file_list.len());

    let cache_synced = if !args.dry_run && !modified_paths.is_empty() {
        sync_written_targets(&modified_paths)
    } else {
        false
    };

    for r in &mut records {
        r.cache_synced = cache_synced;
    }

    let has_errors = !mutation.errors.is_empty();
    if args.format == MutationOutputFormat::Json {
        emit_json_output(mutation, cache_synced, records);
    } else if mutation.entries_affected == 0 && !args.dry_run && mutation.errors.is_empty() {
        println!("All entries already have IDs");
    }

    if has_errors {
        return EXIT_ERROR;
    }
    EXIT_OK
}

// endregion
