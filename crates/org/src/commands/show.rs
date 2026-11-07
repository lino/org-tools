// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org show` subcommand handler for bounded subtree and file-node extraction.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use clap::{Args, ValueEnum};
use serde::Serialize;

use org_tools_core::document::{ChecklistItem, ChecklistSummary};
use org_tools_core::export_md::{
    export_slice_to_markdown, extract_org_slice, resolve_workspace_locator,
    scan_file_node_checklists, truncate_unicode_chars, ExtractedOrgSlice, ResolvedWorkspaceNode,
};
use org_tools_core::locator::{locator_for_entry, OrgLocator, FILE_NODE_ENTRY_INDEX};
use org_tools_core::rules::timestamp::OrgTimestamp;

use super::MutationTelemetry;
use crate::{EXIT_ERROR, EXIT_OK};

/// Output format for `org show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ShowOutputFormat {
    /// Raw Org-mode slice.
    Org,
    /// Structured JSON envelope matching `org-show-output.schema.json`.
    Json,
    /// Zensical-ready GitHub Flavored Markdown.
    Md,
}

/// CLI arguments for `org show`.
#[derive(Args)]
pub(crate) struct ShowArgs {
    /// Org locator (`id:<uuid>`, `<file>::#<custom-id>`, `<file>::*/Path`, or `<file>::<line>`).
    pub(crate) locator: String,

    /// Workspace directory to scan when resolving `id:` locators and cross-node links.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Output format (`org`, `json`, or `md`).
    #[arg(long, value_enum, default_value = "org")]
    pub(crate) format: ShowOutputFormat,

    /// Stop before descendant headings (or before the first `*` heading for file nodes).
    #[arg(long)]
    pub(crate) no_children: bool,

    /// Maximum number of Unicode scalar values to emit in the extracted content.
    #[arg(long)]
    pub(crate) max_chars: Option<usize>,

    /// Include rendered `markdown` alongside `content` in `--format json` output.
    #[arg(long)]
    pub(crate) include_markdown: bool,
}

/// Planning timestamps serialized in `org show --format json`.
#[derive(Default, Serialize)]
struct ShowPlanningJson {
    /// Formatted `SCHEDULED:` timestamp or `null`.
    scheduled: Option<String>,
    /// Formatted `DEADLINE:` timestamp or `null`.
    deadline: Option<String>,
    /// Formatted `CLOSED:` timestamp or `null`.
    closed: Option<String>,
}

/// JSON output envelope strictly conforming to `schemas/org-show-output.schema.json`.
#[derive(Serialize)]
struct ShowJsonEnvelope {
    /// Canonical locator string for the resolved node.
    locator: String,
    /// Path to the Org source file containing the resolved node.
    file: String,
    /// One-based start line number of the resolved node.
    line: usize,
    /// One-based inclusive end line number of the extracted slice.
    end_line: usize,
    /// Outline level (`0` for file nodes, `>= 1` for headings).
    level: usize,
    /// Heading title or level-0 `#+TITLE:` / file stem.
    title: String,
    /// Heading TODO keyword, if present.
    keyword: Option<String>,
    /// Heading priority cookie character, if present.
    priority: Option<String>,
    /// Local heading tags or `#+FILETAGS:` for file nodes.
    tags: Vec<String>,
    /// Node property drawer key-value map.
    properties: HashMap<String, String>,
    /// Planning timestamps attached to the heading.
    planning: ShowPlanningJson,
    /// Aggregate checklist completion counts.
    checklist_summary: ChecklistSummary,
    /// Plain-list checklist items in the node's own body.
    checklist: Vec<ChecklistItem>,
    /// Whether child headings were omitted via `--no-children`.
    no_children: bool,
    /// Whether `content` was truncated to honour `--max-chars`.
    truncated: bool,
    /// Unicode scalar count of `content`.
    char_count: usize,
    /// Bounded raw Org slice for the resolved node.
    content: String,
    /// Optional bounded Markdown slice when `--include-markdown` is requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown: Option<String>,
    /// Execution duration and file-scan observability telemetry.
    telemetry: MutationTelemetry,
}

/// Node-specific metadata shared across level-0 file-node and heading JSON builders.
struct NodeMetadata {
    /// Canonical locator string.
    locator: String,
    /// Outline level (`0` or `>= 1`).
    level: usize,
    /// Node title text.
    title: String,
    /// Optional TODO keyword.
    keyword: Option<String>,
    /// Optional priority cookie string.
    priority: Option<String>,
    /// Node tags.
    tags: Vec<String>,
    /// Node property drawer map.
    properties: HashMap<String, String>,
    /// Planning timestamps.
    planning: ShowPlanningJson,
    /// Node plain-list checklist items.
    checklist: Vec<ChecklistItem>,
}

