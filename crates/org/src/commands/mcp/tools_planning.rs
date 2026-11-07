// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP handlers for `org_query_agenda` and `org_update_planning`.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Map, Value};

use org_tools_core::document::OrgDocument;
use org_tools_core::export_md::normalize_workspace_path;
use org_tools_core::files::{canonicalize_clean, collect_org_files, join_slash_rel};
use org_tools_core::locator::{resolve_in_document, resolve_locator, OrgLocator, ResolvedEntry};
use org_tools_core::planning::PlanningKind;
use org_tools_core::source::SourceFile;

use super::confinement::{
    canonical_workspace_root, confine_and_rewrite_locator, verify_confined_mutation_target,
    verify_existing_confined_org_file,
};
use super::schema::{
    finish_tool_call, optional_bool, optional_positive_usize, optional_str, optional_str_array,
    optional_u32, require_non_empty_str,
};
use super::McpSession;
use crate::commands::query::query_agenda::{
    execute_agenda_json, parse_agenda_from_date, render_agenda_for_options,
    resolve_effective_warning_days, AgendaOutputFormat, AgendaQueryOptions, DEFAULT_AGENDA_DAYS,
};
use crate::commands::update::update_planning::{apply_planning_targets, validate_planning_spec};
use crate::commands::update::FileEntryTargets;
use crate::commands::{MutationResult, MutationTelemetry};
use crate::date;

// region Agenda Parameter Parsing & Confinement

/// Validated and confined parameters for an `org_query_agenda` tool call.
struct AgendaCallParams {
    /// Normalized workspace root directory and whether it is the default workspace.
    ws: (PathBuf, bool),
    /// Confined root path used to load `.org-tools.toml` and confined `.org` file list.
    scan: (PathBuf, Vec<PathBuf>),
    /// Raw optional `YYYY-MM-DD` start date and parsed `(year, month, day)`.
    date_spec: (Option<String>, (u16, u8, u8)),
    /// Calendar days to project, optional warning override, and `composite` flag.
    window: (usize, Option<u32>, bool),
    /// Output format enum and canonical lowercase format label.
    fmt: (AgendaOutputFormat, &'static str),
}

/// Parses an optional `format` string into `(AgendaOutputFormat, canonical_name)`.
fn parse_agenda_format_arg(
    raw: Option<&str>,
) -> Result<(AgendaOutputFormat, &'static str), String> {
    match raw {
        None | Some("json") => Ok((AgendaOutputFormat::Json, "json")),
        Some("org") => Ok((AgendaOutputFormat::Org, "org")),
        Some("md") => Ok((AgendaOutputFormat::Md, "md")),
        Some("human") => Ok((AgendaOutputFormat::Human, "human")),
        Some("locator") => Ok((AgendaOutputFormat::Locator, "locator")),
        Some(other) => Err(format!("unsupported format '{other}'")),
    }
}

/// Collects `.org` files beneath `dir` whose canonical targets remain inside `canon_ws`.
fn collect_confined_dir_files(dir: &Path, canon_ws: &Path) -> Vec<PathBuf> {
    let ok = |p: &PathBuf| verify_existing_confined_org_file(p, canon_ws, "").is_ok();
    collect_org_files(&[dir.to_path_buf()])
        .into_iter()
        .filter(ok)
        .collect()
}

/// Confines an existing file or directory `candidate` inside `canon_ws`.
fn confine_existing_scan_candidate(
    candidate: &Path,
    canon_ws: &Path,
    label: &str,
) -> Result<(PathBuf, Vec<PathBuf>), String> {
    if !candidate.exists() && fs::symlink_metadata(candidate).is_err() {
        return Err(format!("path not found: {label}"));
    }
    let cand_canon = canonicalize_clean(candidate).map_err(|e| format!("bad path {label}: {e}"))?;
    if !cand_canon.starts_with(canon_ws) {
        return Err(format!("path escapes workspace boundary: {label}"));
    }
    if cand_canon.is_dir() {
        let files = collect_confined_dir_files(candidate, canon_ws);
        return Ok((candidate.to_path_buf(), files));
    }
    verify_existing_confined_org_file(candidate, canon_ws, label)?;
    Ok((candidate.to_path_buf(), vec![candidate.to_path_buf()]))
}

/// Resolves optional `raw_path` relative to `display_ws` and confines discovered `.org` files.
fn resolve_confined_agenda_scan(
    raw_path: Option<&str>,
    display_ws: &Path,
    canon_ws: &Path,
) -> Result<(PathBuf, Vec<PathBuf>), String> {
    let Some(raw) = raw_path else {
        let files = collect_confined_dir_files(display_ws, canon_ws);
        return Ok((display_ws.to_path_buf(), files));
    };
    if raw.trim().is_empty() {
        return Err("argument 'path' must not be empty".to_string());
    }
    let input = Path::new(raw);
    if input
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(format!("path traversal outside workspace: {raw}"));
    }
    let candidate = if input.is_absolute() {
        if !normalize_workspace_path(input).starts_with(canon_ws) {
            return Err(format!("path escapes workspace boundary: {raw}"));
        }
        input.to_path_buf()
    } else {
        join_slash_rel(display_ws, raw)
    };
    confine_existing_scan_candidate(&candidate, canon_ws, raw)
}

