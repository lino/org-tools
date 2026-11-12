// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org fleeting list` subcommand execution.

use std::path::PathBuf;

use clap::ValueEnum;
use serde::Serialize;

use org_tools_core::config::Config;
use org_tools_core::fleeting::{scan_fleeting_items, FleetingItem, FleetingListEnvelope};

use super::FleetingListArgs;
use crate::{EXIT_ERROR, EXIT_ISSUES, EXIT_OK};

// region Types & Resolution Helpers

/// Output format for `org fleeting list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FleetingListFormat {
    /// Human-readable tabular list.
    #[default]
    Human,
    /// Detailed JSON array and count envelope.
    Json,
    /// Canonical `OrgLocator` strings, one per line.
    Locator,
}

/// Resolves search root paths from CLI arguments and configuration.
fn resolve_search_roots(args: &FleetingListArgs) -> Result<Vec<PathBuf>, String> {
    if !args.paths.is_empty() {
        return Ok(args.paths.clone());
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let base = args.workspace.as_deref().unwrap_or(&cwd);
    let cfg = Config::load_for_dir(base).map_err(|e| e.to_string())?;

    if let Some(ref sel) = args.roam_dir {
        let (_, path) = cfg
            .resolve_roam_dir(sel)
            .ok_or_else(|| format!("unknown roam directory: {sel}"))?;
        if !path.exists() {
            return Err(format!("roam directory not found: {}", path.display()));
        }
        return Ok(vec![path]);
    }

    let configured = cfg.roam_directories();
    if !configured.is_empty() {
        let valid: Vec<_> = configured
            .into_iter()
            .map(|(_, p)| p)
            .filter(|p| p.exists())
            .collect();
        if !valid.is_empty() {
            return Ok(valid);
        }
    }
    Ok(vec![base.to_path_buf()])
}

// endregion

// region Output Renderers

/// Renders matched fleeting items in the requested format.
fn render_list_output(items: &[FleetingItem], format: FleetingListFormat) {
    match format {
        FleetingListFormat::Json => {
            let env = FleetingListEnvelope {
                count: items.len(),
                items: items.to_vec(),
            };
            println!("{}", serde_json::to_string_pretty(&env).unwrap_or_default());
        }
        FleetingListFormat::Locator => {
            for item in items {
                println!("{}", item.locator);
            }
        }
        FleetingListFormat::Human => {
            for item in items {
                println!(
                    "[{:<17}]  {:<40}  ({})",
                    item.status.as_str(),
                    item.title,
                    item.locator
                );
            }
        }
    }
}

// endregion

// region Public Runner

/// Executes `org fleeting list`.
pub(crate) fn run_list(args: FleetingListArgs) -> i32 {
    let roots = match resolve_search_roots(&args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("org: {e}");
            return EXIT_ERROR;
        }
    };

    let items = scan_fleeting_items(&roots, args.status);
    if items.is_empty() {
        return EXIT_ISSUES;
    }

    render_list_output(&items, args.format);
    EXIT_OK
}

// endregion
