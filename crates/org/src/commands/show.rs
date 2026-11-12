// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand handler for `org show` bounded subtree and file-node extraction.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Instant;

use clap::{Args, ValueEnum, ValueHint};
use serde::Serialize;

use org_tools_core::config::Config;
use org_tools_core::document::{ChecklistItem, ChecklistSummary};
use org_tools_core::export_md::{
    export_slice_to_markdown, extract_org_slice, resolve_workspace_locator,
    scan_file_node_checklists, truncate_unicode_chars, ExtractedOrgSlice, ResolvedWorkspaceNode,
};
use org_tools_core::include::expand_includes;
use org_tools_core::locator::{locator_for_entry, OrgLocator, FILE_NODE_ENTRY_INDEX};
use org_tools_core::rules::timestamp::OrgTimestamp;

use super::update::read_stdin_targets;
use super::MutationTelemetry;
use crate::query;
use crate::query::output::QueryLinkJson;
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
    /// Org locator (`id:<uuid>`, `<file>::#<custom-id>`, `<file>::*/Path`, or `<file>::<line>`); read from `stdin` if omitted.
    pub(crate) locator: Option<String>,

    /// Workspace directory to scan when resolving `id:` locators and cross-node links.
    #[arg(long, value_hint = ValueHint::DirPath)]
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

    /// Expand `#+INCLUDE:` directives before rendering output.
    #[arg(long)]
    pub(crate) expand_includes: bool,

    /// Optional fields to include in output.
    #[arg(long, value_delimiter = ',')]
    pub(crate) include: Option<Vec<String>>,

    /// Exclude property keys from canonical digest calculation.
    #[arg(long, value_delimiter = ',')]
    pub(crate) exclude_properties: Option<Vec<String>>,
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
    /// File-level tags from #+FILETAGS: for file nodes or inherited filetags.
    #[serde(skip_serializing_if = "Option::is_none")]
    filetags: Option<Vec<String>>,
    /// Inherited effective tags.
    #[serde(skip_serializing_if = "Option::is_none")]
    effective_tags: Option<Vec<String>>,
    /// Top-of-body vulpea-meta description list items.
    #[serde(skip_serializing_if = "Option::is_none")]
    vulpea_meta: Option<HashMap<String, Vec<String>>>,
    /// Outbound links detected in the node.
    #[serde(skip_serializing_if = "Option::is_none")]
    links: Option<Vec<QueryLinkJson>>,
    /// Clean body text with drawers and planning stripped.
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    /// Canonical SHA-256 digest of the node/subtree.
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    /// Plain-list checklist items alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    checkboxes: Option<Vec<ChecklistItem>>,
    /// Execution duration and file-scan observability telemetry.
    telemetry: MutationTelemetry,
    /// Optional resolved scope constraint.
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
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

    let empty_exclude: Vec<String> = Vec::new();
    let ex_props = args.exclude_properties.as_ref().unwrap_or(&empty_exclude);
    let digest = Some(query::rich::compute_canonical_digest(slice.raw, ex_props));
    let is_file = loaded.resolved.is_file_node();
    let body = Some(query::rich::extract_clean_body(slice.raw, is_file));
    let links = Some(query::rich::extract_links(slice.raw, slice.start_line));
    let checkboxes = Some(meta.checklist.clone());

    let (filetags, effective_tags, vulpea_meta) = if is_file {
        (
            Some(loaded.doc.filetags.clone()),
            Some(loaded.doc.filetags.clone()),
            Some(loaded.doc.vulpea_meta.clone()),
        )
    } else {
        (
            Some(loaded.doc.filetags.clone()),
            Some(loaded.doc.effective_tags(loaded.resolved.entry_index)),
            Some(
                loaded.doc.entries[loaded.resolved.entry_index]
                    .vulpea_meta
                    .clone(),
            ),
        )
    };

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
        filetags,
        effective_tags,
        vulpea_meta,
        links,
        body,
        digest,
        checkboxes,
        telemetry: MutationTelemetry {
            duration_us: started.elapsed().as_micros() as u64,
            files_scanned: loaded.files_scanned,
            cache_hits: loaded.cache_hits,
            cache_misses: loaded.cache_misses,
        },
        scope: {
            let mut s =
                org_tools_core::scope::resolve_node_scope(&loaded.doc, loaded.resolved.entry_index);
            if s.is_none()
                && args
                    .include
                    .as_ref()
                    .is_some_and(|inc| inc.iter().any(|item| item.eq_ignore_ascii_case("scope")))
            {
                s = Some("global".to_string());
            }
            s
        },
    }
}

