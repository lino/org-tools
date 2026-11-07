// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org update set-property`, `delete-property`, and `set-tags` subcommand handlers.

use std::time::Instant;

use clap::Args;

use org_tools_core::property;

use super::update::resolve_single_target;
use super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

/// CLI arguments for `org update set-property`.
#[derive(Args)]
pub(crate) struct SetPropertyArgs {
    /// Org locator identifying the target entry.
    pub(crate) locator: String,

    /// Property key name without colons.
    pub(crate) key: String,

    /// Property value to set.
    pub(crate) value: String,

    /// Print what would be changed without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// CLI arguments for `org update delete-property`.
#[derive(Args)]
pub(crate) struct DeletePropertyArgs {
    /// Org locator identifying the target entry.
    pub(crate) locator: String,

    /// Property key name without colons.
    pub(crate) key: String,

    /// Print what would be changed without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// CLI arguments for `org update set-tags`.
#[derive(Args)]
pub(crate) struct SetTagsArgs {
    /// Org locator identifying the target entry.
    pub(crate) locator: String,

    /// Tags formatted as `:tag1:tag2:`, `tag1,tag2`, or empty `""` to clear.
    pub(crate) tags: Option<String>,

    /// Clear all tags from the heading.
    #[arg(long)]
    pub(crate) clear: bool,

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

/// Writes an updated single-file property/tag buffer (unless `dry_run`) and records `MutationDetail`.
fn record_single_entry_change(
    mutation: &mut MutationResult,
    file: &std::path::Path,
    changed: bool,
    content: &str,
    line: usize,
    action: &str,
    detail: Option<String>,
) -> Result<(), i32> {
    if !changed {
        return Ok(());
    }
    if !mutation.dry_run {
        std::fs::write(file, content).map_err(|e| {
            eprintln!("org: error writing {}: {e}", file.display());
            EXIT_ERROR
        })?;
    }
    mutation.files_modified = 1;
    mutation.entries_affected = 1;
    mutation.details.push(MutationDetail {
        file: file.display().to_string(),
        line,
        action: action.to_string(),
        detail,
    });
    Ok(())
}

/// Runs `org update set-property`.
pub(crate) fn run_set_property(args: SetPropertyArgs) -> i32 {
    let started = Instant::now();
    let (resolved, source, doc) = match resolve_single_target(&args.locator) {
        Ok(triple) => triple,
        Err(code) => return code,
    };
    let result =
        match property::set_property(&source, &doc, resolved.entry_index, &args.key, &args.value) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("org: {e}");
                return EXIT_ERROR;
            }
        };

    let mut mutation = MutationResult::new("set-property", args.dry_run);
    let detail = Some(format!("{}={}", args.key.trim(), args.value.trim()));
    if let Err(code) = record_single_entry_change(
        &mut mutation,
        &resolved.file,
        result.changed,
        &result.content,
        result.line,
        "set-property",
        detail,
    ) {
        return code;
    }
    mutation.finish(started, 1);
    output_mutation(&mutation, args.format);
    EXIT_OK
}

/// Runs `org update delete-property`.
pub(crate) fn run_delete_property(args: DeletePropertyArgs) -> i32 {
    let started = Instant::now();
    let (resolved, source, doc) = match resolve_single_target(&args.locator) {
        Ok(triple) => triple,
        Err(code) => return code,
    };
    let result = match property::delete_property(&source, &doc, resolved.entry_index, &args.key) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };

    let mut mutation = MutationResult::new("delete-property", args.dry_run);
    let detail = Some(args.key.trim().to_string());
    if let Err(code) = record_single_entry_change(
        &mut mutation,
        &resolved.file,
        result.changed,
        &result.content,
        result.line,
        "deleted-property",
        detail,
    ) {
        return code;
    }
    mutation.finish(started, 1);
    output_mutation(&mutation, args.format);
    EXIT_OK
}

/// Formats the human/JSON detail label for a `set-tags` mutation.
fn format_tags_detail(parsed_tags: &[String]) -> String {
    if parsed_tags.is_empty() {
        "cleared".to_string()
    } else {
        format!(":{}:", parsed_tags.join(":"))
    }
}

/// Runs `org update set-tags`.
pub(crate) fn run_set_tags(args: SetTagsArgs) -> i32 {
    let started = Instant::now();
    let parsed_tags = match property::parse_tag_input(args.tags.as_deref(), args.clear) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };
    let (resolved, source, doc) = match resolve_single_target(&args.locator) {
        Ok(triple) => triple,
        Err(code) => return code,
    };
    let tag_col = property::resolve_tag_column(args.tag_column);
    let result =
        match property::set_tags(&source, &doc, resolved.entry_index, &parsed_tags, tag_col) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("org: {e}");
                return EXIT_ERROR;
            }
        };

    let mut mutation = MutationResult::new("set-tags", args.dry_run);
    let detail = Some(format_tags_detail(&parsed_tags));
    if let Err(code) = record_single_entry_change(
        &mut mutation,
        &resolved.file,
        result.changed,
        &result.content,
        result.line,
        "set-tags",
        detail,
    ) {
        return code;
    }
    mutation.finish(started, 1);
    output_mutation(&mutation, args.format);
    EXIT_OK
}
