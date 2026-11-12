// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org update link` subcommand handler for structured and idempotent outbound link insertion.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use org_tools_core::document::OrgDocument;
use org_tools_core::locator::{ResolvedEntry, FILE_NODE_ENTRY_INDEX};

use super::super::update::{
    invalidate_warm_workspace_caches, resolve_single_target_with_workspace,
};
use super::super::{output_mutation, MutationDetail, MutationOutputFormat, MutationResult};
use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Args & Types

/// CLI arguments for `org update link`.
#[derive(Args, Debug)]
pub(crate) struct LinkArgs {
    /// Org locator identifying the source entry or file node.
    pub(crate) locator: String,

    /// Target destination ID or locator string.
    #[arg(long)]
    pub(crate) to: String,

    /// Optional section heading name under which to insert the link (e.g. "See Also").
    #[arg(long)]
    pub(crate) section: Option<String>,

    /// Optional link label/description override.
    #[arg(long)]
    pub(crate) label: Option<String>,

    /// Workspace directory for resolving locators and warm cache syncing.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Print what would be changed without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: MutationOutputFormat,
}

/// Bounding line span and level for the mutation target entry or file node.
#[derive(Debug, Clone)]
struct TargetSpan {
    /// 0-based start line of the target node or entry.
    start: usize,
    /// 0-based exclusive end line of the target node or entry's direct body.
    content_end: usize,
    /// 0-based exclusive end line of the target node's entire subtree.
    subtree_end: usize,
    /// Heading level (0 for level-0 file nodes).
    level: usize,
}

// endregion

// region Destination Target Resolution

/// Resolves the destination target into an `(id_or_target, title)` pair.
fn resolve_destination_target(
    to_str: &str,
    label: Option<&str>,
    workspace: Option<&Path>,
) -> (String, String) {
    let raw = to_str.trim();
    let locator_str = if !raw.starts_with("id:") && !raw.contains("::") {
        format!("id:{raw}")
    } else {
        raw.to_string()
    };

    if let Ok((resolved, _, target_doc)) =
        resolve_single_target_with_workspace(&locator_str, workspace)
    {
        let target_id = if resolved.is_file_node() {
            target_doc
                .file_id()
                .map(str::to_string)
                .unwrap_or_else(|| raw.to_string())
        } else {
            target_doc.entries[resolved.entry_index]
                .properties
                .get("ID")
                .cloned()
                .unwrap_or_else(|| raw.to_string())
        };
        let title = label
            .map(str::to_string)
            .unwrap_or_else(|| resolved.heading_text.clone());
        (target_id, title)
    } else {
        let clean_id = raw.strip_prefix("id:").unwrap_or(raw);
        let title = label
            .map(str::to_string)
            .unwrap_or_else(|| clean_id.to_string());
        (clean_id.to_string(), title)
    }
}

// endregion

// region Span & Duplicate Detection

/// Determines the bounding line range for the mutation target within `doc`.
fn determine_target_span(
    doc: &OrgDocument,
    resolved: &ResolvedEntry,
    total_lines: usize,
) -> TargetSpan {
    if resolved.entry_index == FILE_NODE_ENTRY_INDEX {
        let end = doc
            .entries
            .first()
            .map(|e| e.heading_line.saturating_sub(1))
            .unwrap_or(total_lines);
        TargetSpan {
            start: 0,
            content_end: end,
            subtree_end: total_lines,
            level: 0,
        }
    } else {
        let entry = &doc.entries[resolved.entry_index];
        let start = entry.heading_line.saturating_sub(1);
        let content_end = entry.content_end_line.saturating_sub(1).min(total_lines);
        let subtree_end = entry.subtree_end_line.saturating_sub(1).min(total_lines);
        TargetSpan {
            start,
            content_end,
            subtree_end,
            level: entry.level,
        }
    }
}

/// Checks whether `lines` inside `span` already contain an outbound link to `target_id`.
fn target_span_has_link(
    lines: &[&str],
    span: &TargetSpan,
    target_id: &str,
    sec_prefix: Option<&str>,
) -> bool {
    let needle = format!("[[id:{target_id}");
    if let Some(sec_hdr) = sec_prefix {
        let mut in_sec = false;
        for line in &lines[span.start..span.subtree_end] {
            let trimmed = line.trim();
            if trimmed.starts_with(sec_hdr) {
                in_sec = true;
                continue;
            }
            if in_sec {
                if trimmed.starts_with('*') {
                    break;
                }
                if line.contains(&needle) {
                    return true;
                }
            }
        }
        false
    } else {
        let end = span.content_end.max(span.start).min(lines.len());
        lines[span.start..end]
            .iter()
            .any(|line| line.contains(&needle))
    }
}

// endregion

// region Insertion Point & Splice