/// Formats an [`OrgTimestamp`] as `YYYY-MM-DD` or `YYYY-MM-DD HH:MM`.
fn format_planning_ts(ts: &OrgTimestamp) -> String {
    match (ts.hour, ts.minute) {
        (Some(h), Some(m)) => format!("{:04}-{:02}-{:02} {h:02}:{m:02}", ts.year, ts.month, ts.day),
        _ => format!("{:04}-{:02}-{:02}", ts.year, ts.month, ts.day),
    }
}

/// Extracts [`NodeMetadata`] for either a level-0 file node or a heading entry.
fn extract_node_metadata(loaded: &ResolvedWorkspaceNode) -> NodeMetadata {
    if loaded.resolved.is_file_node() {
        return NodeMetadata {
            locator: locator_for_entry(&loaded.doc, FILE_NODE_ENTRY_INDEX).to_string(),
            level: 0,
            title: loaded.resolved.heading_text.clone(),
            keyword: None,
            priority: None,
            tags: loaded.doc.filetags.clone(),
            properties: loaded.doc.file_properties.clone(),
            planning: ShowPlanningJson::default(),
            checklist: scan_file_node_checklists(&loaded.source, &loaded.doc),
        };
    }

    let entry = &loaded.doc.entries[loaded.resolved.entry_index];
    NodeMetadata {
        locator: locator_for_entry(&loaded.doc, loaded.resolved.entry_index).to_string(),
        level: entry.level,
        title: entry.title.clone(),
        keyword: entry.keyword.clone(),
        priority: entry.priority.map(|c| c.to_string()),
        tags: entry.tags.clone(),
        properties: entry.properties.clone(),
        planning: ShowPlanningJson {
            scheduled: entry.planning.scheduled.as_ref().map(format_planning_ts),
            deadline: entry.planning.deadline.as_ref().map(format_planning_ts),
            closed: entry.planning.closed.as_ref().map(format_planning_ts),
        },
        checklist: entry.checklist.clone(),
    }
}

/// Builds the JSON response envelope for `org show --format json`.
fn build_json_envelope(
    loaded: &ResolvedWorkspaceNode,
    args: &ShowArgs,
    slice: ExtractedOrgSlice<'_>,
    started: Instant,
) -> ShowJsonEnvelope {
    let (content, truncated, char_count) = truncate_unicode_chars(slice.raw, args.max_chars);
    let markdown = args.include_markdown.then(|| {
        let md = export_slice_to_markdown(slice.raw, &loaded.doc.file, &loaded.workspace_docs);
        truncate_unicode_chars(&md, args.max_chars).0
    });
    let meta = extract_node_metadata(loaded);
    let checklist_summary = ChecklistSummary::from_items(&meta.checklist);

    ShowJsonEnvelope {
        locator: meta.locator,
        file: loaded.doc.file.display().to_string(),
        line: slice.start_line,
        end_line: slice.end_line,
        level: meta.level,
        title: meta.title,
        keyword: meta.keyword,
        priority: meta.priority,
        tags: meta.tags,
        properties: meta.properties,
        planning: meta.planning,
        checklist_summary,
        checklist: meta.checklist,
        no_children: args.no_children,
        truncated,
        char_count,
        content,
        markdown,
        telemetry: MutationTelemetry {
            duration_us: started.elapsed().as_micros() as u64,
            files_scanned: loaded.files_scanned,
            cache_hits: loaded.cache_hits,
            cache_misses: loaded.cache_misses,
        },
    }
}

/// Executes `org show <locator>` and returns the process exit status code.
pub(crate) fn run_show(args: ShowArgs) -> i32 {
    let started = Instant::now();
    let locator = match OrgLocator::parse(&args.locator) {
        Ok(loc) => loc,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let need_ws_docs = args.format == ShowOutputFormat::Md || args.include_markdown;
    let loaded =
        match resolve_workspace_locator(&locator, args.workspace.as_deref(), &cwd, need_ws_docs) {
            Ok(target) => target,
            Err(err) => {
                eprintln!("org: {err}");
                return EXIT_ERROR;
            }
        };

    let slice = extract_org_slice(
        &loaded.source,
        &loaded.doc,
        &loaded.resolved,
        args.no_children,
    );

    match args.format {
        ShowOutputFormat::Org => {
            print!("{}", truncate_unicode_chars(slice.raw, args.max_chars).0);
        }
        ShowOutputFormat::Md => {
            let md = export_slice_to_markdown(slice.raw, &loaded.doc.file, &loaded.workspace_docs);
            print!("{}", truncate_unicode_chars(&md, args.max_chars).0);
        }
        ShowOutputFormat::Json => {
            let envelope = build_json_envelope(&loaded, &args, slice, started);
            println!(
                "{}",
                serde_json::to_string_pretty(&envelope).unwrap_or_default()
            );
        }
    }

    EXIT_OK
}
