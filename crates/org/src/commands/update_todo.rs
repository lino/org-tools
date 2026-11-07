// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org update add-todo` arguments, entry composition, and insertion handler.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, ValueEnum};

use org_tools_core::document::OrgDocument;
use org_tools_core::locator::OrgLocator;
use org_tools_core::property::{self, FinalizeTodoOptions, NewTodoPropertySpec, TodoIdentityMode};
use org_tools_core::source::SourceFile;

use super::update::resolve_single_target;
use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Types

/// CLI mode for `:ID:` generation on `org update add-todo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum TodoIdArg {
    /// Do not automatically mint an `:ID:` property.
    None,
    /// Mint a random UUIDv4 `:ID:` property.
    Auto,
    /// Mint a deterministic Stempelamt UUIDv5 `:ID:` and `:ID_SEED:` pair.
    Deterministic,
}

impl TodoIdArg {
    /// Converts the CLI flag value into the core [`TodoIdentityMode`].
    fn into_core(self) -> TodoIdentityMode {
        match self {
            Self::None => TodoIdentityMode::None,
            Self::Auto => TodoIdentityMode::Auto,
            Self::Deterministic => TodoIdentityMode::Deterministic,
        }
    }
}

/// CLI arguments for `org update add-todo`.
#[derive(Args)]
pub(crate) struct AddTodoArgs {
    /// Title for the new entry.
    pub(crate) title: String,

    /// Target file or locator for parent entry.
    #[arg(default_value = ".")]
    pub(crate) target: String,

    /// Heading level (default: 1, or parent+1 if --parent is used).
    #[arg(long)]
    pub(crate) level: Option<usize>,

    /// TODO keyword (default: TODO).
    #[arg(long, default_value = "TODO")]
    pub(crate) keyword: String,

    /// Priority letter (A-Z).
    #[arg(long)]
    pub(crate) priority: Option<char>,

    /// Tags (comma-separated).
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,

    /// SCHEDULED date (YYYY-MM-DD).
    #[arg(long)]
    pub(crate) scheduled: Option<String>,

    /// DEADLINE date (YYYY-MM-DD).
    #[arg(long)]
    pub(crate) deadline: Option<String>,

    /// Insert as child of this locator target.
    #[arg(long)]
    pub(crate) parent: Option<String>,

    /// ID minting strategy for the new entry.
    #[arg(long = "id", value_enum, default_value = "none")]
    pub(crate) id_mode: TodoIdArg,

    /// Root namespace UUID override for deterministic ID minting.
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

    /// Set property KEY=VALUE inside :PROPERTIES: (repeatable).
    #[arg(long = "property", value_name = "KEY=VALUE")]
    pub(crate) properties: Vec<String>,

    /// Body text inserted below the heading and property drawer.
    #[arg(long)]
    pub(crate) body: Option<String>,

    /// Target display column for heading tag alignment (`F007`).
    #[arg(long)]
    pub(crate) tag_column: Option<usize>,

