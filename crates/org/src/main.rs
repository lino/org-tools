// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Unified CLI entrypoint for `org-tools`: lint, format, query, update, clock, export, archive.

mod clock;
mod commands;
mod date;
mod export;
mod query;

use std::path::{Path, PathBuf};
use std::process;

use clap::{CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::Shell;

use org_tools_core::config::Config;
use org_tools_core::runner::Runner;

use commands::archive::{run_archive, ArchiveArgs};
use commands::cache::{run_cache, CacheCommand};
use commands::clock::{run_clock, ClockCommand};
use commands::complete::{emit_completions, run_complete, CompleteArgs};
use commands::export::{run_export, ExportCommand};
use commands::fleeting::{run_fleeting, FleetingCommand};
use commands::fmt::{run_fmt, FmtCommand};
use commands::lsp::{run_lsp, LspArgs};
use commands::mcp::{run_mcp, McpArgs};
use commands::query::{run_query, QueryCommand};
use commands::query_gtd::{run_next, NextArgs};
use commands::refile::{run_refile, RefileArgs};
use commands::roam::{run_roam, RoamCommand};
use commands::schema::{run_schema, SchemaArgs};
use commands::show::{run_show, ShowArgs};
use commands::update::{run_update, UpdateCommand};

/// Operation completed successfully.
pub(crate) const EXIT_OK: i32 = 0;

/// No results found, or lint/format issues detected.
pub(crate) const EXIT_ISSUES: i32 = 1;

/// Operational error (file not found, parse error, invalid arguments).
pub(crate) const EXIT_ERROR: i32 = 2;

/// Operation requires Emacs Lisp or Emacs Calc (not supported by org-tools).
pub(crate) const EXIT_REQUIRES_EMACS: i32 = 3;

/// Unified CLI for org-mode files.
#[derive(Parser)]
#[command(
    name = "org",
    version,
    about = "Unified CLI for org-mode: lint, format, query, clock, export"
)]
struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    command: Option<OrgCommand>,

    /// Generate shell completions and exit.
    #[arg(long, value_enum)]
    completions: Option<Shell>,

    /// Bypass the `.cache/org-cache.db` SQLite index and parse `.org` files directly.
    #[arg(long, global = true)]
    no_cache: bool,
}

/// Top-level subcommands dispatched by `org`.
#[derive(Subcommand)]
enum OrgCommand {
    /// Lint and format org files.
    Fmt {
        /// Format or check subcommand.
        #[command(subcommand)]
        command: FmtCommand,

        /// Path to config file.
        #[arg(long, global = true, value_hint = ValueHint::FilePath)]
        config: Option<PathBuf>,
    },
    /// Query org entries.
    Query {
        /// Query subcommand.
        #[command(subcommand)]
        command: QueryCommand,
    },
    /// GTD next actions navigator ("what to do next" across org-roam vaults).
    Next(Box<NextArgs>),
    /// Update org entries (add IDs, modify properties).
    Update {
        /// Update subcommand.
        #[command(subcommand)]
        command: UpdateCommand,
    },
    /// Clock time tracking and reports.
    Clock {
        /// Clock subcommand.
        #[command(subcommand)]
        command: ClockCommand,
    },
    /// Export org entries to calendar or Markdown formats.
    Export {
        /// Export subcommand.
        #[command(subcommand)]
        command: ExportCommand,
    },
    /// Luhmann fleeting note queue and transformation workflows.
    Fleeting {
        /// Fleeting subcommand.
        #[command(subcommand)]
        command: FleetingCommand,
    },
    /// Inspect org-roam v2 nodes, backlinks, outgoing links, and workspace graphs.
    Roam {
        /// Roam subcommand.
        #[command(subcommand)]
        command: RoamCommand,
    },
    /// Inspect, rebuild, or clear the SQLite + FTS5 workspace cache.
    Cache {
        /// Cache management subcommand.
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Extract a scoped entry subtree or file node by locator.
    Show(ShowArgs),
    /// Move an entry subtree across files or under a new parent heading.
    Refile(RefileArgs),
    /// Serve the built-in zero-Emacs MCP JSON-RPC 2.0 server over stdio.
    Mcp(McpArgs),
    /// Serve the built-in zero-Emacs LSP JSON-RPC 2.0 server over stdio.
    Lsp(LspArgs),
    /// Archive done entries to a separate file.
    Archive(ArchiveArgs),
    /// Print the JSON schema for a given output type.
    Schema(SchemaArgs),
    /// Internal dynamic workspace-aware shell completion query command.
    #[command(hide = true)]
    Complete(CompleteArgs),
}

/// Parses an explicit `.org-tools.toml` file or exits with [`EXIT_ERROR`].
fn load_explicit_config(path: &Path) -> Config {
    if !path.is_file() {
        eprintln!("org: config file not found: {}", path.display());
        process::exit(EXIT_ERROR);
    }
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("org: error reading {}: {}", path.display(), e);
            process::exit(EXIT_ERROR);
        }
    };
    match toml::from_str::<Config>(&contents) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("org: error parsing {}: {}", path.display(), e);
            process::exit(EXIT_ERROR);
        }
    }
}

/// Loads configuration from an explicit path or by searching ancestor directories.
fn load_config(cli_config: &Option<PathBuf>) -> Config {
    if let Some(path) = cli_config {
        return load_explicit_config(path);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match Config::load(&cwd) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("org: {e}");
            process::exit(EXIT_ERROR);
        }
    }
}

/// Dispatches agent, graph, cache, archive, and schema subcommands.
fn dispatch_agent_command(command: OrgCommand, no_cache: bool) -> i32 {
    match command {
        OrgCommand::Roam { command } => run_roam(command, no_cache),
        OrgCommand::Cache { command } => run_cache(command),
        OrgCommand::Show(args) => run_show(args),
        OrgCommand::Refile(args) => run_refile(args, no_cache),
        OrgCommand::Mcp(args) => run_mcp(args, no_cache),
        OrgCommand::Lsp(args) => run_lsp(args, no_cache),
        OrgCommand::Archive(args) => run_archive(args),
        OrgCommand::Schema(args) => run_schema(&args.name),
        OrgCommand::Complete(args) => run_complete(args),
        _ => unreachable!("non-agent commands handled in dispatch_command"),
    }
}

/// Dispatches a parsed [`OrgCommand`] to its corresponding module handler.
fn dispatch_command(command: OrgCommand, no_cache: bool) -> i32 {
    match command {
        OrgCommand::Fmt { command, config } => {
            let runner = Runner::new(load_config(&config));
            run_fmt(command, &runner)
        }
        OrgCommand::Query { command } => run_query(command, no_cache),
        OrgCommand::Next(args) => run_next(*args),
        OrgCommand::Update { command } => run_update(command),
        OrgCommand::Clock { command } => run_clock(command),
        OrgCommand::Export { command } => run_export(command),
        OrgCommand::Fleeting { command } => run_fleeting(command, no_cache),
        other => dispatch_agent_command(other, no_cache),
    }
}

/// Parses CLI arguments, handles `--completions` / help, and exits with the subcommand code.
fn main() {
    let cli = Cli::parse();

    if let Some(shell) = cli.completions {
        emit_completions(shell, Cli::command());
        process::exit(EXIT_OK);
    }

    let Some(command) = cli.command else {
        Cli::command().print_help().ok();
        process::exit(EXIT_OK);
    };

    process::exit(dispatch_command(command, cli.no_cache));
}
