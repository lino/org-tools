// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand handler and multi-format table renderers (`human`, `json`, `org`, `md`) for `org query columns`.

use std::path::PathBuf;

use clap::ValueEnum;

use org_tools_core::columns::{
    build_column_view, parse_columns_format, resolve_columns_format, ColumnRow, ColumnSpec,
    ColumnView, DEFAULT_COLUMNS_FORMAT,
};
use org_tools_core::document::OrgDocument;
use org_tools_core::files::collect_org_files;
use org_tools_core::locator::{
    resolve_in_document, resolve_locator, LocatorError, OrgLocator, ResolvedEntry,
};
use org_tools_core::source::SourceFile;

use super::parse_org_files;
use crate::{EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

/// Output formats supported by `org query columns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ColumnsOutputFormat {
    /// Aligned human-readable table of columns and rows.
    Human,
    /// Structured JSON object containing `columns` and `rows`.
    Json,
    /// Org-mode pipe table with `|---+---|` separator.
    Org,
    /// GitHub Flavored Markdown pipe table.
    #[value(alias = "markdown")]
    Md,
}

/// Resolves `locator` against `paths`, falling back to matching relative file suffixes in `paths`.
fn resolve_columns_locator(
    locator: &OrgLocator,
    paths: &[PathBuf],
) -> Result<(OrgDocument, ResolvedEntry), LocatorError> {
    match resolve_locator(locator, paths) {
        Ok(resolved) => {
            let src = SourceFile::from_path(&resolved.file)
                .map_err(|e| LocatorError::IoError(e.to_string()))?;
            let doc = OrgDocument::from_source(&src);
            Ok((doc, resolved))
        }
        Err(LocatorError::FileNotFound(ref rel)) => {
            for candidate in collect_org_files(paths) {
                if candidate.ends_with(rel) {
                    let src = SourceFile::from_path(&candidate)
                        .map_err(|e| LocatorError::IoError(e.to_string()))?;
                    let doc = OrgDocument::from_source(&src);
                    let resolved = resolve_in_document(locator, &doc)?;
                    return Ok((doc, resolved));
                }
            }
            Err(LocatorError::FileNotFound(rel.clone()))
        }
        Err(err) => Err(err),
    }
}

/// Selects the effective `#+COLUMNS:` format string across multiple documents when unscoped.
fn select_workspace_format(docs: &[OrgDocument], cli_override: Option<&str>) -> String {
    if let Some(spec) = cli_override.map(str::trim).filter(|s| !s.is_empty()) {
        return spec.to_string();
    }
    for doc in docs {
        let candidate = resolve_columns_format(doc, None, None);
        if candidate != DEFAULT_COLUMNS_FORMAT {
            return candidate;
        }
    }
    DEFAULT_COLUMNS_FORMAT.to_string()
}

/// Builds a combined [`ColumnView`] across all `.org` files discovered under `paths`.
fn build_workspace_view(
    paths: &[PathBuf],
    columns_override: Option<&str>,
) -> Result<ColumnView, i32> {
    let files = collect_org_files(paths);
    if files.is_empty() {
        eprintln!("org: no .org files found");
        return Err(EXIT_ERROR);
    }
    let docs = parse_org_files(&files);
    if docs.is_empty() {
        return Err(EXIT_ERROR);
    }
    let effective_spec = select_workspace_format(&docs, columns_override);
    let columns = parse_columns_format(&effective_spec);
    let mut rows = Vec::new();
    for doc in &docs {
        let mut sub_view = build_column_view(doc, None, Some(&effective_spec));
        rows.append(&mut sub_view.rows);
    }
    Ok(ColumnView { columns, rows })
}

/// Formats a cell string for display, optionally indenting `ITEM` by outline depth in `human` mode.
fn display_cell(row: &ColumnRow, col: &ColumnSpec, min_level: usize, indent_item: bool) -> String {
    let raw = row
        .values
        .get(&col.property)
        .map(String::as_str)
        .unwrap_or("");
    if indent_item && col.property == "ITEM" && row.level > min_level {
        format!("{}{raw}", "  ".repeat(row.level - min_level))
    } else {
        raw.to_string()
    }
}

/// Computes display column widths and cell matrix for tabular text rendering.
fn build_cell_matrix(view: &ColumnView, indent_item: bool) -> (Vec<usize>, Vec<Vec<String>>) {
    let min_level = view.rows.iter().map(|r| r.level).min().unwrap_or(1);
    let mut widths: Vec<usize> = view
        .columns
        .iter()
        .map(|c| c.title.chars().count().max(1))
        .collect();
    let mut matrix = Vec::with_capacity(view.rows.len());
    for row in &view.rows {
        let mut cells = Vec::with_capacity(view.columns.len());
        for (i, col) in view.columns.iter().enumerate() {
            let cell = display_cell(row, col, min_level, indent_item);
            widths[i] = widths[i].max(cell.chars().count());
            cells.push(cell);
        }
        matrix.push(cells);
    }
    (widths, matrix)
}

