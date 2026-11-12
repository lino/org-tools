// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structured record and output envelope types for `org update add-id`.

use std::path::Path;

use org_tools_core::document::OrgDocument;
use org_tools_core::source::SourceFile;
use serde::Serialize;

use super::super::{MutationDetail, MutationTelemetry};

/// A single `:ID:` assignment record emitted by `org update add-id --format json`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AddIdRecord {
    /// File path containing the target element.
    pub(crate) file: String,
    /// Locator string identifying the target element.
    pub(crate) locator: String,
    /// Heading star level (`0` for file-level drawer, `>= 1` for headings).
    pub(crate) level: usize,
    /// 1-based line number of the `:PROPERTIES:` drawer or heading.
    pub(crate) line: usize,
    /// Minted or existing `:ID:` value.
    pub(crate) id: String,
    /// Optional `:ID_SEED:` URN string when provenance was written.
    pub(crate) id_seed: Option<String>,
    /// Whether a new `:ID:` was minted (`false` if existing ID was preserved).
    pub(crate) created: bool,
    /// Whether the active workspace index was updated with this mutation.
    pub(crate) cache_synced: bool,
}

/// JSON output envelope for `org update add-id --format json`.
#[derive(Serialize)]
pub(crate) struct AddIdOutput {
    /// Canonical subcommand identifier (`"add-id"`).
    pub(crate) command: String,
    /// Number of files modified.
    pub(crate) files_modified: usize,
    /// Total number of entries affected.
    pub(crate) entries_affected: usize,
    /// Whether file writes were skipped (`--dry-run`).
    pub(crate) dry_run: bool,
    /// Per-entry mutation details conforming to `MutationDetail`.
    pub(crate) details: Vec<MutationDetail>,
    /// Operational errors collected during execution.
    pub(crate) errors: Vec<String>,
    /// Execution telemetry conforming to `MutationTelemetry`.
    pub(crate) telemetry: MutationTelemetry,
    /// Whether `.cache/org-cache.db` was synchronized inline.
    pub(crate) cache_synced: bool,
    /// Machine-readable records for all inspected targets.
    pub(crate) records: Vec<AddIdRecord>,
}

/// Filters entry indices by `--max-level`.
pub(crate) fn filter_max_level(
    doc: &OrgDocument,
    indices: &[usize],
    max_level: Option<usize>,
) -> Vec<usize> {
    let Some(max) = max_level else {
        return indices.to_vec();
    };
    indices
        .iter()
        .copied()
        .filter(|&idx| doc.entries.get(idx).is_some_and(|e| e.level <= max))
        .collect()
}

/// Builds per-entry [`MutationDetail`] and [`AddIdRecord`] rows for modified headings.
pub(crate) fn collect_heading_results(
    file: &Path,
    before: &OrgDocument,
    after_content: &str,
    records: &mut Vec<AddIdRecord>,
    details: &mut Vec<MutationDetail>,
) {
    let after_source = SourceFile::new(file, after_content.to_string());
    let after_doc = OrgDocument::from_source(&after_source);
    let file_str = file.display().to_string();

    for (idx, entry) in after_doc.entries.iter().enumerate() {
        if let Some(id) = entry.properties.get("ID") {
            let was_new = before
                .entries
                .get(idx)
                .is_some_and(|p| !p.properties.contains_key("ID"));
            if was_new {
                details.push(MutationDetail {
                    file: file_str.clone(),
                    line: entry.heading_line,
                    action: "added-id".to_string(),
                    detail: Some(id.clone()),
                });
                records.push(AddIdRecord {
                    file: file_str.clone(),
                    locator: format!("{file_str}::*{}", entry.title),
                    level: entry.level,
                    line: entry.heading_line,
                    id: id.clone(),
                    id_seed: entry.properties.get("ID_SEED").cloned(),
                    created: true,
                    cache_synced: false,
                });
            }
        }
    }
}