/// Parses scalar agenda options and assembles [`AgendaCallParams`].
fn build_agenda_call_params(
    ws: (PathBuf, bool),
    scan: (PathBuf, Vec<PathBuf>),
    args: &Map<String, Value>,
) -> Result<AgendaCallParams, String> {
    let from = optional_str(args, "from")?;
    let start = parse_agenda_from_date(from.as_deref())?;
    let days = optional_positive_usize(args, "days")?.unwrap_or(DEFAULT_AGENDA_DAYS);
    let warn = optional_u32(args, "warning_days")?;
    let comp = optional_bool(args, "composite", false)?;
    let fmt = parse_agenda_format_arg(optional_str(args, "format")?.as_deref())?;
    Ok(AgendaCallParams {
        ws,
        scan,
        date_spec: (from, start),
        window: (days, warn, comp),
        fmt,
    })
}

/// Validates and confines all parameters for `org_query_agenda`.
fn parse_agenda_call_params(
    session: &McpSession,
    args: &Map<String, Value>,
) -> Result<AgendaCallParams, String> {
    let k: Vec<_> = "path|from|days|warning_days|composite|format|workspace"
        .split('|')
        .collect();
    let (display_ws, norm_ws, is_default) = session.prepare_workspace(args, &k)?;
    let raw_path = optional_str(args, "path")?;
    let canon_ws = canonical_workspace_root(&norm_ws)?;
    let scan = resolve_confined_agenda_scan(raw_path.as_deref(), &display_ws, &canon_ws)?;
    build_agenda_call_params((norm_ws, is_default), scan, args)
}

// endregion

// region Agenda Execution Handler

/// Evaluates the confined agenda request and builds the JSON or rendered-report envelope.
fn evaluate_agenda_payload(
    session: &McpSession,
    p: &AgendaCallParams,
    tel: MutationTelemetry,
    started: Instant,
) -> Result<Value, String> {
    let eff_warn = resolve_effective_warning_days(std::slice::from_ref(&p.scan.0), p.window.1)?;
    let opts = AgendaQueryOptions {
        paths: p.scan.1.clone(),
        from: p.date_spec.0.clone(),
        days: p.window.0,
        warning_days: Some(eff_warn),
        composite: p.window.2,
        no_cache: true,
    };
    if p.fmt.0 == AgendaOutputFormat::Json {
        let mut val = execute_agenda_json(&opts, Some(tel), true)?;
        val["telemetry"]["duration_us"] = json!(started.elapsed().as_micros() as u64);
        return Ok(val);
    }
    let counts = (tel.files_scanned, tel.cache_hits, tel.cache_misses);
    let (report, has_items) = render_agenda_for_options(&opts, p.fmt.0, Some(tel), true)?;
    let final_tel = session.counts_telemetry(started, counts);
    let (y, m, d) = p.date_spec.1;
    Ok(json!({
        "format": p.fmt.1, "from": format!("{y:04}-{m:02}-{d:02}"), "days": p.window.0,
        "warning_days": eff_warn, "has_items": has_items, "report": report, "telemetry": final_tel
    }))
}

/// Executes `org_query_agenda` over `stdio` with warm `MabuseCache` telemetry.
pub(super) fn exec_query_agenda(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let params = parse_agenda_call_params(session, args)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let tel = session
            .sync_telemetry(&params.ws.0, params.ws.1, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let counts = (tel.files_scanned, tel.cache_hits, tel.cache_misses);
        evaluate_agenda_payload(session, &params, tel, started)
            .map_err(|e| (e, session.counts_telemetry(started, counts)))
    })())
}

