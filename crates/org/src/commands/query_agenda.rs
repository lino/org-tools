// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Command-line arguments, configuration/cache integration, and reusable execution
//! helpers for `org query agenda` shared with MCP tools.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, ValueEnum, ValueHint};

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::export_md::normalize_workspace_path;
use org_tools_core::files::{collect_org_files, write_file_atomic};
use org_tools_core::rules::timestamp::is_valid_date;
use org_tools_core::source::SourceFile;

use crate::commands::MutationTelemetry;
use crate::query::agenda::{build_agenda_with_options, AgendaBuildOptions, AgendaDay};
use crate::query::agenda_render::{
    build_agenda_json_value, collect_composite_agenda, render_agenda_doc, render_agenda_locators,
    render_agenda_report_human, CompositeAgenda,
};
use crate::{date, EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

// region CLI & Reusable Options Types

/// Default number of calendar days projected by `org query agenda`.
pub(crate) const DEFAULT_AGENDA_DAYS: usize = 7;

/// Expected ASCII digit count of the `YYYY` year segment in an ISO `YYYY-MM-DD` date.
const ISO_YEAR_DIGITS: usize = 4;

/// Expected ASCII digit count of `MM` and `DD` segments in an ISO `YYYY-MM-DD` date.
const ISO_MONTH_DAY_DIGITS: usize = 2;

/// Output formats supported by `org query agenda`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum AgendaOutputFormat {
    /// Emacs-like human-readable timeline and optional GTD summary.
    Human,
    /// Structured JSON object conforming to `schemas/org-agenda-output.schema.json`.
    Json,
    /// Deduplicated `file::line` locators (one per line).
    Locator,
    /// Validated Org-mode document (`org fmt check` clean).
    Org,
    /// Zensical-ready GitHub Flavored Markdown document.
    #[value(alias = "markdown")]
    Md,
}

/// CLI arguments for `org query agenda`.
#[derive(Debug, Clone, Args)]
pub(crate) struct AgendaArgs {
    /// Files or directories to scan (defaults to configured `org-roam` vaults or `.`).
    #[arg(value_hint = ValueHint::AnyPath)]
    pub(crate) paths: Vec<PathBuf>,

    /// Repeatable named `[roam.directories]` vault or directory path selector.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Vec<String>,

    /// Scan only `#+filetags: :agenda:` files and base `<gtd_subdir>/` files.
    #[arg(long)]
    pub(crate) agenda_only: bool,

    /// Inclusive start date in `YYYY-MM-DD` format (defaults to local today).
    #[arg(long)]
    pub(crate) from: Option<String>,

    /// Number of calendar days to project in the timeline.
    #[arg(long, default_value_t = DEFAULT_AGENDA_DAYS)]
    pub(crate) days: usize,

    /// Override deadline lead-time warning window in days (defaults to `.org-tools.toml`).
    #[arg(long)]
    pub(crate) warning_days: Option<u32>,

    /// Append deterministic GTD `Next Actions`, `Blocked`, and `Inbox` sections.
    #[arg(long)]
    pub(crate) composite: bool,

    /// Output format (`human`, `json`, `locator`, `org`, or `md`).
    #[arg(long, value_enum, default_value_t = AgendaOutputFormat::Human)]
    pub(crate) format: AgendaOutputFormat,

    /// Optional output file path written atomically (defaults to stdout).
    #[arg(long, short, value_hint = ValueHint::FilePath)]
    pub(crate) output: Option<PathBuf>,
}

/// Reusable agenda query parameters shared by CLI and MCP handlers.
#[derive(Debug, Clone)]
pub(crate) struct AgendaQueryOptions {
    /// Files or directories to scan.
    pub(crate) paths: Vec<PathBuf>,
    /// Optional `YYYY-MM-DD` start date override.
    pub(crate) from: Option<String>,
    /// Number of calendar days to project in the timeline.
    pub(crate) days: usize,
    /// Optional deadline warning window override in days.
    pub(crate) warning_days: Option<u32>,
    /// Whether to include composite GTD `Next Actions`, `Blocked`, and `Inbox` sections.
    pub(crate) composite: bool,
    /// Whether to bypass warm `.cache/org-cache.db` synchronization.
    pub(crate) no_cache: bool,
}