/// Pads `text` on the right with ASCII spaces to reach `target_width` display columns.
fn pad_right(text: &str, target_width: usize) -> String {
    let width = text.chars().count();
    let pad = target_width.saturating_sub(width);
    format!("{text}{}", " ".repeat(pad))
}

/// Renders `view` as an aligned human-readable table.
fn render_human(view: &ColumnView) -> String {
    let (widths, matrix) = build_cell_matrix(view, true);
    let mut out = String::new();
    let headers: Vec<String> = view
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| pad_right(&c.title, widths[i]))
        .collect();
    out.push_str(headers.join("  ").trim_end());
    out.push('\n');
    let rule: Vec<String> = widths.iter().map(|&w| "-".repeat(w)).collect();
    out.push_str(&rule.join("  "));
    out.push('\n');
    for row_cells in &matrix {
        let formatted: Vec<String> = row_cells
            .iter()
            .enumerate()
            .map(|(i, cell)| pad_right(cell, widths[i]))
            .collect();
        out.push_str(formatted.join("  ").trim_end());
        out.push('\n');
    }
    out
}

/// Renders `view` as an Org-mode table (`| Title | ... |\n|---+---|\n| ... |`).
fn render_org(view: &ColumnView) -> String {
    let (widths, matrix) = build_cell_matrix(view, false);
    let mut out = String::new();
    let headers: Vec<String> = view
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!(" {} ", pad_right(&c.title, widths[i])))
        .collect();
    out.push_str(&format!("|{}|\n", headers.join("|")));
    let sep: Vec<String> = widths.iter().map(|&w| "-".repeat(w + 2)).collect();
    out.push_str(&format!("|{}|\n", sep.join("+")));
    for row_cells in &matrix {
        let cells: Vec<String> = row_cells
            .iter()
            .enumerate()
            .map(|(i, cell)| format!(" {} ", pad_right(cell, widths[i])))
            .collect();
        out.push_str(&format!("|{}|\n", cells.join("|")));
    }
    out
}

/// Renders `view` as a GitHub Flavored Markdown pipe table.
fn render_md(view: &ColumnView) -> String {
    let (mut widths, matrix) = build_cell_matrix(view, false);
    for w in &mut widths {
        *w = (*w).max(3);
    }
    let mut out = String::new();
    let headers: Vec<String> = view
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!(" {} ", pad_right(&c.title, widths[i])))
        .collect();
    out.push_str(&format!("|{}|\n", headers.join("|")));
    let sep: Vec<String> = widths
        .iter()
        .map(|&w| format!(" {} ", "-".repeat(w)))
        .collect();
    out.push_str(&format!("|{}|\n", sep.join("|")));
    for row_cells in &matrix {
        let cells: Vec<String> = row_cells
            .iter()
            .enumerate()
            .map(|(i, cell)| format!(" {} ", pad_right(cell, widths[i])))
            .collect();
        out.push_str(&format!("|{}|\n", cells.join("|")));
    }
    out
}

/// Executes `org query columns` across `paths` and returns the process exit code.
pub(crate) fn run_columns(
    paths: &[PathBuf],
    locator: Option<&str>,
    columns_override: Option<&str>,
    format: ColumnsOutputFormat,
) -> i32 {
    let view = if let Some(loc_str) = locator {
        let parsed = match OrgLocator::parse(loc_str) {
            Ok(loc) => loc,
            Err(err) => {
                eprintln!("org: {err}");
                return EXIT_ERROR;
            }
        };
        let (doc, resolved) = match resolve_columns_locator(&parsed, paths) {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("org: {err}");
                return EXIT_ERROR;
            }
        };
        let root_idx = (!resolved.is_file_node()).then_some(resolved.entry_index);
        build_column_view(&doc, root_idx, columns_override)
    } else {
        match build_workspace_view(paths, columns_override) {
            Ok(v) => v,
            Err(code) => return code,
        }
    };

    if view.rows.is_empty() {
        return EXIT_ISSUES;
    }

    match format {
        ColumnsOutputFormat::Human => print!("{}", render_human(&view)),
        ColumnsOutputFormat::Org => print!("{}", render_org(&view)),
        ColumnsOutputFormat::Md => print!("{}", render_md(&view)),
        ColumnsOutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&view).unwrap_or_default()
            );
        }
    }
    EXIT_OK
}