// endregion

// region Planning Parameter Parsing & Target Resolution

/// Validated parameters for an `org_update_planning` tool call.
struct PlanningCallCtx {
    /// Caller-facing, normalized, and canonical workspace directories plus `is_default`.
    ws: (PathBuf, PathBuf, PathBuf, bool),
    /// Planning timestamp kind (`Scheduled` or `Deadline`) and validated spec.
    plan: (PlanningKind, Option<String>),
    /// Confined `(raw_label, rewritten_locator)` pairs.
    locators: Vec<(String, OrgLocator)>,
    /// Local calendar date and `dry_run` flag.
    exec: ((u16, u8, u8), bool),
}

/// Validates mutual exclusivity between `clear: true` and `value`.
fn resolve_clear_vs_value(clear: bool, value: Option<String>) -> Result<Option<String>, String> {
    if clear {
        if value.is_some_and(|v| !v.trim().is_empty()) {
            return Err("cannot specify non-empty 'value' when 'clear' is true".to_string());
        }
        return Ok(None);
    }
    value
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "missing required string argument 'value' when 'clear' is false".to_string())
        .map(Some)
}

/// Parses `kind`, `clear`, and `value` and validates the timestamp specification against `today`.
fn parse_planning_kind_and_spec(
    args: &Map<String, Value>,
    today: (u16, u8, u8),
) -> Result<(PlanningKind, Option<String>), String> {
    let kind = match require_non_empty_str(args, "kind")?.as_str() {
        "scheduled" => PlanningKind::Scheduled,
        "deadline" => PlanningKind::Deadline,
        other => {
            return Err(format!(
                "unsupported kind '{other}' (expected scheduled or deadline)"
            ))
        }
    };
    let spec = resolve_clear_vs_value(
        optional_bool(args, "clear", false)?,
        optional_str(args, "value")?,
    )?;
    validate_planning_spec(kind, spec.as_deref(), today)?;
    Ok((kind, spec))
}

/// Extracts a non-empty list of raw locator strings from mutually exclusive `locator` / `targets`.
fn unpack_locator_inputs(
    locator: Option<String>,
    targets: Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    match (locator, targets) {
        (Some(_), Some(_)) => Err("provide either 'locator' or 'targets', not both".to_string()),
        (None, None) => Err("missing required 'locator' or 'targets'".to_string()),
        (Some(loc), None) if loc.trim().is_empty() => {
            Err("argument 'locator' must not be empty".to_string())
        }
        (Some(loc), None) => Ok(vec![loc]),
        (None, Some(list)) => Ok(list),
    }
}

/// Parses and confines all target `OrgLocator` arguments for `org_update_planning`.
fn parse_planning_locators(
    args: &Map<String, Value>,
    display_ws: &Path,
    canon_ws: &Path,
) -> Result<Vec<(String, OrgLocator)>, String> {
    let raw_list = unpack_locator_inputs(
        optional_str(args, "locator")?,
        optional_str_array(args, "targets")?,
    )?;
    let mut out = Vec::with_capacity(raw_list.len());
    for raw_str in raw_list {
        let parsed = OrgLocator::parse(&raw_str).map_err(|e| e.to_string())?;
        let confined = confine_and_rewrite_locator(&parsed, display_ws, canon_ws)?;
        out.push((raw_str, confined));
    }
    Ok(out)
}

/// Parses and confines all parameters for `org_update_planning`.
fn parse_planning_call_ctx(
    session: &McpSession,
    args: &Map<String, Value>,
) -> Result<PlanningCallCtx, String> {
    let k: Vec<_> = "kind|locator|targets|value|clear|dry_run|workspace"
        .split('|')
        .collect();
    let (display_ws, norm_ws, is_default) = session.prepare_workspace(args, &k)?;
    let today = date::current_date();
    let plan = parse_planning_kind_and_spec(args, today)?;
    let dry_run = optional_bool(args, "dry_run", false)?;
    let canon_ws = canonical_workspace_root(&norm_ws)?;
    let locators = parse_planning_locators(args, &display_ws, &canon_ws)?;
    Ok(PlanningCallCtx {
        ws: (display_ws, norm_ws, canon_ws, is_default),
        plan,
        locators,
        exec: (today, dry_run),
    })
}