    /// Print what would be changed without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

// endregion

// region Destination & Source Helpers

/// Resolves the target file path and optional parent entry for `org update add-todo`.
fn resolve_add_todo_destination(
    target: &str,
    parent: Option<&str>,
) -> Result<(PathBuf, Option<usize>, Option<usize>), i32> {
    if let Some(parent_loc) = parent {
        let (resolved, _source, doc) = resolve_single_target(parent_loc)?;
        if resolved.is_file_node() {
            return Ok((resolved.file, None, None));
        }
        let plevel = doc.entries[resolved.entry_index].level;
        return Ok((resolved.file, Some(resolved.entry_index), Some(plevel)));
    }
    if OrgLocator::parse(target).is_ok() {
        let (resolved, _source, doc) = resolve_single_target(target)?;
        if resolved.is_file_node() {
            return Ok((resolved.file, None, None));
        }
        let plevel = doc.entries[resolved.entry_index].level;
        return Ok((resolved.file, Some(resolved.entry_index), Some(plevel)));
    }

    let path = PathBuf::from(target);
    if path.is_dir() {
        eprintln!("org: target must be a file, not a directory");
        return Err(EXIT_ERROR);
    }
    Ok((path, None, None))
}

/// Loads an existing Org file or initializes an empty in-memory [`SourceFile`].
fn load_or_init_todo_source(file_path: &Path) -> Result<(SourceFile, bool, bool), i32> {
    if file_path.exists() {
        let source = SourceFile::from_path(file_path).map_err(|e| {
            eprintln!("org: error reading {}: {e}", file_path.display());
            EXIT_ERROR
        })?;
        let was_empty = source.content.trim().is_empty();
        Ok((source, true, was_empty))
    } else {
        Ok((SourceFile::new(file_path, String::new()), false, true))
    }
}

// endregion

// region Entry Insertion & Output

/// Emits human-readable or JSON output for `org update add-todo`.
fn emit_add_todo_output(
    mutation: &MutationResult,
    file_path: &Path,
    existed: bool,
    format: MutationOutputFormat,
) {
    if format == MutationOutputFormat::Json {
        output_mutation(mutation, MutationOutputFormat::Json);
    } else if mutation.dry_run && !existed {
        println!("Would create {} with new entry", file_path.display());
    } else if mutation.dry_run {
        println!("Would add entry to {}", file_path.display());
    } else {
        println!("Added entry to {}", file_path.display());
    }
}

/// Inserts and post-processes a new TODO entry via `entry::add_entry` and `property`.
fn insert_and_finalize_todo(
    args: &AddTodoArgs,
    file_path: &Path,
    parent_idx: Option<usize>,
    parent_level: Option<usize>,
    properties: Vec<(String, String)>,
) -> Result<(property::FinalizedTodoResult, bool), i32> {
    let (source, existed, was_empty) = load_or_init_todo_source(file_path)?;
    let doc = OrgDocument::from_source(&source);
    let entry_level = args
        .level
        .unwrap_or_else(|| parent_level.map_or(1, |l| l + 1));
    let opts = org_tools_core::entry::NewEntryOpts {
        title: args.title.clone(),
        level: entry_level,
        keyword: Some(args.keyword.clone()),
        priority: args.priority,
        tags: args.tags.clone(),
        scheduled: args.scheduled.clone(),
        deadline: args.deadline.clone(),
    };

    let added = org_tools_core::entry::add_entry(&source, &doc, parent_idx, &opts);
    let finalized = property::finalize_added_todo(
        was_empty,
        &added.content,
        file_path,
        parent_idx,
        &FinalizeTodoOptions {
            properties,
            body: args.body.clone(),
            tag_column: property::resolve_tag_column(args.tag_column),
        },
    )
    .map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;
    Ok((finalized, existed))
}

// endregion

// region Entrypoint

/// Runs `org update add-todo`.
pub(crate) fn run_add_todo(args: AddTodoArgs) -> i32 {
    let started = Instant::now();
    let (file_path, parent_idx, parent_level) =
        match resolve_add_todo_destination(&args.target, args.parent.as_deref()) {
            Ok(dest) => dest,
            Err(code) => return code,
        };

    let properties = match property::build_todo_properties(&NewTodoPropertySpec {
        title: &args.title,
        file_path: &file_path,
        id_mode: args.id_mode.into_core(),
        namespace: args.namespace.as_deref(),
        urn_prefix: args.urn_prefix.as_deref(),
        seed: args.seed.as_deref(),
        scope: args.scope.as_deref(),
        kind: args.kind.as_deref(),
        author: args.author.as_deref(),
        raw_properties: &args.properties,
    }) {
        Ok(props) => props,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };

    let (finalized, existed) =
        match insert_and_finalize_todo(&args, &file_path, parent_idx, parent_level, properties) {
            Ok(pair) => pair,
            Err(code) => return code,
        };

    if !args.dry_run {
        if let Err(e) = std::fs::write(&file_path, &finalized.content) {
            eprintln!("org: error writing {}: {e}", file_path.display());
            return EXIT_ERROR;
        }
    }

    let mut mutation = MutationResult::new("add-todo", args.dry_run);
    mutation.files_modified = 1;
    mutation.entries_affected = 1;
    mutation.details.push(MutationDetail {
        file: file_path.display().to_string(),
        line: finalized.heading_line,
        action: "added-todo".to_string(),
        detail: Some(args.title),
    });
    mutation.finish(started, usize::from(existed));
    emit_add_todo_output(&mutation, &file_path, existed, args.format);
    EXIT_OK
}

// endregion