/// Finds the insertion line and formatted text block for `--section` insertion.
fn find_section_insertion(
    lines: &[&str],
    span: &TargetSpan,
    section: &str,
    link_bullet: &str,
) -> (usize, String) {
    let stars = "*".repeat(span.level + 1);
    let sec_heading = format!("{stars} {section}");

    let mut found_line = None;
    for (i, line) in lines[span.start..span.subtree_end].iter().enumerate() {
        if line.trim().starts_with(&sec_heading) {
            found_line = Some(span.start + i);
            break;
        }
    }

    if let Some(sec_idx) = found_line {
        let mut insert_at = span.subtree_end;
        for (j, line) in lines[sec_idx + 1..span.subtree_end].iter().enumerate() {
            if line.trim().starts_with('*') {
                insert_at = sec_idx + 1 + j;
                break;
            }
        }
        (insert_at, format!("{link_bullet}\n"))
    } else {
        let prefix = if span.subtree_end > 0 && !lines[span.subtree_end - 1].trim().is_empty() {
            "\n"
        } else {
            ""
        };
        let text = format!("{prefix}{sec_heading}\n{link_bullet}\n");
        (span.subtree_end, text)
    }
}

/// Finds the 0-based insertion line and text for adding a link to `span`.
fn plan_link_insertion(
    lines: &[&str],
    span: &TargetSpan,
    section: Option<&str>,
    link_bullet: &str,
) -> (usize, String) {
    if let Some(sec) = section {
        find_section_insertion(lines, span, sec, link_bullet)
    } else {
        let mut insert_line = span.content_end;
        while insert_line > span.start && lines[insert_line - 1].trim().is_empty() {
            insert_line -= 1;
        }
        (insert_line, format!("{link_bullet}\n"))
    }
}

/// Splices an inserted block of lines at `insert_line` into `lines`.
fn splice_link_insertion(
    lines: &[&str],
    insert_line: usize,
    text: &str,
    ends_with_nl: bool,
) -> String {
    let mut out = String::new();
    let clamped = insert_line.min(lines.len());
    for line in &lines[..clamped] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
    for line in &lines[clamped..] {
        out.push_str(line);
        out.push('\n');
    }
    if !ends_with_nl && out.ends_with('\n') {
        out.pop();
    }
    out
}

// endregion

// region Execution Entrypoint

/// Executes the mutation write and cache invalidation when changed.
fn apply_link_mutation(
    resolved_file: &PathBuf,
    content: &str,
    changed: bool,
    dry_run: bool,
    workspace: Option<&Path>,
) -> Result<(), i32> {
    if !changed || dry_run {
        return Ok(());
    }
    if let Err(e) = org_tools_core::files::write_file_atomic(resolved_file, content) {
        eprintln!("org: error writing {}: {e}", resolved_file.display());
        return Err(EXIT_ERROR);
    }
    invalidate_warm_workspace_caches(workspace, std::slice::from_ref(resolved_file));
    Ok(())
}

/// Runs `org update link`.
pub(crate) fn run_link(args: LinkArgs) -> i32 {
    let started = Instant::now();
    let (resolved, source, doc) =
        match resolve_single_target_with_workspace(&args.locator, args.workspace.as_deref()) {
            Ok(triple) => triple,
            Err(code) => return code,
        };

    let (target_id, target_title) =
        resolve_destination_target(&args.to, args.label.as_deref(), args.workspace.as_deref());
    let link_bullet = format!("- [[id:{target_id}][{target_title}]]");

    let lines: Vec<&str> = source.content.lines().collect();
    let span = determine_target_span(&doc, &resolved, lines.len());
    let sec_prefix = args
        .section
        .as_ref()
        .map(|s| format!("{} {s}", "*".repeat(span.level + 1)));

    let has_link = target_span_has_link(&lines, &span, &target_id, sec_prefix.as_deref());
    let (content, changed) = if has_link {
        (source.content.clone(), false)
    } else {
        let (insert_at, block) =
            plan_link_insertion(&lines, &span, args.section.as_deref(), &link_bullet);
        let spliced =
            splice_link_insertion(&lines, insert_at, &block, source.content.ends_with('\n'));
        let diff = spliced != source.content;
        (spliced, diff)
    };

    if let Err(code) = apply_link_mutation(
        &resolved.file,
        &content,
        changed,
        args.dry_run,
        args.workspace.as_deref(),
    ) {
        return code;
    }

    let mut mutation = MutationResult::new("link", args.dry_run);
    if changed {
        mutation.files_modified = 1;
        mutation.entries_affected = 1;
    }
    mutation.details.push(MutationDetail {
        file: resolved.file.display().to_string(),
        line: resolved.line,
        action: "link".to_string(),
        detail: Some(format!("to id:{target_id}")),
    });
    mutation.finish(started, 1);
    output_mutation(&mutation, args.format);
    EXIT_OK
}

// endregion
