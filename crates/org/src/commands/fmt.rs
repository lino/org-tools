// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org fmt` subcommand definitions and check/format execution handlers.

mod scope;

use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueHint};

use org_tools_core::cache::WorkspaceCache;
use org_tools_core::export_md::normalize_workspace_path;
use org_tools_core::files::{collect_org_files, write_file_atomic};
use org_tools_core::output::{render_diagnostics, OutputFormat};
use org_tools_core::runner::Runner;
use org_tools_core::source::SourceFile;

use self::scope::{resolve_fmt_scope, ResolvedScope, ScopeArgs};
use super::read_stdin_source;
use crate::{EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

/// Subcommands for `org fmt`.
#[derive(Subcommand)]
pub(crate) enum FmtCommand {
    /// Lint org files and report diagnostics.
    Check {
        /// Files or directories to check.
        #[arg(value_hint = ValueHint::AnyPath)]
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: OutputFormat,

        /// Auto-fix fixable issues (applies format rule fixes in-place).
        #[arg(long)]
        fix: bool,

        /// Resolution scope and workspace configuration arguments.
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Format org files.
    Format {
        /// Files or directories to format.
        #[arg(value_hint = ValueHint::AnyPath)]
        paths: Vec<PathBuf>,

        /// Check mode: exit 1 if changes needed, don't modify files.
        #[arg(long)]
        check: bool,

        /// Write to stdout instead of modifying files.
        #[arg(long)]
        stdout: bool,

        /// Resolution scope and workspace configuration arguments.
        #[command(flatten)]
        scope: ScopeArgs,
    },
}

/// Runs the `fmt` subcommand (`check` or `format`).
pub(crate) fn run_fmt(command: FmtCommand, runner: &Runner) -> i32 {
    match command {
        FmtCommand::Check {
            paths,
            format,
            fix,
            scope,
        } => run_check(&paths, format, fix, &scope, runner),
        FmtCommand::Format {
            paths,
            check,
            stdout,
            scope,
        } => run_format(&paths, check, stdout, &scope, runner),
    }
}

/// Finds any ancestor directory of `file` containing `.cache/org-cache.db`.
fn find_ancestor_with_cache(file: &Path) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if WorkspaceCache::resolve_db_path(&cwd).is_file() {
        return Some(normalize_workspace_path(&cwd));
    }
    let norm_file = normalize_workspace_path(file);
    for ancestor in norm_file.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            break;
        }
        if WorkspaceCache::resolve_db_path(ancestor).is_file() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

/// Synchronizes written files with any active ancestor `.cache/org-cache.db`.
fn sync_fmt_written_files(files: &[PathBuf]) {
    let mut roots = HashSet::new();
    for file in files {
        if let Some(r) = find_ancestor_with_cache(file) {
            roots.insert(r);
        }
    }
    for root in roots {
        let _ = WorkspaceCache::sync_written_files(&root, files);
    }
}

/// Constructs an effective runner honoring the resolved [`org_tools_core::config::IdScope`].
fn build_effective_runner(base_runner: &Runner, resolved: &ResolvedScope) -> Runner {
    let mut config = base_runner.config().clone();
    config.lint.id_scope = resolved.effective_scope;
    Runner::new(config)
}

/// Executes `org fmt check` on piped stdin when no file paths are provided.
fn run_check_stdin(format: OutputFormat, fix: bool, runner: &Runner) -> i32 {
    let source = match read_stdin_source() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return EXIT_ERROR;
        }
    };
    if fix {
        let (formatted, lint_diags) = runner.format(&source);
        print!("{formatted}");
        if !lint_diags.is_empty() {
            eprint!("{}", render_diagnostics(&lint_diags, format));
            return EXIT_ISSUES;
        }
        return EXIT_OK;
    }
    let diagnostics = runner.check(&source);
    if !diagnostics.is_empty() {
        print!("{}", render_diagnostics(&diagnostics, format));
        return EXIT_ISSUES;
    }
    EXIT_OK
}

/// Applies auto-fixes to one file for `org fmt check --fix`, returning `(has_lint_issues, has_io_error)`.
fn fix_single_file(
    file: &Path,
    format: OutputFormat,
    runner: &Runner,
    resolved: &ResolvedScope,
) -> (bool, bool) {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("org: error reading {}: {}", file.display(), e);
            return (false, true);
        }
    };
    let (formatted, lint_diags) = runner.format_with_scope(
        &source,
        Some(&resolved.workspace_docs),
        Some(&resolved.scoped_ids),
        Some(&resolved.id_locations),
    );
    let mut io_error = false;
    if formatted != source.content {
        if let Err(e) = write_file_atomic(file, &formatted) {
            eprintln!("org: error writing {}: {}", file.display(), e);
            io_error = true;
        } else {
            println!("Fixed: {}", file.display());
        }
    }
    if !lint_diags.is_empty() {
        print!("{}", render_diagnostics(&lint_diags, format));
        return (true, io_error);
    }
    (false, io_error)
}

