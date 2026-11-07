// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org update` subcommand entry module, shared locator resolution, and dispatch.

use std::path::PathBuf;

use clap::Subcommand;

use org_tools_core::document::OrgDocument;
use org_tools_core::locator::{resolve_locator, OrgLocator, ResolvedEntry};
use org_tools_core::source::SourceFile;

use super::update_id::{run_add_id, AddIdArgs};
use super::update_property::{
    run_delete_property, run_set_property, run_set_tags, DeletePropertyArgs, SetPropertyArgs,
    SetTagsArgs,
};
use super::update_state::{run_set_state, SetStateArgs};
use super::update_table::{run_add_cookie, run_calc};
use super::update_todo::{run_add_todo, AddTodoArgs};
use super::MutationOutputFormat;
use crate::EXIT_ERROR;

/// Sorted `(file_path, optional_entry_indices)` targets for batch mutation commands.
pub(crate) type FileEntryTargets = Vec<(PathBuf, Option<Vec<usize>>)>;

/// Subcommands for `org update`.
#[derive(Subcommand)]
pub(crate) enum UpdateCommand {
    /// Add :ID: properties to entries that lack them.
    AddId(Box<AddIdArgs>),
    /// Change the TODO state of entries.
    SetState(Box<SetStateArgs>),
    /// Add a new TODO entry.
    AddTodo(Box<AddTodoArgs>),
    /// Set or insert a property in an entry's :PROPERTIES: drawer.
    SetProperty(SetPropertyArgs),
    /// Delete a property from an entry's :PROPERTIES: drawer.
    DeleteProperty(DeletePropertyArgs),
    /// Set or clear heading tags aligned to column 77.
    SetTags(SetTagsArgs),
    /// Update statistic cookies ([/] and [%]) on headings.
    AddCookie {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
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
        #[arg(default_value = ".")]
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

/// Resolves a single Org locator string into its parsed entry, source file, and document.
pub(crate) fn resolve_single_target(
    locator_str: &str,
) -> Result<(ResolvedEntry, SourceFile, OrgDocument), i32> {
    let locator = OrgLocator::parse(locator_str).map_err(|e| {
        eprintln!("org: {e}");
        EXIT_ERROR
    })?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let resolved = resolve_locator(&locator, &[cwd]).map_err(|e| {
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

/// Runs the `update` subcommand.
pub(crate) fn run_update(command: UpdateCommand) -> i32 {
    match command {
        UpdateCommand::AddId(args) => run_add_id(*args),
        UpdateCommand::SetState(args) => run_set_state(*args),
        UpdateCommand::AddTodo(args) => run_add_todo(*args),
        UpdateCommand::SetProperty(args) => run_set_property(args),
        UpdateCommand::DeleteProperty(args) => run_delete_property(args),
        UpdateCommand::SetTags(args) => run_set_tags(args),
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
    }
}
