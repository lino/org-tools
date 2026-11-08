// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand definitions and iCalendar, JSCalendar, and Markdown handlers for `org export`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::Subcommand;

use org_tools_core::document::OrgDocument;
use org_tools_core::export_md::{
    discover_workspace_root, export_document_to_markdown, normalize_workspace_path,
};
use org_tools_core::files::collect_org_files;
use org_tools_core::include::expand_includes;
use org_tools_core::source::SourceFile;

use super::parse_org_files;
use crate::{date, export, EXIT_ERROR, EXIT_OK};

/// Subcommands for `org export`.
#[derive(Subcommand)]
pub(crate) enum ExportCommand {
    /// Export to iCalendar (.ics) format.
    Ical {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output file (default: stdout).
        #[arg(long, short)]
        output: Option<PathBuf>,

        /// Start date filter (YYYY-MM-DD).
        #[arg(long)]
        from: Option<String>,

        /// End date filter (YYYY-MM-DD).
        #[arg(long)]
        to: Option<String>,

        /// Filter to entries with these tags.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    /// Export to JSCalendar (JSON) format.
    Jscal {
        /// Files or directories to scan.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output file (default: stdout).
        #[arg(long, short)]
        output: Option<PathBuf>,

        /// Start date filter (YYYY-MM-DD).
        #[arg(long)]
        from: Option<String>,

        /// End date filter (YYYY-MM-DD).
        #[arg(long)]
        to: Option<String>,

        /// Filter to entries with these tags.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    /// Export Org documents to Zensical-ready GitHub Flavored Markdown (.md).
    Md {
        /// Files or directories to export.
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,

        /// Output file (single input file) or output directory (multiple files / directory).
        #[arg(long, short)]
        output: Option<PathBuf>,

        /// Workspace root directory used to resolve cross-node `[[id:...]]` links.
        #[arg(long)]
        workspace: Option<PathBuf>,

        /// Expand `#+INCLUDE:` directives before rendering Markdown.
        #[arg(long)]
        expand_includes: bool,
    },
}

/// Writes exported calendar text to `output` if provided, or prints it to stdout.
fn emit_exported_payload(payload: &str, output: Option<&PathBuf>, trailing_newline: bool) -> i32 {
    if let Some(path) = output {
        if let Err(e) = std::fs::write(path, payload) {
            eprintln!("org: error writing {}: {}", path.display(), e);
            return EXIT_ERROR;
        }
        println!("Exported to {}", path.display());
    } else if trailing_newline {
        println!("{payload}");
    } else {
        print!("{payload}");
    }
    EXIT_OK
}

/// Executes either `ical` or `jscal` calendar export.
fn execute_calendar_export(
    paths: &[PathBuf],
    output: Option<&PathBuf>,
    from: Option<&str>,
    to: Option<&str>,
    tags: &[String],
    jscal_mode: bool,
) -> i32 {
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }
    let from_date = from.and_then(date::parse_date);
    let to_date = to.and_then(date::parse_date);
    let docs = parse_org_files(&files);

    if jscal_mode {
        let json = export::jscal::export_jscal(&docs, from_date, to_date, tags);
        emit_exported_payload(&json, output, true)
    } else {
        let ical = export::ical::export_ical(&docs, from_date, to_date, tags);
        emit_exported_payload(&ical, output, false)
    }
}

/// Loads all workspace [`OrgDocument`]s needed to resolve cross-node `[[id:...]]` links.
fn load_markdown_workspace_docs(files: &[PathBuf], explicit_ws: Option<&Path>) -> Vec<OrgDocument> {
    let norm_files: Vec<PathBuf> = files.iter().map(|p| normalize_workspace_path(p)).collect();
    let anchor = common_base_directory(&norm_files);
    let ws_root = discover_workspace_root(explicit_ws, &anchor);
    let mut all_paths: BTreeSet<PathBuf> = norm_files.into_iter().collect();
    if ws_root.exists() {
        for p in collect_org_files(&[ws_root]) {
            all_paths.insert(normalize_workspace_path(&p));
        }
    }
    let ordered: Vec<PathBuf> = all_paths.into_iter().collect();
    parse_org_files(&ordered)
}

/// Computes the deepest shared directory across `files`.
fn common_base_directory(files: &[PathBuf]) -> PathBuf {
    let Some(first) = files.first() else {
        return PathBuf::from(".");
    };
    let mut base = first
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    for file in &files[1..] {
        let dir = file.parent().unwrap_or_else(|| Path::new("."));
        while !dir.starts_with(&base) {
            let Some(parent) = base.parent() else {
                return PathBuf::from(".");
            };
            base = parent.to_path_buf();
        }
    }
    base
}

/// Returns `true` when `-o` should mirror `.md` files into an output directory.
fn should_export_to_directory(paths: &[PathBuf], files: &[PathBuf], out_path: &Path) -> bool {
    files.len() > 1
        || paths.iter().any(|p| p.is_dir())
        || out_path.is_dir()
        || out_path.to_string_lossy().ends_with(['/', '\\'])
}

/// Writes `content` to `dest`, creating parent directories when needed.
fn write_markdown_file(dest: &Path, content: &str) -> Result<(), i32> {
    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("org: error creating {}: {e}", parent.display());
            return Err(EXIT_ERROR);
        }
    }
    if let Err(e) = std::fs::write(dest, content) {
        eprintln!("org: error writing {}: {e}", dest.display());
        return Err(EXIT_ERROR);
    }
    Ok(())
}

