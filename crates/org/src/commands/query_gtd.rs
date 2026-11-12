// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! GTD workflow query views (`next`, `blocked`, `inbox`, `waiting`, `stuck`) for
//! `org query` and top-level `org next`.

use std::path::PathBuf;

use clap::{Args, ValueEnum, ValueHint};

use org_tools_core::document::{OrgDocument, OrgEntry};
use org_tools_core::gtd_roam::{parse_effort_minutes, NextGroupBy, NextQueryFilter};
use org_tools_core::locator::locator_for_entry;

use super::query::{collect_predicate_matches, render_standard_matches, QueryOutputFormat};
pub(crate) use crate::query::next_render::resolve_gtd_scan_paths;
use crate::query::next_render::{
    build_next_json_entries, collect_ranked_next_candidates, load_gtd_docs,
    render_next_grouped_human, render_pick_human, LoadedGtdDoc, NextCandidate,
};
use crate::query::output::{JsonEntry, MatchedEntry, StuckProject};
use crate::{date, query, EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

// region CLI Types

/// CLI grouping selector for `org [query] next --group-by`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub(crate) enum NextGroupByCli {
    /// Group by `@context` tag or `:CONTEXT:` property.
    #[default]
    Context,
    /// Group by resolved owning GTD project.
    Project,
    /// Group by configured `org-roam` vault name.
    Vault,
    /// Group by priority cookie (`[#A]`, `[#B]`, `[#C]`).
    Priority,
    /// Group by `:ENERGY:` property.
    Energy,
    /// Group by `:WORKMODE:` property.
    Workmode,
    /// Flat ungrouped list in urgency order.
    None,
}

impl From<NextGroupByCli> for NextGroupBy {
    fn from(v: NextGroupByCli) -> Self {
        match v {
            NextGroupByCli::Context => Self::Context,
            NextGroupByCli::Project => Self::Project,
            NextGroupByCli::Vault => Self::Vault,
            NextGroupByCli::Priority => Self::Priority,
            NextGroupByCli::Energy => Self::Energy,
            NextGroupByCli::Workmode => Self::Workmode,
            NextGroupByCli::None => Self::None,
        }
    }
}

/// Shared CLI arguments for `org query blocked|inbox|waiting|stuck`.
#[derive(Args, Debug, Clone)]
pub(crate) struct GtdViewArgs {
    /// Files or directories to scan (defaults to configured `org-roam` vaults or `.`).
    #[arg(value_hint = ValueHint::AnyPath)]
    pub(crate) paths: Vec<PathBuf>,
    /// Repeatable named `[roam.directories]` vault or directory path selector.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Vec<String>,
    /// Scan only `#+filetags: :agenda:` files and base `<gtd_subdir>/` files.
    #[arg(long)]
    pub(crate) agenda_only: bool,
    /// Output format (`human`, `json`, or `locator`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: QueryOutputFormat,
    /// Maximum results to return.
    #[arg(long)]
    pub(crate) limit: Option<usize>,
}

/// CLI arguments for `org query next` and top-level `org next`.
#[derive(Args, Debug, Clone)]
pub(crate) struct NextArgs {
    /// Files or directories to scan (defaults to configured `org-roam` vaults or `.`).
    #[arg(value_hint = ValueHint::AnyPath)]
    pub(crate) paths: Vec<PathBuf>,
    /// Repeatable named `[roam.directories]` vault or directory path selector.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Vec<String>,
    /// Scan only `#+filetags: :agenda:` files and base `<gtd_subdir>/` files.
    #[arg(long)]
    pub(crate) agenda_only: bool,
    /// Filter by owning GTD project title, `:ID:`, alias, or file stem.
    #[arg(long, short = 'p')]
    pub(crate) project: Option<String>,
    /// Filter to a specific `@context` tag or `:CONTEXT:` property.
    #[arg(long, short = 'c')]
    pub(crate) context: Option<String>,
    /// Filter by `:ENERGY:` level (`low`, `medium`, `high`).
    #[arg(long, short = 'e')]
    pub(crate) energy: Option<String>,
    /// Filter by `:WORKMODE:` (`deep`, `shallow`, `any`).
    #[arg(long, short = 'w')]
    pub(crate) workmode: Option<String>,
    /// Maximum `:EFFORT:` duration (`H:MM`, `MMm`, or integer minutes).
    #[arg(long)]
    pub(crate) max_effort: Option<String>,
    /// Minimum `:EFFORT:` duration (`H:MM`, `MMm`, or integer minutes).
    #[arg(long)]
    pub(crate) min_effort: Option<String>,
    /// Priority filter (`A`, `B`, `C`, or `>=B`).
    #[arg(long)]
    pub(crate) priority: Option<String>,
    /// Repeatable required tag filter.
    #[arg(long = "tag")]
    pub(crate) tags: Vec<String>,
    /// Filter by `:GOAL:` property substring or ID.
    #[arg(long)]
    pub(crate) goal: Option<String>,
    /// Filter by planning due date (`overdue`, `today`, `soon`, or `YYYY-MM-DD`).
    #[arg(long)]
    pub(crate) due: Option<String>,
    /// Only return entries whose keyword is explicitly `NEXT`.
    #[arg(long)]
    pub(crate) next_only: bool,
    /// Include entries scheduled in the future (`SCHEDULED: > today`).
    #[arg(long)]
    pub(crate) include_future: bool,
    /// Grouping dimension (`context`, `project`, `vault`, `priority`, `energy`, `workmode`, `none`).
    #[arg(long, value_enum, default_value_t = NextGroupByCli::Context)]
    pub(crate) group_by: NextGroupByCli,
    /// Recommend the single highest-ranked next action (or top `--limit`).
    #[arg(long)]
    pub(crate) pick: bool,
    /// Output format (`human`, `json`, or `locator`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: QueryOutputFormat,
    /// Maximum results to return.
    #[arg(long)]
    pub(crate) limit: Option<usize>,
}