/// Renders `slice` in the requested `ShowOutputFormat` (`org`, `md`, or `json`).
fn emit_show_output(
    loaded: &ResolvedWorkspaceNode,
    args: &ShowArgs,
    slice: ExtractedOrgSlice<'_>,
    started: Instant,
) {
    match args.format {
        ShowOutputFormat::Org => {
            print!("{}", truncate_unicode_chars(slice.raw, args.max_chars).0);
        }
        ShowOutputFormat::Md => {
            let md = export_slice_to_markdown(slice.raw, &loaded.doc.file, &loaded.workspace_docs);
            print!("{}", truncate_unicode_chars(&md, args.max_chars).0);
        }
        ShowOutputFormat::Json => {
            let envelope = build_json_envelope(loaded, args, slice, started);
            println!(
                "{}",
                serde_json::to_string_pretty(&envelope).unwrap_or_default()
            );
        }
    }
}

/// Resolves the target locator string from CLI arguments or piped `stdin`.
fn resolve_show_locator_str(locator_opt: Option<&str>) -> Result<String, String> {
    if let Some(loc) = locator_opt.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(loc.to_string());
    }
    if !std::io::stdin().is_terminal() {
        let targets = read_stdin_targets()?;
        return targets
            .into_iter()
            .next()
            .ok_or_else(|| "no target locator found in stdin".to_string());
    }
    Err("missing target locator (supply <locator> or pipe via stdin)".to_string())
}

/// Resolves `locator` against `workspace`, `cwd`, or configured `[roam] directories`.
fn resolve_show_node(
    locator: &OrgLocator,
    workspace: Option<&std::path::Path>,
    cwd: &std::path::Path,
    need_ws_docs: bool,
) -> Result<ResolvedWorkspaceNode, String> {
    match resolve_workspace_locator(locator, workspace, cwd, need_ws_docs) {
        Ok(target) => Ok(target),
        Err(primary_err) => {
            if workspace.is_none() {
                if let Ok(cfg) = Config::load_for_dir(cwd) {
                    for (_, vault) in cfg.roam_directories() {
                        if vault.is_dir() {
                            if let Ok(target) =
                                resolve_workspace_locator(locator, Some(&vault), cwd, need_ws_docs)
                            {
                                return Ok(target);
                            }
                        }
                    }
                }
            }
            Err(primary_err)
        }
    }
}

/// Executes `org show <locator>` and returns the process exit status code.
pub(crate) fn run_show(args: ShowArgs) -> i32 {
    let started = Instant::now();
    let raw_locator = match resolve_show_locator_str(args.locator.as_deref()) {
        Ok(loc) => loc,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    let locator = match OrgLocator::parse(&raw_locator) {
        Ok(loc) => loc,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let need_ws_docs = args.format == ShowOutputFormat::Md || args.include_markdown;
    let loaded = match resolve_show_node(&locator, args.workspace.as_deref(), &cwd, need_ws_docs) {
        Ok(target) => target,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    let mut slice = extract_org_slice(
        &loaded.source,
        &loaded.doc,
        &loaded.resolved,
        args.no_children,
    );
    let expanded_buf;
    if args.expand_includes {
        match expand_includes(slice.raw, &loaded.doc.file, args.workspace.as_deref()) {
            Ok(expanded) => {
                expanded_buf = expanded;
                slice.raw = &expanded_buf;
            }
            Err(err) => {
                eprintln!("org: {err}");
                return EXIT_ERROR;
            }
        }
    }
    emit_show_output(&loaded, &args, slice, started);
    EXIT_OK
}
