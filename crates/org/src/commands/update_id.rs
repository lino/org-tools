// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org update add-id` arguments, target resolution, and Stempelamt / UUIDv4 minting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;

use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::id::{self, IdGenerator};
use org_tools_core::locator::OrgLocator;
use org_tools_core::property::{self, DeterministicIdOptions, StempelamtConfig};
use org_tools_core::source::SourceFile;

use super::update::{resolve_single_target, FileEntryTargets};
use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Types

/// CLI arguments for `org update add-id`.
#[derive(Args)]
pub(crate) struct AddIdArgs {
    /// Targets: file paths, directory paths, or org locator strings.
    pub(crate) targets: Vec<String>,

    /// Include descendant entries when targeting via locator.
    #[arg(long, short)]
    pub(crate) recursive: bool,

    /// Print what would be changed without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Generate IDs from a template with {placeholders}.
    #[arg(long, conflicts_with_all = ["id_command", "deterministic"])]
    pub(crate) id_format: Option<String>,

    /// Pipe entry metadata (JSON) to a command that outputs the ID.
    #[arg(long, conflicts_with_all = ["id_format", "deterministic"])]
    pub(crate) id_command: Option<String>,

    /// Mint deterministic RFC 9562 UUIDv5 `:ID:` and `:ID_SEED:` properties.
    #[arg(long)]
    pub(crate) deterministic: bool,

    /// Root namespace UUID override for deterministic Stempelamt minting.
    #[arg(long)]
    pub(crate) namespace: Option<String>,

    /// URN prefix override for deterministic Stempelamt seed derivation.
    #[arg(long)]
    pub(crate) urn_prefix: Option<String>,

    /// Explicit Stempelamt seed URN override.
    #[arg(long)]
    pub(crate) seed: Option<String>,

    /// Scope slug for deterministic Stempelamt seed derivation.
    #[arg(long)]
    pub(crate) scope: Option<String>,

    /// Artifact kind for deterministic Stempelamt seed derivation.
    #[arg(long)]
    pub(crate) kind: Option<String>,

