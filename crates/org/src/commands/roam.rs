// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org roam` subcommand handlers for `org-roam` v2 nodes, backlinks, links, and graphs.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, Subcommand, ValueEnum, ValueHint};
use serde::Serialize;

use org_tools_core::cache::RoamWorkspace;
use org_tools_core::config::Config;
use org_tools_core::export_md::{normalize_workspace_path, WORKSPACE_ENV_VAR};
use org_tools_core::roam::{RoamBacklink, RoamLink, RoamNode, RoamOutgoingLink};

use super::MutationTelemetry;
use crate::{EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

// region CLI Types

/// Ancestor markers inspected when discovering a workspace root from `cwd`.
const ROAM_WORKSPACE_MARKERS: &[&str] = &[".git", "kb.toml", ".org-tools.toml", ".fritz.toml"];

/// Output format for `org roam node`, `backlinks`, and `links`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RoamOutputFormat {
    /// Human-readable text summary.
    Human,
    /// Structured JSON conforming to `schemas/org-roam-output.schema.json`.
    Json,
}

/// Output format for `org roam graph`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RoamGraphFormat {
    /// Structured JSON graph (`nodes`, `edges`, `telemetry`).
    Json,
    /// Mermaid `flowchart LR` diagram.
    Mermaid,
    /// Graphviz DOT `digraph org_roam` specification.
    Dot,
}

/// Shared scan-root and vault-selection arguments for `org roam` subcommands.
#[derive(Args, Debug, Clone)]
pub(crate) struct RoamScopeArgs {
    /// Optional Org files or directories to scan.
    #[arg(value_hint = ValueHint::AnyPath)]
    pub(crate) paths: Vec<PathBuf>,
    /// Optional workspace root directory to scan.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,
    /// Repeatable named `[roam.directories]` vault or directory path selector.
    #[arg(long = "roam-dir")]
    pub(crate) roam_dir: Vec<String>,
}

/// Subcommands under `org roam`.
#[derive(Subcommand)]
pub(crate) enum RoamCommand {
    /// Resolve an `org-roam` v2 node by `id:<uuid>`, `:ID:`, title, or alias.
    Node {
        /// Node query (`id:<uuid>`, `:ID:`, title, or `:ROAM_ALIASES:` token).
        query: String,
        /// Scan-root and vault-selection arguments.
        #[command(flatten)]
        scope: RoamScopeArgs,
        /// Output format (`human` or `json`).
        #[arg(long, value_enum, default_value = "human")]
        format: RoamOutputFormat,
    },
    /// List incoming `[[id:...]]` backlinks pointing to a target node.
    Backlinks {
        /// Target node (`id:<uuid>`, `:ID:`, title, alias, or `OrgLocator`).
        target: String,
        /// Scan-root and vault-selection arguments.
        #[command(flatten)]
        scope: RoamScopeArgs,
        /// Output format (`human` or `json`).
        #[arg(long, value_enum, default_value = "human")]
        format: RoamOutputFormat,
    },
    /// List outgoing `[[id:...]]` links attributed to a source node.
    Links {
        /// Source node (`id:<uuid>`, `:ID:`, title, alias, or `OrgLocator`).
        source: String,
        /// Scan-root and vault-selection arguments.
        #[command(flatten)]
        scope: RoamScopeArgs,
        /// Output format (`human` or `json`).
        #[arg(long, value_enum, default_value = "human")]
        format: RoamOutputFormat,
    },
    /// Export the workspace `org-roam` v2 node and link graph.
    Graph {
        /// Scan-root and vault-selection arguments.
        #[command(flatten)]
        scope: RoamScopeArgs,
        /// Graph output format (`json`, `mermaid`, or `dot`).
        #[arg(long, value_enum, default_value = "json")]
        format: RoamGraphFormat,
    },
}

// endregion

// region JSON Envelopes

/// JSON output envelope for `org roam node --format json`.
#[derive(Serialize)]
struct NodeJsonEnvelope {
    /// Response discriminator (`"node"`).
    kind: &'static str,
    /// Original query string.
    query: String,
    /// Matched nodes sorted by `(file, line)`.
    nodes: Vec<RoamNode>,
    /// Execution timing and scan telemetry.
    telemetry: MutationTelemetry,
}

/// JSON output envelope for `org roam backlinks --format json`.
#[derive(Serialize)]
struct BacklinksJsonEnvelope {
    /// Response discriminator (`"backlinks"`).
    kind: &'static str,
    /// Original target query or locator.
    target: String,
    /// Resolved target nodes.
    nodes: Vec<RoamNode>,
    /// Incoming backlinks sorted by `(source_file, line)`.
    backlinks: Vec<RoamBacklink>,
    /// Execution timing and scan telemetry.
    telemetry: MutationTelemetry,
}

