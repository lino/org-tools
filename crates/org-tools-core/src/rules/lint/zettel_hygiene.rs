// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opt-in Zettelkasten and authoring hygiene lint rules (`W038`, `W039`, `W040`).
//!
//! Spec:
//! - Org Mode Manual: [§2.1 Headings](https://orgmode.org/manual/Headings.html),
//!   [§2.7 Plain Lists](https://orgmode.org/manual/Plain-Lists.html),
//!   [§3.1 Built-in Table Editor](https://orgmode.org/manual/Built_002din-Table-Editor.html)
//! - Org Syntax: <https://orgmode.org/worg/org-syntax.html>
//! - Ecosystem Extensions: `docs/org-ecosystem.org`

use crate::config::Config;
use crate::diagnostic::{Diagnostic, Severity};
use crate::document::OrgDocument;
use crate::roam::lookup_prop_ci;
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::list::parse_list_item;
use crate::rules::{LintContext, LintRule};

// region Rule W038: Zettel Task Hygiene

/// Default maximum character width for non-separator Org table cells when `W040` is enabled.
pub const DEFAULT_MAX_TABLE_CELL_CHARS: usize = 120;

/// Warns when active TODO keywords or checklists appear in concept notes (`W038`).
///
/// Citations:
/// - Org Mode Manual: [§2.1 Headings](https://orgmode.org/manual/Headings.html),
///   [§2.7 Plain Lists](https://orgmode.org/manual/Plain-Lists.html)
/// - Org Syntax: [Headings](https://orgmode.org/worg/org-syntax.html#Headings),
///   [Plain Lists](https://orgmode.org/worg/org-syntax.html#Plain_Lists_and_Items)
/// - Ecosystem Spec: `docs/org-ecosystem.org`
pub struct ZettelTaskHygiene;

impl LintRule for ZettelTaskHygiene {
    /// Returns the canonical warning rule identifier `"W038"`.
    fn id(&self) -> &'static str {
        "W038"
    }

    /// Returns the kebab-case rule slug `"zettel-task-hygiene"`.
    fn name(&self) -> &'static str {
        "zettel-task-hygiene"
    }

    /// Returns a one-line summary of the task hygiene check.
    fn description(&self) -> &'static str {
        "Warn when TODO tasks or checklists appear in concept notes"
    }

    /// Returns `true` when explicitly enabled via `[lint.zettel]` or `[lint] enable`.
    fn is_enabled(&self, config: &Config) -> bool {
        if config.is_rule_disabled(self.id(), self.name()) {
            return false;
        }
        config.lint.zettel.forbid_tasks
            || config.is_rule_enabled(self.id(), self.name())
            || config.is_rule_enabled(self.id(), "zettel_hygiene")
    }

    /// Flags active TODO keywords and checklists inside concept notes or forbidden files.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let doc = OrgDocument::from_source_with_config(ctx.source, ctx.config);
        let is_concept = lookup_prop_ci(&doc.file_properties, "TYPE")
            .map(str::trim)
            .is_some_and(|v| v.eq_ignore_ascii_case("concept"));

        if !ctx.config.lint.zettel.forbid_tasks && !is_concept {
            return Vec::new();
        }

        let mut diagnostics = Vec::new();
        let regions = protected_regions(&ctx.source.content);

        // Check active TODO keywords on headings.
        for entry in &doc.entries {
            if let Some(ref kw) = entry.keyword {
                if !doc.todo_keywords.is_done(kw) && doc.todo_keywords.contains(kw) {
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: entry.heading_line,
                        column: 1,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!("active task keyword '{kw}' forbidden in concept note"),
                        fix: None,
                    });
                }
            }
        }

        // Check checklists in plain lists.
        let mut offset = 0;
        let mut in_drawer = false;
        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            if is_protected(i, &regions) {
                offset += line.len() + 1;
                continue;
            }

            if in_drawer {
                if trimmed.eq_ignore_ascii_case(":END:") {
                    in_drawer = false;
                }
                offset += line.len() + 1;
                continue;
            }
            if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
                in_drawer = true;
                offset += line.len() + 1;
                continue;
            }

            if !trimmed.starts_with('#') {
                if let Some(item) = parse_list_item(raw) {
                    if item.checkbox.is_some() {
                        let (line_num, col) = ctx.source.line_col(offset);
                        diagnostics.push(Diagnostic {
                            file: ctx.source.path.clone(),
                            line: line_num,
                            column: col,
                            severity: Severity::Warning,
                            rule_id: self.id(),
                            rule: self.name(),
                            message: "checklist item forbidden in concept note".to_string(),
                            fix: None,
                        });
                    }
                }
            }
            offset += line.len() + 1;
        }

        diagnostics.sort_by_key(|d| (d.line, d.column));
        diagnostics
    }
}

// endregion

// region Rule W039: Ephemeral Header

/// Warns on boilerplate `#+DATE:` and `#+AUTHOR:` headers (`W039`).
///
/// Citations:
/// - Org Mode Manual: [§2.1 Headings](https://orgmode.org/manual/Headings.html)
/// - Org Syntax: [Keywords](https://orgmode.org/worg/org-syntax.html#Keywords)
/// - Ecosystem Spec: `docs/org-ecosystem.org`
pub struct EphemeralHeader;

