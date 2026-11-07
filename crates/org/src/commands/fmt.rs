// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org fmt` subcommand definitions and check/format execution handlers.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::Subcommand;

use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::output::{render_diagnostics, OutputFormat};
use org_tools_core::runner::Runner;
use org_tools_core::source::SourceFile;

use super::read_stdin_source;
use crate::{EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

/// Subcommands for `org fmt`.
#[derive(Subcommand)]
pub(crate) enum FmtCommand {
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

/// Runs the `fmt` subcommand (`check` or `format`).
pub(crate) fn run_fmt(command: FmtCommand, runner: &Runner) -> i32 {
    match command {
        FmtCommand::Check { paths, format, fix } => run_check(&paths, format, fix, runner),
        FmtCommand::Format {
            paths,
            check,
            stdout,
        } => run_format(&paths, check, stdout, runner),
    }
}

/// Parses all readable Org files in `files` into [`OrgDocument`]s for cross-file lints.
fn parse_workspace_docs(files: &[PathBuf]) -> Vec<OrgDocument> {
    files
        .iter()
        .filter_map(|path| SourceFile::from_path(path).ok())
        .map(|src| OrgDocument::from_source(&src))
        .collect()
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

/// Applies auto-fixes to a single file for `org fmt check --fix`, returning whether lint issues remain.
fn fix_single_file(
    file: &Path,
    format: OutputFormat,
    runner: &Runner,
    workspace_docs: &[OrgDocument],
) -> bool {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("org: error reading {}: {}", file.display(), e);
            return false;
        }
    };
    let (formatted, lint_diags) = runner.format_with_workspace(&source, workspace_docs);
    if formatted != source.content {
        if let Err(e) = std::fs::write(file, &formatted) {
            eprintln!("org: error writing {}: {}", file.display(), e);
        } else {
            println!("Fixed: {}", file.display());
        }
    }
    if !lint_diags.is_empty() {
        print!("{}", render_diagnostics(&lint_diags, format));
        return true;
    }
    false
}

/// Runs `org fmt check --fix` across all collected Org files.
fn run_check_fix_files(files: &[PathBuf], format: OutputFormat, runner: &Runner) -> i32 {
    let workspace_docs = parse_workspace_docs(files);
    let mut has_issues = false;
    for file in files {
        if fix_single_file(file, format, runner, &workspace_docs) {
            has_issues = true;
        }
    }
    if has_issues {
        EXIT_ISSUES
    } else {
        EXIT_OK
    }
}

/// Runs read-only `org fmt check` across all collected Org files.
fn run_check_readonly_files(files: &[PathBuf], format: OutputFormat, runner: &Runner) -> i32 {
    let workspace_docs = parse_workspace_docs(files);
    let mut all_diagnostics = Vec::new();
    for file in files {
        match SourceFile::from_path(file) {
            Ok(source) => {
                all_diagnostics.extend(runner.check_with_workspace(&source, &workspace_docs))
            }
            Err(e) => eprintln!("org: error reading {}: {}", file.display(), e),
        }
    }
    if !all_diagnostics.is_empty() {
        print!("{}", render_diagnostics(&all_diagnostics, format));
        EXIT_ISSUES
    } else {
        EXIT_OK
    }
}

/// Dispatches `org fmt check` between piped stdin and filesystem targets.
fn run_check(paths: &[PathBuf], format: OutputFormat, fix: bool, runner: &Runner) -> i32 {
    if paths.is_empty() && !std::io::stdin().is_terminal() {
        return run_check_stdin(format, fix, runner);
    }
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    if fix {
        run_check_fix_files(&files, format, runner)
    } else {
        run_check_readonly_files(&files, format, runner)
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

/// Formats a single file and returns `(changed, has_lint_issues)`.
fn format_single_file(
    file: &Path,
    check: bool,
    stdout: bool,
    runner: &Runner,
    workspace_docs: &[OrgDocument],
) -> (bool, bool) {
    let source = match SourceFile::from_path(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("org: error reading {}: {}", file.display(), e);
            return (false, false);
        }
    };
    let (formatted, lint_diags) = runner.format_with_workspace(&source, workspace_docs);
    let changed = formatted != source.content;
    let has_lint = !lint_diags.is_empty();
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
        if let Err(e) = std::fs::write(file, &formatted) {
            eprintln!("org: error writing {}: {}", file.display(), e);
        } else {
            println!("Formatted: {}", file.display());
        }
    }
    (changed, has_lint)
}

/// Dispatches `org fmt format` between piped stdin and filesystem targets.
fn run_format(paths: &[PathBuf], check: bool, stdout: bool, runner: &Runner) -> i32 {
    if paths.is_empty() && !std::io::stdin().is_terminal() {
        return run_format_stdin(check, runner);
    }
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    let workspace_docs = parse_workspace_docs(&files);
    let mut has_changes = false;
    let mut has_lint_issues = false;
    for file in &files {
        let (changed, lint) = format_single_file(file, check, stdout, runner, &workspace_docs);
        has_changes |= changed;
        has_lint_issues |= lint;
    }
    if (check && has_changes) || has_lint_issues {
        EXIT_ISSUES
    } else {
        EXIT_OK
    }
}