    /// Optional author handle for deterministic Stempelamt seed derivation.
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

// region ID Minting & File Mutation

/// Builds per-entry [`MutationDetail`] records by comparing pre/post `:ID:` properties.
fn collect_added_id_details(
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
        .filter(|(idx, entry)| {
            entry.properties.contains_key("ID")
                && before
                    .entries
                    .get(*idx)
                    .is_some_and(|prev| !prev.properties.contains_key("ID"))
        })
        .map(|(_, entry)| MutationDetail {
            file: file_str.clone(),
            line: entry.heading_line,
            action: "added-id".to_string(),
            detail: entry.properties.get("ID").cloned(),
        })
        .collect()
}

/// Applies either deterministic Stempelamt or classic `:ID:` minting to one document.
fn apply_file_add_ids(
    source: &SourceFile,
    doc: &OrgDocument,
    indices: Option<&[usize]>,
    generator: &IdGenerator,
    det_opts: Option<&DeterministicIdOptions>,
) -> Result<Option<(String, Vec<MutationDetail>)>, String> {
    if let Some(opts) = det_opts {
        let Some(res) = property::add_deterministic_ids(source, doc, indices, opts)? else {
            return Ok(None);
        };
        let file_str = source.path.display().to_string();
        let details = res
            .entries
            .into_iter()
            .map(|entry| MutationDetail {
                file: file_str.clone(),
                line: entry.line,
                action: "added-id".to_string(),
                detail: Some(format!("{} ({})", entry.id, entry.seed)),
            })
            .collect();
        return Ok(Some((res.content, details)));
    }

    let Some(res) = id::add_ids(source, doc, indices, generator)? else {
        return Ok(None);
    };
    let details = collect_added_id_details(&source.path, doc, &res.content);
    Ok(Some((res.content, details)))
}

/// Prints the human-mode per-file summary for `org update add-id`.
fn emit_add_id_file_human(file: &Path, count: usize, dry_run: bool) {
    let suffix = if count == 1 { "" } else { "s" };
    if dry_run {
        println!("Would add {count} ID{suffix} to {}", file.display());
    } else {
        println!("Added {count} ID{suffix} to {}", file.display());
    }
}

/// Processes `add-id` mutations for a single target file and updates `mutation`.
fn process_add_id_file(
    file: &Path,
    indices: Option<&[usize]>,
    args: &AddIdArgs,
    generator: &IdGenerator,
    det_opts: Option<&DeterministicIdOptions>,
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
    let applied = match apply_file_add_ids(&source, &doc, indices, generator, det_opts) {
        Ok(Some(pair)) => pair,
        Ok(None) => return,
        Err(e) => {
            let msg = format!("error processing {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    };

    let (new_content, details) = applied;
    let added_count = details.len();
    if !args.dry_run {
        if let Err(e) = org_tools_core::files::write_file_atomic(file, &new_content) {
            let msg = format!("error writing {}: {e}", file.display());
            eprintln!("org: {msg}");
            mutation.errors.push(msg);
            return;
        }
    }

    mutation.files_modified += 1;
    mutation.entries_affected += added_count;
    mutation.details.extend(details);
    if args.format == MutationOutputFormat::Human {
        emit_add_id_file_human(file, added_count, args.dry_run);
    }
}

/// Selects the classic [`IdGenerator`] configured on [`AddIdArgs`].
fn build_id_generator(args: &AddIdArgs) -> IdGenerator {
    if let Some(ref tpl) = args.id_format {
        IdGenerator::Template(tpl.clone())
    } else if let Some(ref cmd) = args.id_command {
        IdGenerator::Command(cmd.clone())
    } else {
        IdGenerator::Uuid
    }
}

/// Resolves optional Stempelamt [`DeterministicIdOptions`] from [`AddIdArgs`].
fn build_deterministic_opts(args: &AddIdArgs) -> Result<Option<DeterministicIdOptions>, i32> {
    if !args.deterministic {
        return Ok(None);
    }
    let cfg = StempelamtConfig::resolve(args.namespace.as_deref(), args.urn_prefix.as_deref())
        .map_err(|e| {
            eprintln!("org: {e}");
            EXIT_ERROR
        })?;
    Ok(Some(DeterministicIdOptions {
        config: cfg,
        explicit_seed: args.seed.clone(),
        scope: args.scope.clone(),
        kind: args.kind.clone(),
        author: args.author.clone(),
    }))
}

// endregion

// region Entrypoint

/// Runs `org update add-id`.
pub(crate) fn run_add_id(args: AddIdArgs) -> i32 {
    let started = Instant::now();
    if args.targets.is_empty() {
        eprintln!("org: no targets specified");
        return EXIT_ERROR;
    }

    let det_opts = match build_deterministic_opts(&args) {
        Ok(opts) => opts,
        Err(code) => return code,
    };
    let generator = build_id_generator(&args);
    let file_list = match resolve_add_id_targets(&args.targets, args.recursive) {
        Ok(list) => list,
        Err(code) => return code,
    };

    let mut mutation = MutationResult::new("add-id", args.dry_run);
    for (file, indices) in &file_list {
        process_add_id_file(
            file,
            indices.as_deref(),
            &args,
            &generator,
            det_opts.as_ref(),
            &mut mutation,
        );
    }
    mutation.finish(started, file_list.len());

    if args.format == MutationOutputFormat::Json {
        output_mutation(&mutation, MutationOutputFormat::Json);
    } else if mutation.entries_affected == 0 && !args.dry_run && mutation.errors.is_empty() {
        println!("All entries already have IDs");
    }
    if !mutation.errors.is_empty() {
        return EXIT_ERROR;
    }
    EXIT_OK
}

// endregion