/// Validated workspace inputs prepared before building the agenda timeline.
struct PreparedAgendaWorkspace {
    /// Inclusive `(year, month, day)` start date.
    start_date: (u16, u8, u8),
    /// Effective deadline warning window in days.
    warning_days: u32,
    /// Loaded raw [`SourceFile`] buffers aligned 1-to-1 with `docs`.
    sources: Vec<SourceFile>,
    /// Parsed [`OrgDocument`] structures in deterministic path order.
    docs: Vec<OrgDocument>,
    /// Cache telemetry counts `(files_scanned, cache_hits, cache_misses)`.
    counts: (usize, usize, usize),
}

// endregion

// region Validation, Config & Cache Helpers

/// Checks whether `YYYY-MM-DD` components have exact digit widths and ASCII digits.
fn has_iso_date_digits(raw: &str, ys: &str, ms: &str, ds: &str) -> bool {
    ys.len() == ISO_YEAR_DIGITS
        && ms.len() == ISO_MONTH_DAY_DIGITS
        && ds.len() == ISO_MONTH_DAY_DIGITS
        && raw.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

/// Parses an optional `YYYY-MM-DD` `--from` argument or returns local `current_date()`.
pub(crate) fn parse_agenda_from_date(raw: Option<&str>) -> Result<(u16, u8, u8), String> {
    let Some(text) = raw else {
        return Ok(date::current_date());
    };
    let parts: Vec<&str> = text.split('-').collect();
    if let [ys, ms, ds] = parts.as_slice() {
        if has_iso_date_digits(text, ys, ms, ds) {
            if let (Ok(y), Ok(m), Ok(d)) = (ys.parse::<u16>(), ms.parse::<u8>(), ds.parse::<u8>()) {
                if is_valid_date(y, m, d) {
                    return Ok((y, m, d));
                }
            }
        }
    }
    Err(format!(
        "invalid --from date '{text}': expected valid YYYY-MM-DD calendar date"
    ))
}

/// Resolves `.org-tools.toml` `[agenda].deadline_warning_days` from `paths` with optional override.
pub(crate) fn resolve_effective_warning_days(
    paths: &[PathBuf],
    cli_override: Option<u32>,
) -> Result<u32, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let start_dir = match paths.first() {
        Some(first) => {
            let resolved = if first.is_absolute() {
                first.clone()
            } else {
                cwd.join(first)
            };
            if resolved.is_dir() {
                resolved
            } else {
                resolved.parent().map(Path::to_path_buf).unwrap_or(cwd)
            }
        }
        None => cwd,
    };
    let config = Config::load(&start_dir).map_err(|e| e.to_string())?;
    Ok(cli_override.unwrap_or(config.agenda.deadline_warning_days))
}

/// Synchronizes an already-existing `.cache/org-cache.db` without auto-creating a new database.
fn sync_warm_cache_counts(
    paths: &[PathBuf],
    files_scanned: usize,
    no_cache: bool,
) -> (usize, usize, usize) {
    if !no_cache && paths.len() == 1 && paths[0].is_dir() {
        let norm_root = normalize_workspace_path(&paths[0]);
        if WorkspaceCache::resolve_db_path(&norm_root).is_file() {
            if let Ok((_, tel)) = WorkspaceCache::open_and_sync(&paths[0]) {
                return (tel.files_scanned, tel.cache_hits, tel.cache_misses);
            }
        }
    }
    (files_scanned, 0, files_scanned)
}

