// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Unified CLI for org-mode: lint, format, query, clock, export.

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

use orgfmt_core::config::Config;
use orgfmt_core::files::collect_org_files;
use orgfmt_core::output::{render_diagnostics, OutputFormat};
use orgfmt_core::runner::Runner;
use orgfmt_core::source::SourceFile;

/// Unified CLI for org-mode files.
#[derive(Parser)]
#[command(name = "org", about = "Unified CLI for org-mode: lint, format, query, clock, export")]
struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    command: OrgCommand,
}

/// Top-level subcommands.
#[derive(Subcommand)]
enum OrgCommand {
    /// Lint and format org files.
    Fmt {
        #[command(subcommand)]
        command: FmtCommand,

        /// Path to config file.
        #[arg(long, global = true)]
        config: Option<PathBuf>,
    },
    // TODO: Query, Clock, Update, Export subcommands (phases 4-7)
}

/// Subcommands for `org fmt`.
#[derive(Subcommand)]
enum FmtCommand {
    /// Lint org files and report diagnostics.
    Check {
        /// Files or directories to check.
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: OutputFormat,

        /// Auto-fix fixable issues (applies format rule fixes in-place).
        #[arg(long)]
        fix: bool,
    },
    /// Format org files.
    Format {
        /// Files or directories to format.
        paths: Vec<PathBuf>,

        /// Check mode: exit 1 if changes needed, don't modify files.
        #[arg(long)]
        check: bool,

        /// Write to stdout instead of modifying files.
        #[arg(long)]
        stdout: bool,
    },
}

/// Loads configuration from an explicit path or by searching ancestor directories.
fn load_config(cli_config: &Option<PathBuf>) -> Config {
    if let Some(path) = cli_config {
        if path.is_file() {
            match std::fs::read_to_string(path) {
                Ok(contents) => match toml::from_str::<Config>(&contents) {
                    Ok(config) => return config,
                    Err(e) => {
                        eprintln!("org: error parsing {}: {}", path.display(), e);
                        process::exit(2);
                    }
                },
                Err(e) => {
                    eprintln!("org: error reading {}: {}", path.display(), e);
                    process::exit(2);
                }
            }
        } else {
            eprintln!("org: config file not found: {}", path.display());
            process::exit(2);
        }
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Config::load(&cwd)
}

fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        OrgCommand::Fmt { command, config } => {
            let config = load_config(&config);
            let runner = Runner::new(config);
            run_fmt(command, &runner)
        }
    };

    process::exit(exit_code);
}

/// Runs the `fmt` subcommand (check or format).
fn run_fmt(command: FmtCommand, runner: &Runner) -> i32 {
    match command {
        FmtCommand::Check { paths, format, fix } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            if fix {
                let mut has_issues = false;
                for file in &files {
                    match SourceFile::from_path(file) {
                        Ok(source) => {
                            let (formatted, lint_diags) = runner.format(&source);
                            let changed = formatted != source.content;

                            if changed {
                                if let Err(e) = std::fs::write(file, &formatted) {
                                    eprintln!("org: error writing {}: {}", file.display(), e);
                                } else {
                                    println!("Fixed: {}", file.display());
                                }
                            }

                            if !lint_diags.is_empty() {
                                has_issues = true;
                                print!("{}", render_diagnostics(&lint_diags, format));
                            }
                        }
                        Err(e) => {
                            eprintln!("org: error reading {}: {}", file.display(), e);
                        }
                    }
                }
                if has_issues { 1 } else { 0 }
            } else {
                let mut all_diagnostics = Vec::new();
                for file in &files {
                    match SourceFile::from_path(file) {
                        Ok(source) => {
                            all_diagnostics.extend(runner.check(&source));
                        }
                        Err(e) => {
                            eprintln!("org: error reading {}: {}", file.display(), e);
                        }
                    }
                }

                if !all_diagnostics.is_empty() {
                    print!("{}", render_diagnostics(&all_diagnostics, format));
                    1
                } else {
                    0
                }
            }
        }
        FmtCommand::Format {
            paths,
            check,
            stdout,
        } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("org: no .org files found");
                return 2;
            }

            let mut has_changes = false;
            let mut has_lint_issues = false;

            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => {
                        let (formatted, lint_diags) = runner.format(&source);
                        let changed = formatted != source.content;

                        if changed {
                            has_changes = true;
                        }

                        if !lint_diags.is_empty() {
                            has_lint_issues = true;
                            print!(
                                "{}",
                                render_diagnostics(&lint_diags, OutputFormat::Human)
                            );
                        }

                        if check {
                            if changed {
                                println!("Would reformat: {}", file.display());
                            }
                        } else if stdout {
                            print!("{}", formatted);
                        } else if changed {
                            if let Err(e) = std::fs::write(file, &formatted) {
                                eprintln!("org: error writing {}: {}", file.display(), e);
                            } else {
                                println!("Formatted: {}", file.display());
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("org: error reading {}: {}", file.display(), e);
                    }
                }
            }

            if (check && has_changes) || has_lint_issues {
                1
            } else {
                0
            }
        }
    }
}
