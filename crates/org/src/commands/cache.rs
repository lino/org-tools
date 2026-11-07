// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org cache` subcommand handlers (`status`, `rebuild`, `clear`).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use org_tools_core::cache::{CacheStatus, CacheTelemetry, MabuseCache};
use org_tools_core::export_md::{normalize_workspace_path, WORKSPACE_ENV_VAR};

use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Types

/// Ancestor markers inspected when discovering a default workspace root from `cwd`.
const WORKSPACE_MARKERS: &[&str] = &[".git", "kb.toml", ".org-tools.toml", ".fritz.toml"];

/// Output format for `org cache` subcommands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CacheOutputFormat {
    /// Human-readable summary text.
    Human,
    /// Structured JSON object with execution telemetry.
    Json,
}

/// Shared CLI arguments for `org cache status`, `rebuild`, and `clear`.
#[derive(Debug, Clone, Args)]
pub(crate) struct CacheArgs {
    /// Optional directory path to inspect, rebuild, or clear.
    pub(crate) path: Option<PathBuf>,

    /// Explicit workspace root directory override.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Output format (`human` or `json`).
    #[arg(long, value_enum, default_value = "human")]
    pub(crate) format: CacheOutputFormat,
}

/// Subcommands under `org cache`.
#[derive(Subcommand)]
pub(crate) enum CacheCommand {
    /// Report row counts, file size, and status of `.cache/mabuse.db`.
    Status(CacheArgs),
    /// Drop and rebuild `.cache/mabuse.db` inside a single transaction.
    Rebuild(CacheArgs),
    /// Remove `.cache/mabuse.db` and its `-wal` / `-shm` sidecar files.
    Clear(CacheArgs),
}

/// JSON envelope returned by `org cache clear --format json`.
#[derive(Serialize)]
struct ClearJsonEnvelope {
    /// Resolved path to the SQLite database file.
    db_path: String,
    /// Whether any database or sidecar file existed and was deleted.
    removed: bool,
    /// Execution telemetry for the clear operation.
    telemetry: CacheTelemetry,
}

// endregion

// region Workspace Resolution & Output Helpers

/// Discovers a default workspace root via `ORG_TOOLS_WORKSPACE` or ancestor project markers.
fn discover_workspace_from_cwd(cwd: &Path) -> PathBuf {
    if let Some(env_ws) = std::env::var_os(WORKSPACE_ENV_VAR).filter(|v| !v.is_empty()) {
        return PathBuf::from(env_ws);
    }
    let norm = normalize_workspace_path(cwd);
    let mut cur: Option<&Path> = Some(&norm);
    while let Some(dir) = cur {
        if WORKSPACE_MARKERS.iter().any(|m| dir.join(m).exists()) {
            return dir.to_path_buf();
        }
        cur = dir.parent();
    }
    cwd.to_path_buf()
}

/// Resolves and validates the target workspace directory from `args`.
fn resolve_cache_workspace(args: &CacheArgs) -> Result<PathBuf, String> {
    if let Some(ref ws) = args.workspace {
        if !ws.is_dir() {
            return Err(format!("workspace directory not found: {}", ws.display()));
        }
        return Ok(ws.clone());
    }
    if let Some(ref p) = args.path {
        if p.is_dir() {
            return Ok(p.clone());
        }
        if p.is_file() {
            let parent = p.parent().unwrap_or_else(|| Path::new("."));
            return Ok(parent.to_path_buf());
        }
        return Err(format!("path not found: {}", p.display()));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Ok(discover_workspace_from_cwd(&cwd))
}

/// Emits a [`CacheStatus`] payload in `human` or `json` format.
fn emit_status_payload(action_label: &str, status: &CacheStatus, format: CacheOutputFormat) {
    match format {
        CacheOutputFormat::Json => {
            let rendered = serde_json::to_string_pretty(status).unwrap_or_default();
            let _ = writeln!(std::io::stdout().lock(), "{rendered}");
        }
        CacheOutputFormat::Human => {
            let _ = writeln!(
                std::io::stdout().lock(),
                "{action_label}: {} (exists={}, size={} B, files={}, nodes={}, links={}, edna={}, fts={})",
                status.db_path,
                status.exists,
                status.size_bytes,
                status.files_indexed,
                status.nodes_count,
                status.links_count,
                status.edna_edges_count,
                status.fts_entries_count,
            );
        }
    }
}

// endregion

// region Subcommand Handlers

/// Executes `org cache status`.
fn handle_status(args: CacheArgs) -> i32 {
    let root = match resolve_cache_workspace(&args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };
    match MabuseCache::status(&root) {
        Ok(status) => {
            emit_status_payload("status", &status, args.format);
            EXIT_OK
        }
        Err(e) => {
            eprintln!("org: {e}");
            EXIT_ERROR
        }
    }
}

/// Executes `org cache rebuild`.
fn handle_rebuild(args: CacheArgs) -> i32 {
    let root = match resolve_cache_workspace(&args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };
    match MabuseCache::rebuild(&root) {
        Ok((_, status)) => {
            emit_status_payload("rebuilt", &status, args.format);
            EXIT_OK
        }
        Err(e) => {
            eprintln!("org: {e}");
            EXIT_ERROR
        }
    }
}

/// Executes `org cache clear`.
fn handle_clear(args: CacheArgs) -> i32 {
    let started = Instant::now();
    let root = match resolve_cache_workspace(&args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };
    match MabuseCache::clear(&root) {
        Ok((db_path, removed)) => {
            match args.format {
                CacheOutputFormat::Json => {
                    let payload = ClearJsonEnvelope {
                        db_path: db_path.display().to_string(),
                        removed,
                        telemetry: CacheTelemetry {
                            duration_us: started.elapsed().as_micros() as u64,
                            ..CacheTelemetry::default()
                        },
                    };
                    let rendered = serde_json::to_string_pretty(&payload).unwrap_or_default();
                    let _ = writeln!(std::io::stdout().lock(), "{rendered}");
                }
                CacheOutputFormat::Human => {
                    let label = if removed { "cleared" } else { "already absent" };
                    let _ = writeln!(std::io::stdout().lock(), "{label}: {}", db_path.display());
                }
            }
            EXIT_OK
        }
        Err(e) => {
            eprintln!("org: {e}");
            EXIT_ERROR
        }
    }
}

// endregion

// region Entrypoint

/// Dispatches `org cache` subcommands and returns the CLI exit status code.
pub(crate) fn run_cache(command: CacheCommand) -> i32 {
    match command {
        CacheCommand::Status(args) => handle_status(args),
        CacheCommand::Rebuild(args) => handle_rebuild(args),
        CacheCommand::Clear(args) => handle_clear(args),
    }
}

// endregion