/// JSON output envelope for `org roam links --format json`.
#[derive(Serialize)]
struct LinksJsonEnvelope {
    /// Response discriminator (`"links"`).
    kind: &'static str,
    /// Original source query or locator.
    source: String,
    /// Resolved source nodes.
    nodes: Vec<RoamNode>,
    /// Outgoing links sorted by `(source_file, line)`.
    links: Vec<RoamOutgoingLink>,
    /// Execution timing and scan telemetry.
    telemetry: MutationTelemetry,
}

/// JSON output envelope for `org roam graph --format json`.
#[derive(Serialize)]
struct GraphJsonEnvelope {
    /// Response discriminator (`"graph"`).
    kind: &'static str,
    /// All non-excluded workspace nodes.
    nodes: Vec<RoamNode>,
    /// All extracted workspace `[[id:...]]` edges.
    edges: Vec<RoamLink>,
    /// Execution timing and scan telemetry.
    telemetry: MutationTelemetry,
}

// endregion

// region Workspace Discovery & Indexing

/// Discovers a default workspace root via `ORG_TOOLS_WORKSPACE` or ancestor project markers.
fn discover_default_workspace(cwd: &Path) -> PathBuf {
    if let Some(env_ws) = std::env::var_os(WORKSPACE_ENV_VAR).filter(|v| !v.is_empty()) {
        return PathBuf::from(env_ws);
    }
    let norm = normalize_workspace_path(cwd);
    let mut cur: Option<&Path> = Some(&norm);
    while let Some(dir) = cur {
        if ROAM_WORKSPACE_MARKERS.iter().any(|m| dir.join(m).exists()) {
            return dir.to_path_buf();
        }
        cur = dir.parent();
    }
    cwd.to_path_buf()
}

/// Validates and collects scan roots from optional `--workspace` and positional `paths`.
fn resolve_scan_roots(paths: &[PathBuf], workspace: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    if let Some(ws) = workspace {
        if !ws.exists() {
            return Err(format!("workspace path not found: {}", ws.display()));
        }
        roots.push(ws.to_path_buf());
    }
    for path in paths {
        if path.exists() {
            roots.push(path.clone());
        } else if let Some(joined) = workspace.map(|ws| ws.join(path)).filter(|p| p.exists()) {
            roots.push(joined);
        } else {
            return Err(format!("path not found: {}", path.display()));
        }
    }
    if roots.is_empty() {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        roots.push(discover_default_workspace(&cwd));
    }
    Ok(roots)
}

/// Opens a synchronized [`RoamWorkspace`] for `scope` and `no_cache`.
fn open_roam_backend(scope: &RoamScopeArgs, no_cache: bool) -> Result<RoamWorkspace, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cfg = Config::load_for_dir(scope.workspace.as_deref().unwrap_or(&cwd)).unwrap_or_default();

    if !scope.roam_dir.is_empty() {
        let mut named = Vec::with_capacity(scope.roam_dir.len());
        for selector in &scope.roam_dir {
            let Some((opt_name, path)) = cfg.resolve_roam_dir(selector) else {
                return Err(format!("unknown roam directory: {selector}"));
            };
            if !path.exists() {
                return Err(format!("roam directory not found: {}", path.display()));
            }
            named.push((opt_name.unwrap_or_else(|| selector.clone()), path));
        }
        return RoamWorkspace::open_named(&named, no_cache);
    }

    if scope.paths.is_empty() && scope.workspace.is_none() {
        let configured = cfg.roam_directories();
        if !configured.is_empty() {
            return RoamWorkspace::open_named(&configured, no_cache);
        }
    }

    let roots = resolve_scan_roots(&scope.paths, scope.workspace.as_deref())?;
    RoamWorkspace::open(&roots, no_cache)
}

/// Finalizes [`MutationTelemetry`] from `started` and `(scanned, hits, misses)` counters.
fn finish_telemetry(started: Instant, counts: (usize, usize, usize)) -> MutationTelemetry {
    MutationTelemetry {
        duration_us: started.elapsed().as_micros() as u64,
        files_scanned: counts.0,
        cache_hits: counts.1,
        cache_misses: counts.2,
    }
}

/// Writes pretty-printed JSON to stdout without panicking on broken pipes.
fn emit_json<T: Serialize>(payload: &T) {
    let rendered = serde_json::to_string_pretty(payload).unwrap_or_default();
    let _ = writeln!(std::io::stdout().lock(), "{rendered}");
}

/// Writes raw text to stdout without panicking on broken pipes.
fn emit_text(text: &str) {
    let _ = write!(std::io::stdout().lock(), "{text}");
}

// endregion

// region Subcommand Handlers & Entrypoint