// endregion

// region Next Navigator Execution

/// Builds a [`NextQueryFilter`] from CLI [`NextArgs`], validating effort durations.
fn build_next_filter(args: &NextArgs, warning_days: u32) -> Result<NextQueryFilter, String> {
    let max_effort_mins = match args.max_effort.as_deref() {
        Some(raw) => Some(
            parse_effort_minutes(raw)
                .ok_or_else(|| format!("invalid --max-effort duration '{raw}'"))?,
        ),
        None => None,
    };
    let min_effort_mins = match args.min_effort.as_deref() {
        Some(raw) => Some(
            parse_effort_minutes(raw)
                .ok_or_else(|| format!("invalid --min-effort duration '{raw}'"))?,
        ),
        None => None,
    };
    Ok(NextQueryFilter {
        project: args.project.clone(),
        context: args.context.clone(),
        energy: args.energy.clone(),
        workmode: args.workmode.clone(),
        max_effort_mins,
        min_effort_mins,
        priority: args.priority.clone(),
        tags: args.tags.clone(),
        goal: args.goal.clone(),
        due: args.due.clone(),
        next_only: args.next_only,
        include_future: args.include_future,
        warning_days,
    })
}

/// Evaluates `args` and returns ranked [`NextCandidate`] items.
fn collect_ranked_next<'a>(
    loaded: &'a [LoadedGtdDoc],
    doc_refs: &[&'a OrgDocument],
    args: &NextArgs,
) -> Result<Vec<NextCandidate<'a>>, String> {
    let warn_days = loaded.first().map_or(14, |d| d.warning_days);
    let filter = build_next_filter(args, warn_days)?;
    let cap = if args.pick {
        Some(args.limit.unwrap_or(1))
    } else {
        args.limit
    };
    Ok(collect_ranked_next_candidates(
        loaded,
        doc_refs,
        &filter,
        date::current_date(),
        cap,
    ))
}

/// Evaluates `args` and returns enriched [`JsonEntry`] items (reusable by `org_query_next` MCP tool).
#[allow(dead_code)]
pub(crate) fn evaluate_next_json_entries(args: &NextArgs) -> Result<Vec<JsonEntry>, String> {
    let loaded = load_gtd_docs(&args.paths, &args.roam_dir, args.agenda_only)?;
    let doc_refs: Vec<&OrgDocument> = loaded.iter().map(|d| &d.doc).collect();
    let candidates = collect_ranked_next(&loaded, &doc_refs, args)?;
    Ok(build_next_json_entries(&candidates, &doc_refs))
}

/// Executes `org query next` and top-level `org next`.
pub(crate) fn run_next(args: NextArgs) -> i32 {
    let loaded = match load_gtd_docs(&args.paths, &args.roam_dir, args.agenda_only) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("org: {msg}");
            return EXIT_ERROR;
        }
    };
    let doc_refs: Vec<&OrgDocument> = loaded.iter().map(|d| &d.doc).collect();
    let candidates = match collect_ranked_next(&loaded, &doc_refs, &args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("org: {msg}");
            return EXIT_ERROR;
        }
    };
    if candidates.is_empty() {
        eprintln!("org: no actionable entries found");
        return EXIT_ISSUES;
    }
    match args.format {
        QueryOutputFormat::Human => {
            let rendered = if args.pick {
                render_pick_human(&candidates)
            } else {
                render_next_grouped_human(&candidates, args.group_by.into())
            };
            print!("{rendered}");
        }
        QueryOutputFormat::Json => {
            let items = build_next_json_entries(&candidates, &doc_refs);
            println!(
                "{}",
                serde_json::to_string_pretty(&items).unwrap_or_default()
            );
        }
        QueryOutputFormat::Locator => {
            for c in &candidates {
                println!("{}", locator_for_entry(&c.loaded.doc, c.entry_idx));
            }
        }
    }
    EXIT_OK
}

// endregion

// region GTD View Execution (`blocked`, `inbox`, `waiting`, `stuck`)