/// Validates `paths` and loads `(SourceFile, OrgDocument)` buffers in sorted path order.
fn load_agenda_workspace(
    paths: &[PathBuf],
    allow_empty_files: bool,
) -> Result<(Vec<SourceFile>, Vec<OrgDocument>), String> {
    for path in paths {
        if !path.exists() {
            return Err(format!("path not found: {}", path.display()));
        }
    }
    let files = collect_org_files(paths);
    if files.is_empty() && !allow_empty_files {
        return Err("no .org files found".to_string());
    }
    let (mut sources, mut docs) = (
        Vec::with_capacity(files.len()),
        Vec::with_capacity(files.len()),
    );
    for file in &files {
        let src = SourceFile::from_path(file)
            .map_err(|e| format!("error reading {}: {e}", file.display()))?;
        docs.push(OrgDocument::from_source(&src));
        sources.push(src);
    }
    Ok((sources, docs))
}

/// Validates options, resolves config/cache state, and parses workspace `.org` files.
fn prepare_agenda_workspace(
    opts: &AgendaQueryOptions,
    telemetry_override: Option<&MutationTelemetry>,
    allow_empty_files: bool,
) -> Result<PreparedAgendaWorkspace, String> {
    let start_date = parse_agenda_from_date(opts.from.as_deref())?;
    let warning_days = resolve_effective_warning_days(&opts.paths, opts.warning_days)?;
    let (sources, docs) = load_agenda_workspace(&opts.paths, allow_empty_files)?;
    let counts = match telemetry_override {
        Some(t) => (t.files_scanned, t.cache_hits, t.cache_misses),
        None => sync_warm_cache_counts(&opts.paths, sources.len(), opts.no_cache),
    };
    Ok(PreparedAgendaWorkspace {
        start_date,
        warning_days,
        sources,
        docs,
        counts,
    })
}

/// Serializes a [`MutationTelemetry`] JSON object from `started`, `counts`, and optional override.
fn build_telemetry_value(
    started: Instant,
    counts: (usize, usize, usize),
    telemetry_override: Option<MutationTelemetry>,
) -> serde_json::Value {
    let elapsed_us = started.elapsed().as_micros() as u64;
    let tel = MutationTelemetry {
        duration_us: telemetry_override.map_or(elapsed_us, |t| t.duration_us.max(elapsed_us)),
        files_scanned: counts.0,
        cache_hits: counts.1,
        cache_misses: counts.2,
    };
    serde_json::to_value(tel).unwrap_or_default()
}

// endregion

// region Execution & Output Dispatch