/// Writes mirrored `.md` files under `out_dir` preserving relative directory layout.
fn write_mirrored_markdown(
    paths: &[PathBuf],
    rendered: &[(PathBuf, String)],
    out_dir: &Path,
) -> i32 {
    let files: Vec<PathBuf> = rendered.iter().map(|(p, _)| p.clone()).collect();
    let base_dir = if paths.len() == 1 && paths[0].is_dir() {
        paths[0].clone()
    } else {
        common_base_directory(&files)
    };

    for (src_path, md) in rendered {
        let rel = src_path
            .strip_prefix(&base_dir)
            .ok()
            .filter(|r| !r.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(src_path.file_name().unwrap_or_default()));
        let dest = out_dir.join(rel).with_extension("md");
        if let Err(code) = write_markdown_file(&dest, md) {
            return code;
        }
    }
    EXIT_OK
}

/// Executes pure-Rust Markdown export for one or more `.org` files.
fn execute_markdown_export(
    paths: &[PathBuf],
    output: Option<&PathBuf>,
    workspace: Option<&Path>,
    expand_includes_flag: bool,
) -> i32 {
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return EXIT_ERROR;
    }

    let ws_docs = load_markdown_workspace_docs(&files, workspace);
    let mut rendered = Vec::with_capacity(files.len());
    for file in &files {
        let raw_source = match SourceFile::from_path(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("org: error reading {}: {e}", file.display());
                return EXIT_ERROR;
            }
        };
        let source = if expand_includes_flag {
            match expand_includes(&raw_source.content, file, workspace) {
                Ok(expanded) => SourceFile::new(file.clone(), expanded),
                Err(err) => {
                    eprintln!("org: {err}");
                    return EXIT_ERROR;
                }
            }
        } else {
            raw_source
        };
        let doc = OrgDocument::from_source(&source);
        rendered.push((
            file.clone(),
            export_document_to_markdown(&source, &doc, &ws_docs),
        ));
    }

    let Some(out_path) = output else {
        let joined = rendered
            .iter()
            .map(|(_, md)| md.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        print!("{joined}");
        return EXIT_OK;
    };

    if should_export_to_directory(paths, &files, out_path) {
        return write_mirrored_markdown(paths, &rendered, out_path);
    }

    match write_markdown_file(out_path, &rendered[0].1) {
        Ok(()) => EXIT_OK,
        Err(code) => code,
    }
}

/// Runs the `export` subcommand (`ical`, `jscal`, or `md`).
pub(crate) fn run_export(command: ExportCommand) -> i32 {
    match command {
        ExportCommand::Ical {
            paths,
            output,
            from,
            to,
            tags,
        } => execute_calendar_export(
            &paths,
            output.as_ref(),
            from.as_deref(),
            to.as_deref(),
            &tags,
            false,
        ),
        ExportCommand::Jscal {
            paths,
            output,
            from,
            to,
            tags,
        } => execute_calendar_export(
            &paths,
            output.as_ref(),
            from.as_deref(),
            to.as_deref(),
            &tags,
            true,
        ),
        ExportCommand::Md {
            paths,
            output,
            workspace,
            expand_includes,
        } => execute_markdown_export(
            &paths,
            output.as_ref(),
            workspace.as_deref(),
            expand_includes,
        ),
    }
}