impl LintRule for EphemeralHeader {
    /// Returns the canonical warning rule identifier `"W039"`.
    fn id(&self) -> &'static str {
        "W039"
    }

    /// Returns the kebab-case rule slug `"ephemeral-header"`.
    fn name(&self) -> &'static str {
        "ephemeral-header"
    }

    /// Returns a one-line summary of the ephemeral header check.
    fn description(&self) -> &'static str {
        "Warn on boilerplate #+DATE: and #+AUTHOR: headers"
    }

    /// Returns `true` when explicitly enabled via `[lint.authoring]` or `[lint] enable`.
    fn is_enabled(&self, config: &Config) -> bool {
        if config.is_rule_disabled(self.id(), self.name()) {
            return false;
        }
        config.lint.authoring.forbid_ephemeral_headers
            || config.is_rule_enabled(self.id(), self.name())
    }

    /// Scans source lines for `#+DATE:` and `#+AUTHOR:` keywords outside protected blocks.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let regions = protected_regions(&ctx.source.content);
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            if !is_protected(i, &regions) {
                let upper = trimmed.to_ascii_uppercase();
                let matched = if upper.starts_with("#+DATE:") {
                    Some("#+DATE:")
                } else if upper.starts_with("#+AUTHOR:") {
                    Some("#+AUTHOR:")
                } else {
                    None
                };

                if let Some(hdr) = matched {
                    let (line_num, col) = ctx.source.line_col(offset);
                    diagnostics.push(Diagnostic {
                        file: ctx.source.path.clone(),
                        line: line_num,
                        column: col,
                        severity: Severity::Warning,
                        rule_id: self.id(),
                        rule: self.name(),
                        message: format!(
                            "ephemeral header '{hdr}' should be omitted in favour of drawer or git metadata"
                        ),
                        fix: None,
                    });
                }
            }
            offset += line.len() + 1;
        }

        diagnostics
    }
}

// endregion

// region Rule W040: Oversized Table Cell

/// Warns when non-separator Org table cells exceed character width limits (`W040`).
///
/// Citations:
/// - Org Mode Manual: [§3.1 Built-in Table Editor](https://orgmode.org/manual/Built_002din-Table-Editor.html)
/// - Org Syntax: [Tables](https://orgmode.org/worg/org-syntax.html#Tables)
/// - Ecosystem Spec: `docs/org-ecosystem.org`
pub struct OversizedTableCell;

impl LintRule for OversizedTableCell {
    /// Returns the canonical warning rule identifier `"W040"`.
    fn id(&self) -> &'static str {
        "W040"
    }

    /// Returns the kebab-case rule slug `"oversized-table-cell"`.
    fn name(&self) -> &'static str {
        "oversized-table-cell"
    }

    /// Returns a one-line summary of the oversized table cell check.
    fn description(&self) -> &'static str {
        "Warn when Org table cells exceed maximum character width"
    }

    /// Returns `true` when explicitly enabled via `[lint.authoring]` or `[lint] enable`.
    fn is_enabled(&self, config: &Config) -> bool {
        if config.is_rule_disabled(self.id(), self.name()) {
            return false;
        }
        config.lint.authoring.max_table_cell_chars.is_some()
            || config.is_rule_enabled(self.id(), self.name())
    }

    /// Scans table lines and flags non-separator cells exceeding `max_table_cell_chars`.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let max_chars = ctx
            .config
            .lint
            .authoring
            .max_table_cell_chars
            .unwrap_or(DEFAULT_MAX_TABLE_CELL_CHARS);
        let regions = protected_regions(&ctx.source.content);
        let mut diagnostics = Vec::new();
        let mut offset = 0;

        for (i, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = raw.trim();

            if !is_protected(i, &regions)
                && trimmed.starts_with('|')
                && !is_table_separator_row(trimmed)
            {
                check_table_row_cells(
                    ctx,
                    raw,
                    offset,
                    max_chars,
                    self.id(),
                    self.name(),
                    &mut diagnostics,
                );
            }
            offset += line.len() + 1;
        }

        diagnostics
    }
}

/// Returns `true` when `trimmed_line` is a table horizontal separator (e.g. `|---+---|`).
fn is_table_separator_row(trimmed_line: &str) -> bool {
    trimmed_line
        .chars()
        .all(|c| c == '|' || c == '-' || c == '+' || c.is_whitespace())
}

/// Checks each cell in `raw_line` against `max_chars` and pushes diagnostics.
fn check_table_row_cells(
    ctx: &LintContext,
    raw_line: &str,
    line_offset: usize,
    max_chars: usize,
    rule_id: &'static str,
    rule_name: &'static str,
    out: &mut Vec<Diagnostic>,
) {
    let parts: Vec<&str> = raw_line.split('|').collect();
    if parts.len() < 2 {
        return;
    }

    // Skip leading empty slice before first '|' and trailing empty slice after last '|'.
    let mut current_byte = raw_line.find('|').unwrap_or(0);
    for cell in &parts[1..parts.len() - 1] {
        current_byte += 1; // Skip the '|'
        let trimmed = cell.trim();
        let char_count = trimmed.chars().count();
        if char_count > max_chars {
            let (line_num, col) = ctx.source.line_col(line_offset + current_byte);
            out.push(Diagnostic {
                file: ctx.source.path.clone(),
                line: line_num,
                column: col,
                severity: Severity::Warning,
                rule_id,
                rule: rule_name,
                message: format!("table cell exceeds {max_chars} characters ({char_count} chars)"),
                fix: None,
            });
        }
        current_byte += cell.len();
    }
}

// endregion