/// Executes `org query blocked`.
pub(crate) fn run_blocked(args: GtdViewArgs) -> i32 {
    let Ok(loaded) = load_gtd_docs(&args.paths, &args.roam_dir, args.agenda_only).map_err(|e| {
        eprintln!("org: {e}");
    }) else {
        return EXIT_ERROR;
    };
    let docs: Vec<OrgDocument> = loaded.into_iter().map(|d| d.doc).collect();
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let matches = collect_predicate_matches(
        &query::parser::Predicate::Blocked,
        &docs,
        &doc_refs,
        date::current_date(),
        args.limit,
    );
    if matches.is_empty() {
        eprintln!("org: no blocked entries found");
        return EXIT_ISSUES;
    }
    match args.format {
        QueryOutputFormat::Human => {
            print!(
                "{}",
                query::output::render_blocked_human(&matches, &doc_refs)
            )
        }
        QueryOutputFormat::Json => {
            print!(
                "{}",
                query::output::render_blocked_json(&matches, &doc_refs)
            )
        }
        QueryOutputFormat::Locator => print!("{}", query::output::render_locators(&matches)),
    }
    EXIT_OK
}

/// Returns `true` when `entry` is an inbox task (either in `inbox.org` or untagged/unscheduled active TODO).
fn is_inbox_entry(doc: &OrgDocument, entry_idx: usize, entry: &OrgEntry) -> bool {
    let Some(ref kw) = entry.keyword else {
        return false;
    };
    if doc.todo_keywords.is_done(kw) {
        return false;
    }
    let in_inbox_file = doc
        .file
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|f| f.eq_ignore_ascii_case("inbox.org"));
    if in_inbox_file {
        return true;
    }
    let non_agenda_tags: Vec<_> = doc
        .inherited_tags(entry_idx)
        .into_iter()
        .filter(|t| !t.eq_ignore_ascii_case("agenda"))
        .collect();
    non_agenda_tags.is_empty()
        && entry.planning.scheduled.is_none()
        && entry.planning.deadline.is_none()
}

/// Executes `org query inbox`.
pub(crate) fn run_inbox(args: GtdViewArgs) -> i32 {
    let Ok(loaded) = load_gtd_docs(&args.paths, &args.roam_dir, args.agenda_only).map_err(|e| {
        eprintln!("org: {e}");
    }) else {
        return EXIT_ERROR;
    };
    let docs: Vec<OrgDocument> = loaded.into_iter().map(|d| d.doc).collect();
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let mut matches = Vec::new();
    for doc in &docs {
        for (idx, entry) in doc.entries.iter().enumerate() {
            if is_inbox_entry(doc, idx, entry) {
                matches.push(MatchedEntry {
                    doc,
                    entry_idx: idx,
                });
            }
        }
    }
    if let Some(n) = args.limit {
        matches.truncate(n);
    }
    if matches.is_empty() {
        eprintln!("org: inbox is empty");
        return EXIT_ISSUES;
    }
    render_standard_matches(&matches, &doc_refs, args.format);
    EXIT_OK
}

/// Executes `org query waiting`.
pub(crate) fn run_waiting(args: GtdViewArgs) -> i32 {
    let Ok(loaded) = load_gtd_docs(&args.paths, &args.roam_dir, args.agenda_only).map_err(|e| {
        eprintln!("org: {e}");
    }) else {
        return EXIT_ERROR;
    };
    let docs: Vec<OrgDocument> = loaded.into_iter().map(|d| d.doc).collect();
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let matches = collect_predicate_matches(
        &query::parser::Predicate::Waiting,
        &docs,
        &doc_refs,
        date::current_date(),
        args.limit,
    );
    if matches.is_empty() {
        eprintln!("org: no waiting entries found");
        return EXIT_ISSUES;
    }
    match args.format {
        QueryOutputFormat::Human => print!("{}", query::output::render_waiting_human(&matches)),
        QueryOutputFormat::Json => print!("{}", query::output::render_json(&matches, &doc_refs)),
        QueryOutputFormat::Locator => print!("{}", query::output::render_locators(&matches)),
    }
    EXIT_OK
}

/// Executes `org query stuck`.
pub(crate) fn run_stuck(args: GtdViewArgs) -> i32 {
    let Ok(loaded) = load_gtd_docs(&args.paths, &args.roam_dir, args.agenda_only).map_err(|e| {
        eprintln!("org: {e}");
    }) else {
        return EXIT_ERROR;
    };
    let docs: Vec<OrgDocument> = loaded.into_iter().map(|d| d.doc).collect();
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let all_stuck = query::output::find_stuck_projects(&docs, &doc_refs, date::current_date());
    let slice_len = args
        .limit
        .map_or(all_stuck.len(), |n| all_stuck.len().min(n));
    let stuck: &[StuckProject<'_>] = &all_stuck[..slice_len];
    if stuck.is_empty() {
        eprintln!("org: no stuck projects found");
        return EXIT_ISSUES;
    }
    match args.format {
        QueryOutputFormat::Human => print!("{}", query::output::render_stuck_human(stuck)),
        QueryOutputFormat::Json => print!("{}", query::output::render_stuck_json(stuck)),
        QueryOutputFormat::Locator => {
            for sp in stuck {
                println!("{}", locator_for_entry(sp.doc, sp.entry_idx));
            }
        }
    }
    EXIT_OK
}

// endregion
