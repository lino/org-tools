// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `org schema` subcommand handler printing embedded JSON schemas.

use clap::Args;

use crate::{EXIT_ERROR, EXIT_OK};

/// CLI arguments for `org schema`.
#[derive(Args)]
pub(crate) struct SchemaArgs {
    /// Schema name (diagnostic, query, roam, show, blocked, stuck, deps, clock-report, clock-status, mutation).
    pub(crate) name: String,
}

/// Static table mapping CLI schema aliases to their embedded JSON Schema documents.
const SCHEMA_REGISTRY: &[(&[&str], &str)] = &[
    (
        &["diagnostic", "diagnostics"],
        include_str!("../../schemas/org-diagnostic-output.schema.json"),
    ),
    (
        &["query", "entry", "entries"],
        include_str!("../../schemas/org-query-output.schema.json"),
    ),
    (
        &["roam", "org-roam-output"],
        include_str!("../../schemas/org-roam-output.schema.json"),
    ),
    (
        &["show"],
        include_str!("../../schemas/org-show-output.schema.json"),
    ),
    (
        &["blocked"],
        include_str!("../../schemas/org-blocked-output.schema.json"),
    ),
    (
        &["stuck"],
        include_str!("../../schemas/org-stuck-output.schema.json"),
    ),
    (
        &["deps", "dependencies"],
        include_str!("../../schemas/org-deps-output.schema.json"),
    ),
    (
        &["clock-report", "clock_report"],
        include_str!("../../schemas/org-clock-report-output.schema.json"),
    ),
    (
        &["clock-status", "clock_status"],
        include_str!("../../schemas/org-clock-status-output.schema.json"),
    ),
    (
        &["mutation", "update", "archive"],
        include_str!("../../schemas/org-mutation-output.schema.json"),
    ),
];

/// Resolves a schema name or alias to its embedded JSON Schema text.
fn lookup_schema(name: &str) -> Option<&'static str> {
    SCHEMA_REGISTRY
        .iter()
        .find(|(aliases, _)| aliases.contains(&name))
        .map(|(_, body)| *body)
}

/// Prints the JSON Schema matching `name` to stdout, or exits with `EXIT_ERROR`.
pub(crate) fn run_schema(name: &str) -> i32 {
    let Some(schema) = lookup_schema(name) else {
        eprintln!("org: unknown schema: {name}");
        eprintln!(
            "Available: diagnostic, query, roam, show, blocked, stuck, deps, clock-report, clock-status, mutation"
        );
        return EXIT_ERROR;
    };
    println!("{schema}");
    EXIT_OK
}