/// Attempts a fast-path `id:<uuid>` heading lookup through the warm `MabuseCache`.
fn try_resolve_cached_id(
    session: &McpSession,
    ctx: &PlanningCallCtx,
    locator: &OrgLocator,
    label: &str,
) -> Result<Option<ResolvedEntry>, String> {
    let (display_ws, _, canon_ws, is_default) = &ctx.ws;
    let (Some(cache), OrgLocator::Id(uuid)) = (session.warm_cache_for(*is_default), locator) else {
        return Ok(None);
    };
    let Some(first) = cache
        .resolve_roam_nodes(display_ws, &format!("id:{uuid}"), false)
        .ok()
        .and_then(|mut ns| (!ns.is_empty()).then(|| ns.swap_remove(0)))
    else {
        return Ok(None);
    };
    let cand = Path::new(&first.file);
    verify_existing_confined_org_file(cand, canon_ws, label)?;
    let Ok(src) = SourceFile::from_path(cand) else {
        return Ok(None);
    };
    let Ok(mut resolved) = resolve_in_document(locator, &OrgDocument::from_source(&src)) else {
        return Ok(None);
    };
    resolved.file = PathBuf::from(first.file);
    Ok(Some(resolved))
}

/// Resolves a single confined heading locator, rejecting level-0 file `:ID:` nodes.
fn resolve_planning_entry(
    session: &McpSession,
    ctx: &PlanningCallCtx,
    locator: &OrgLocator,
    label: &str,
) -> Result<(PathBuf, PathBuf, usize), String> {
    let (display_ws, _, canon_ws, _) = &ctx.ws;
    let resolved = match try_resolve_cached_id(session, ctx, locator, label)? {
        Some(hit) => hit,
        None => {
            resolve_locator(locator, std::slice::from_ref(display_ws)).map_err(|e| e.to_string())?
        }
    };
    let file_canon = verify_existing_confined_org_file(&resolved.file, canon_ws, label)?;
    verify_confined_mutation_target(&resolved.file, canon_ws)?;
    if resolved.is_file_node() {
        return Err(format!("cannot set planning on file-level node {label}"));
    }
    Ok((file_canon, resolved.file, resolved.entry_index))
}

// endregion

// region Planning Mutation Handler

/// Resolves confined targets and executes the atomic planning batch mutation pass.
fn run_planning_mutation_pass(
    session: &McpSession,
    ctx: &PlanningCallCtx,
) -> Result<(MutationResult, Vec<PathBuf>), String> {
    let mut slot_by_canon: HashMap<PathBuf, usize> = HashMap::new();
    let mut grouped: FileEntryTargets = Vec::new();
    for (label, locator) in &ctx.locators {
        let (file_canon, file_path, idx) = resolve_planning_entry(session, ctx, locator, label)?;
        if let Some(&pos) = slot_by_canon.get(&file_canon) {
            if let Some(ref mut indices) = grouped[pos].1 {
                indices.push(idx);
            }
        } else {
            slot_by_canon.insert(file_canon, grouped.len());
            grouped.push((file_path, Some(vec![idx])));
        }
    }
    let ((kind, ref spec), (today, dry)) = (&ctx.plan, ctx.exec);
    let (res, modified) =
        apply_planning_targets(&grouped, None, *kind, spec.as_deref(), today, dry, None)?;
    if !res.errors.is_empty() {
        return Err(res.errors.join("; "));
    }
    Ok((res, modified))
}

/// Executes `org_update_planning`, invalidating and refreshing warm `MabuseCache` entries.
pub(super) fn exec_update_planning(
    session: &mut McpSession,
    args: &Map<String, Value>,
    started: Instant,
) -> Value {
    finish_tool_call((|| {
        let ctx = parse_planning_call_ctx(session, args)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        let (_, ref norm_ws, _, is_default) = ctx.ws;
        let outcome = run_planning_mutation_pass(session, &ctx);
        if let (Ok((_, ref paths)), false) = (&outcome, ctx.exec.1) {
            session
                .invalidate_workspace_paths(norm_ws, is_default, paths)
                .map_err(|e| (e, session.empty_telemetry(started)))?;
        }
        let mut tel = session
            .sync_telemetry(norm_ws, is_default, started)
            .map_err(|e| (e, session.empty_telemetry(started)))?;
        tel.duration_us = started.elapsed().as_micros() as u64;
        let (mut mutation, _) = match outcome {
            Ok(v) => v,
            Err(msg) => return Err((msg, tel)),
        };
        mutation.telemetry = tel;
        Ok(serde_json::to_value(mutation).unwrap_or_default())
    })())
}

// endregion