/// Executes `org roam node`.
fn handle_node(
    query: String,
    scope: &RoamScopeArgs,
    format: RoamOutputFormat,
    no_cache: bool,
) -> i32 {
    let started = Instant::now();
    let res = open_roam_backend(scope, no_cache).and_then(|b| Ok((b.resolve(&query)?, b.counts())));
    let (nodes, counts) = match res {
        Ok(v) => v,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    let code = if nodes.is_empty() {
        EXIT_ISSUES
    } else {
        EXIT_OK
    };
    match format {
        RoamOutputFormat::Json => emit_json(&NodeJsonEnvelope {
            kind: "node",
            query,
            nodes,
            telemetry: finish_telemetry(started, counts),
        }),
        RoamOutputFormat::Human => {
            let mut buf = String::new();
            for n in &nodes {
                buf.push_str(&format!("{}  {}  ({}:{})\n", n.id, n.title, n.file, n.line));
            }
            emit_text(&buf);
        }
    }
    code
}

/// Executes `org roam backlinks`.
fn handle_backlinks(
    target: String,
    scope: &RoamScopeArgs,
    format: RoamOutputFormat,
    no_cache: bool,
) -> i32 {
    let started = Instant::now();
    let res = open_roam_backend(scope, no_cache).and_then(|b| {
        let ns = b.resolve(&target)?;
        let bl = b.backlinks(&ns)?;
        Ok((ns, bl, b.counts()))
    });
    let (nodes, backlinks, counts) = match res {
        Ok(v) => v,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    let code = if nodes.is_empty() {
        EXIT_ISSUES
    } else {
        EXIT_OK
    };
    match format {
        RoamOutputFormat::Json => emit_json(&BacklinksJsonEnvelope {
            kind: "backlinks",
            target,
            nodes,
            backlinks,
            telemetry: finish_telemetry(started, counts),
        }),
        RoamOutputFormat::Human => {
            let mut buf = String::new();
            for bl in &backlinks {
                let src = bl.source_id.as_deref().unwrap_or("-");
                buf.push_str(&format!(
                    "{}:{}  [{}]  {}\n",
                    bl.source_file, bl.line, src, bl.context
                ));
            }
            emit_text(&buf);
        }
    }
    code
}

/// Executes `org roam links`.
fn handle_links(
    source: String,
    scope: &RoamScopeArgs,
    format: RoamOutputFormat,
    no_cache: bool,
) -> i32 {
    let started = Instant::now();
    let res = open_roam_backend(scope, no_cache).and_then(|b| {
        let ns = b.resolve(&source)?;
        let out = b.outgoing(&ns)?;
        Ok((ns, out, b.counts()))
    });
    let (nodes, links, counts) = match res {
        Ok(v) => v,
        Err(err) => {
            eprintln!("org: {err}");
            return EXIT_ERROR;
        }
    };
    let code = if nodes.is_empty() {
        EXIT_ISSUES
    } else {
        EXIT_OK
    };
    match format {
        RoamOutputFormat::Json => emit_json(&LinksJsonEnvelope {
            kind: "links",
            source,
            nodes,
            links,
            telemetry: finish_telemetry(started, counts),
        }),
        RoamOutputFormat::Human => {
            let mut buf = String::new();
            for l in &links {
                let title = l.target_title.as_deref().unwrap_or("<unresolved>");
                buf.push_str(&format!(
                    "{}:{} -> {} ({title})\n",
                    l.source_file, l.line, l.target_id
                ));
            }
            emit_text(&buf);
        }
    }
    code
}

/// Executes `org roam graph`.
fn handle_graph(scope: &RoamScopeArgs, format: RoamGraphFormat, no_cache: bool) -> i32 {
    let started = Instant::now();
    let (index, counts) =
        match open_roam_backend(scope, no_cache).and_then(RoamWorkspace::into_graph) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("org: {err}");
                return EXIT_ERROR;
            }
        };
    match format {
        RoamGraphFormat::Json => emit_json(&GraphJsonEnvelope {
            kind: "graph",
            nodes: index.nodes,
            edges: index.links,
            telemetry: finish_telemetry(started, counts),
        }),
        RoamGraphFormat::Mermaid => emit_text(&index.render_mermaid()),
        RoamGraphFormat::Dot => emit_text(&index.render_dot()),
    }
    EXIT_OK
}

/// Dispatches `org roam` subcommands and returns the CLI exit status code.
pub(crate) fn run_roam(command: RoamCommand, no_cache: bool) -> i32 {
    match command {
        RoamCommand::Node {
            query,
            scope,
            format,
        } => handle_node(query, &scope, format, no_cache),
        RoamCommand::Backlinks {
            target,
            scope,
            format,
        } => handle_backlinks(target, &scope, format, no_cache),
        RoamCommand::Links {
            source,
            scope,
            format,
        } => handle_links(source, &scope, format, no_cache),
        RoamCommand::Graph { scope, format } => handle_graph(&scope, format, no_cache),
    }
}

// endregion