/// Runs `org fmt check --fix` across all collected Org files.
fn run_check_fix_files(
    files: &[PathBuf],
    format: OutputFormat,
    scope: &ScopeArgs,
    runner: &Runner,
) -> i32 {
    let resolved = resolve_fmt_scope(files, scope, runner.config());
    let effective_runner = build_effective_runner(runner, &resolved);
    let mut has_issues = false;
    let mut has_io_error = false;
    for file in files {
        let (issues, io_err) = fix_single_file(file, format, &effective_runner, &resolved);
        has_issues |= issues;
        has_io_error |= io_err;
    }
    sync_fmt_written_files(files);
    if has_io_error {
        EXIT_ERROR
    } else if has_issues {
        EXIT_ISSUES
    } else {
        EXIT_OK
    }
}

/// Runs read-only `org fmt check` across all collected Org files.
fn run_check_readonly_files(
    files: &[PathBuf],
    format: OutputFormat,
    scope: &ScopeArgs,
    runner: &Runner,
) -> i32 {
    let resolved = resolve_fmt_scope(files, scope, runner.config());
    let effective_runner = build_effective_runner(runner, &resolved);
    let mut all_diagnostics = Vec::new();
    let mut has_io_error = false;
    for file in files {
        match SourceFile::from_path(file) {
            Ok(source) => {
                all_diagnostics.extend(effective_runner.check_with_scope(
                    &source,
                    Some(&resolved.workspace_docs),
                    Some(&resolved.scoped_ids),
                    Some(&resolved.id_locations),
                ));
            }
            Err(e) => {
                eprintln!("org: error reading {}: {}", file.display(), e);
                has_io_error = true;
            }
        }
    }
    if has_io_error {
        EXIT_ERROR
    } else if !all_diagnostics.is_empty() {
        print!("{}", render_diagnostics(&all_diagnostics, format));
        EXIT_ISSUES
    } else {
        EXIT_OK
    }
}

/// Dispatches `org fmt check` between piped stdin and filesystem targets.
fn run_check(
    paths: &[PathBuf],
    format: OutputFormat,
    fix: bool,
    scope: &ScopeArgs,
    runner: &Runner,
) -> i32 {
    if paths.is_empty() && !std::io::stdin().is_terminal() {
        return run_check_stdin(format, fix, runner);
    }
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    if fix {
        run_check_fix_files(&files, format, scope, runner)
    } else {
        run_check_readonly_files(&files, format, scope, runner)
    }
}

/// Executes `org fmt format` on piped stdin when no file paths are provided.
fn run_format_stdin(check: bool, runner: &Runner) -> i32 {
    let source = match read_stdin_source() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return EXIT_ERROR;
        }
    };
    let (formatted, lint_diags) = runner.format(&source);
    let changed = formatted != source.content;
    if !lint_diags.is_empty() {
        eprint!("{}", render_diagnostics(&lint_diags, OutputFormat::Human));
    }
    if check {
        if changed {
            eprintln!("Would reformat: <stdin>");
        }
        return if changed { EXIT_ISSUES } else { EXIT_OK };
    }
    print!("{formatted}");
    EXIT_OK
}

/// Formats a single file and returns `(changed, has_lint_issues, has_io_error)`.
fn format_single_file(
    file: &Path,
    check: bool,
    stdout: bool,
    runner: &Runner,
    resolved: &ResolvedScope,
) -> (bool, bool, bool) {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("org: error reading {}: {}", file.display(), e);
            return (false, false, true);
        }
    };
    let (formatted, lint_diags) = runner.format_with_scope(
        &source,
        Some(&resolved.workspace_docs),
        Some(&resolved.scoped_ids),
        Some(&resolved.id_locations),
    );
    let changed = formatted != source.content;
    let has_lint = !lint_diags.is_empty();
    let mut io_error = false;
    if has_lint {
        print!("{}", render_diagnostics(&lint_diags, OutputFormat::Human));
    }
    if check {
        if changed {
            println!("Would reformat: {}", file.display());
        }
    } else if stdout {
        print!("{formatted}");
    } else if changed {
        if let Err(e) = write_file_atomic(file, &formatted) {
            eprintln!("org: error writing {}: {}", file.display(), e);
            io_error = true;
        } else {
            println!("Formatted: {}", file.display());
        }
    }
    (changed, has_lint, io_error)
}

/// Dispatches `org fmt format` between piped stdin and filesystem targets.
fn run_format(
    paths: &[PathBuf],
    check: bool,
    stdout: bool,
    scope: &ScopeArgs,
    runner: &Runner,
) -> i32 {
    if paths.is_empty() && !std::io::stdin().is_terminal() {
        return run_format_stdin(check, runner);
    }
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    let resolved = resolve_fmt_scope(&files, scope, runner.config());
    let effective_runner = build_effective_runner(runner, &resolved);
    let mut has_changes = false;
    let mut has_lint_issues = false;
    let mut has_io_error = false;
    let mut written = Vec::new();
    for file in &files {
        let (changed, lint, io_err) =
            format_single_file(file, check, stdout, &effective_runner, &resolved);
        has_changes |= changed;
        has_lint_issues |= lint;
        has_io_error |= io_err;
        if changed && !check && !stdout {
            written.push(file.clone());
        }
    }
    if !written.is_empty() {
        sync_fmt_written_files(&written);
    }
    if has_io_error {
        EXIT_ERROR
    } else if (check && has_changes) || has_lint_issues {
        EXIT_ISSUES
    } else {
        EXIT_OK
    }
}