/// Formats an evaluated agenda state into the requested [`AgendaOutputFormat`].
fn dispatch_agenda_format(
    format: AgendaOutputFormat,
    prep: &PreparedAgendaWorkspace,
    num_days: usize,
    agenda_days: &[AgendaDay<'_>],
    composite: Option<&CompositeAgenda<'_>>,
    doc_refs: &[&OrgDocument],
    tel_val: serde_json::Value,
) -> String {
    match format {
        AgendaOutputFormat::Human => render_agenda_report_human(agenda_days, composite, doc_refs),
        AgendaOutputFormat::Locator => render_agenda_locators(agenda_days, composite),
        AgendaOutputFormat::Org => render_agenda_doc(
            prep.start_date,
            num_days,
            agenda_days,
            composite,
            doc_refs,
            false,
        ),
        AgendaOutputFormat::Md => render_agenda_doc(
            prep.start_date,
            num_days,
            agenda_days,
            composite,
            doc_refs,
            true,
        ),
        AgendaOutputFormat::Json => {
            let val = build_agenda_json_value(
                prep.start_date,
                num_days,
                prep.warning_days,
                agenda_days,
                composite,
                doc_refs,
                tel_val,
            );
            format!(
                "{}\n",
                serde_json::to_string_pretty(&val).unwrap_or_default()
            )
        }
    }
}

/// Evaluates `opts` and returns the schema-valid JSON agenda envelope (`org_query_agenda`-ready).
#[allow(dead_code)]
pub(crate) fn execute_agenda_json(
    opts: &AgendaQueryOptions,
    telemetry_override: Option<MutationTelemetry>,
    allow_empty_files: bool,
) -> Result<serde_json::Value, String> {
    let started = Instant::now();
    let prep = prepare_agenda_workspace(opts, telemetry_override.as_ref(), allow_empty_files)?;
    let build_opts = AgendaBuildOptions {
        sources: Some(&prep.sources),
        start_date: prep.start_date,
        num_days: opts.days,
        warning_days: prep.warning_days,
    };
    let agenda_days = build_agenda_with_options(&prep.docs, build_opts);
    let doc_refs: Vec<&OrgDocument> = prep.docs.iter().collect();
    let composite = opts
        .composite
        .then(|| collect_composite_agenda(&prep.docs, &doc_refs, prep.start_date));
    let tel_val = build_telemetry_value(started, prep.counts, telemetry_override);
    Ok(build_agenda_json_value(
        prep.start_date,
        opts.days,
        prep.warning_days,
        &agenda_days,
        composite.as_ref(),
        &doc_refs,
        tel_val,
    ))
}

/// Evaluates `opts` and renders `(formatted_text, has_items)` for `format`.
pub(crate) fn render_agenda_for_options(
    opts: &AgendaQueryOptions,
    format: AgendaOutputFormat,
    telemetry_override: Option<MutationTelemetry>,
    allow_empty_files: bool,
) -> Result<(String, bool), String> {
    let started = Instant::now();
    let prep = prepare_agenda_workspace(opts, telemetry_override.as_ref(), allow_empty_files)?;
    let build_opts = AgendaBuildOptions {
        sources: Some(&prep.sources),
        start_date: prep.start_date,
        num_days: opts.days,
        warning_days: prep.warning_days,
    };
    let agenda_days = build_agenda_with_options(&prep.docs, build_opts);
    let doc_refs: Vec<&OrgDocument> = prep.docs.iter().collect();
    let composite = opts
        .composite
        .then(|| collect_composite_agenda(&prep.docs, &doc_refs, prep.start_date));
    let has_items = agenda_days.iter().any(|d| !d.items.is_empty())
        || composite.as_ref().is_some_and(|c| !c.is_empty());
    let tel_val = build_telemetry_value(started, prep.counts, telemetry_override);
    let rendered = dispatch_agenda_format(
        format,
        &prep,
        opts.days,
        &agenda_days,
        composite.as_ref(),
        &doc_refs,
        tel_val,
    );
    Ok((rendered, has_items))
}

/// Executes `org query agenda` from CLI `args` and returns the process exit code.
pub(crate) fn run_agenda(args: AgendaArgs, no_cache: bool) -> i32 {
    let resolved_paths = if !args.roam_dir.is_empty() || args.agenda_only || args.paths.is_empty() {
        match crate::commands::query_gtd::resolve_gtd_scan_paths(
            &args.paths,
            &args.roam_dir,
            args.agenda_only,
        ) {
            Ok(p) => p,
            Err(err) => {
                eprintln!("org: {err}");
                return EXIT_ERROR;
            }
        }
    } else {
        args.paths
    };
    let opts = AgendaQueryOptions {
        paths: resolved_paths,
        from: args.from,
        days: args.days,
        warning_days: args.warning_days,
        composite: args.composite,
        no_cache,
    };
    let (rendered, has_items) = match render_agenda_for_options(&opts, args.format, None, false) {
        Ok(res) => res,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    if let Some(ref out_path) = args.output {
        if let Err(err) = write_file_atomic(out_path, &rendered) {
            eprintln!("org: error writing {}: {err}", out_path.display());
            return EXIT_ERROR;
        }
    } else if !rendered.is_empty() {
        let _ = write!(std::io::stdout().lock(), "{rendered}");
    }
    if has_items {
        EXIT_OK
    } else {
        EXIT_ISSUES
    }
}

// endregion
