// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Subcommand handler listing, printing, and exporting embedded JSON Schema Draft 2020-12 contracts for `org schema`.

use std::fs;
use std::path::{Path, PathBuf};

use clap::builder::{PossibleValue, TypedValueParser};
use clap::{Args, ValueEnum, ValueHint};
use serde::Serialize;

use crate::{EXIT_ERROR, EXIT_OK};

// region CLI Arguments & Types

/// Custom [`TypedValueParser`] advertising schema `possible_values()` to `clap_complete` while deferring validation to `run_schema`.
#[derive(Clone, Copy)]
struct SchemaNameParser;

impl TypedValueParser for SchemaNameParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        clap::builder::StringValueParser::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(Box::new(SCHEMA_REGISTRY.iter().map(|entry| {
            PossibleValue::new(entry.name)
                .aliases(entry.aliases.iter().copied().filter(|&a| a != entry.name))
        })))
    }
}

/// Returns the [`SchemaNameParser`] registering canonical schema names and their hidden CLI aliases.
fn schema_name_parser() -> SchemaNameParser {
    SchemaNameParser
}

/// Output format for `org schema --list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub(crate) enum SchemaListFormat {
    /// Human-readable table of registered JSON Schema contracts.
    #[default]
    Human,
    /// Machine-readable JSON array of registered schema metadata.
    Json,
}

/// Flattened CLI arguments and flags for `org schema`.
#[derive(Debug, Clone, Args)]
pub(crate) struct SchemaCommandArgs {
    /// Schema name (`fmt`, `query`, `agenda`, `columns`, `show`, `update`, `clock-status`, `clock-report`, `jscalendar`, `roam`, `fleeting`, `blocked`, `stuck`, `deps`).
    #[arg(
        value_name = "NAME",
        required_unless_present = "list",
        value_parser = schema_name_parser()
    )]
    pub(crate) schema_name: Option<String>,

    /// List all registered JSON Schema Draft 2020-12 artifacts and exit.
    #[arg(long)]
    pub(crate) list: bool,

    /// Output format for `--list` (`human` or `json`).
    #[arg(long, value_enum, default_value_t = SchemaListFormat::Human)]
    pub(crate) format: SchemaListFormat,

    /// Write the selected JSON Schema or catalog output to a file path instead of stdout.
    #[arg(short = 'o', long = "output", value_name = "PATH", value_hint = ValueHint::FilePath)]
    pub(crate) output: Option<PathBuf>,
}

/// Top-level `org schema` CLI argument wrapper passed from `main.rs`.
#[derive(Debug, Clone, Args)]
pub(crate) struct SchemaArgs {
    /// Flattened schema command arguments passed to [`run_schema`].
    #[command(flatten)]
    pub(crate) name: SchemaCommandArgs,
}

/// Static descriptor for a single registered JSON Schema Draft 2020-12 artifact.
struct SchemaEntry {
    /// Canonical primary CLI name (e.g., `"fmt"`, `"columns"`).
    name: &'static str,
    /// Accepted CLI aliases including the canonical name.
    aliases: &'static [&'static str],
    /// Canonical `$id` URI of the JSON Schema.
    id: &'static str,
    /// Repository-relative `schemas/*.schema.json` file path.
    path: &'static str,
    /// Concise human-readable description of the schema contract.
    description: &'static str,
    /// Embedded JSON Schema Draft 2020-12 document text.
    body: &'static str,
}

/// Serializable metadata row emitted by `org schema --list --format json`.
#[derive(Debug, Serialize)]
struct SchemaSummary<'a> {
    /// Canonical schema name.
    name: &'a str,
    /// Accepted CLI aliases.
    aliases: &'a [&'a str],
    /// Canonical `$id` URI.
    id: &'a str,
    /// Repository-relative schema file path.
    path: &'a str,
    /// Human-readable description of the JSON payload validated by this schema.
    description: &'a str,
}

// endregion

// region Embedded Schema Registry

/// Embedded JSON Schema Draft 2020-12 contract for `org export jscalendar`.
const JSCALENDAR_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://org-tools.dev/schemas/org-jscalendar-output.schema.json",
  "title": "org export jscalendar JSON output",
  "description": "Schema for RFC 8984 JSCalendar Event and Task objects emitted by 'org export jscalendar'.",
  "type": "array",
  "items": {
    "type": "object",
    "required": ["@type", "title", "x-org-file", "x-org-line"],
    "properties": {
      "@type": {
        "type": "string",
        "enum": ["Event", "Task"],
        "description": "JSCalendar object type ('Event' for SCHEDULED, 'Task' for DEADLINE)."
      },
      "uid": {
        "type": "string",
        "description": "Optional entry :ID: identifier."
      },
      "title": {
        "type": "string",
        "description": "Heading title text."
      },
      "start": {
        "type": "string",
        "description": "ISO 8601 start date or datetime for Event objects."
      },
      "due": {
        "type": "string",
        "description": "ISO 8601 due date or datetime for Task objects."
      },
      "priority": {
        "type": "integer",
        "minimum": 1,
        "maximum": 9,
        "description": "Mapped numeric priority (A=1, B=5, C=9)."
      },
      "status": {
        "type": "string",
        "description": "Entry TODO state keyword when present."
      },
      "keywords": {
        "type": "object",
        "additionalProperties": { "type": "boolean" },
        "description": "Tag map keyed by inherited tag name."
      },
      "x-org-file": {
        "type": "string",
        "description": "Source Org file path."
      },
      "x-org-line": {
        "type": "integer",
        "minimum": 1,
        "description": "One-based heading line number in the source Org file."
      }
    },
    "additionalProperties": false
  }
}
"#;

/// Static catalog of all registered JSON Schema Draft 2020-12 contracts.
const SCHEMA_REGISTRY: &[SchemaEntry] = &[
    SchemaEntry {
        name: "fmt",
        aliases: &["fmt", "diagnostic", "diagnostics", "org-diagnostic-output"],
        id: "https://org-tools.dev/schemas/org-diagnostic-output.schema.json",
        path: "schemas/org-diagnostic-output.schema.json",
        description: "Schema for the JSON output of 'org fmt check --format json'.",
        body: include_str!("../../schemas/org-diagnostic-output.schema.json"),
    },
    SchemaEntry {
        name: "query",
        aliases: &["query", "entry", "entries", "org-query-output"],
        id: "https://org-tools.dev/schemas/org-query-output.schema.json",
        path: "schemas/org-query-output.schema.json",
        description: "Schema for the JSON output of 'org query search --format json'.",
        body: include_str!("../../schemas/org-query-output.schema.json"),
    },
    SchemaEntry {
        name: "agenda",
        aliases: &["agenda", "org-agenda-output"],
        id: "https://org-tools.dev/schemas/org-agenda-output.schema.json",
        path: "schemas/org-agenda-output.schema.json",
        description: "Schema for the JSON output of 'org query agenda --format json'.",
        body: include_str!("../../schemas/org-agenda-output.schema.json"),
    },
    SchemaEntry {
        name: "columns",
        aliases: &["columns", "column-view", "org-columns-output"],
        id: "https://org-tools.dev/schemas/org-columns-output.schema.json",
        path: "schemas/org-columns-output.schema.json",
        description: "Schema for the JSON output of 'org query columns --format json'.",
        body: include_str!("../../schemas/org-columns-output.schema.json"),
    },
    SchemaEntry {
        name: "show",
        aliases: &["show", "org-show-output"],
        id: "https://org-tools.dev/schemas/org-show-output.schema.json",
        path: "schemas/org-show-output.schema.json",
        description: "Schema for the JSON output of 'org show --format json'.",
        body: include_str!("../../schemas/org-show-output.schema.json"),
    },
    SchemaEntry {
        name: "update",
        aliases: &["update", "mutation", "archive", "org-mutation-output"],
        id: "https://org-tools.dev/schemas/org-mutation-output.schema.json",
        path: "schemas/org-mutation-output.schema.json",
        description: "Schema for the JSON output of 'org update' and 'org archive' commands with '--format json'.",
        body: include_str!("../../schemas/org-mutation-output.schema.json"),
    },
    SchemaEntry {
        name: "clock-status",
        aliases: &["clock-status", "clock_status", "org-clock-status-output"],
        id: "https://org-tools.dev/schemas/org-clock-status-output.schema.json",
        path: "schemas/org-clock-status-output.schema.json",
        description: "Schema for the JSON output of 'org clock status --format json'.",
        body: include_str!("../../schemas/org-clock-status-output.schema.json"),
    },
    SchemaEntry {
        name: "clock-report",
        aliases: &["clock-report", "clock_report", "org-clock-report-output"],
        id: "https://org-tools.dev/schemas/org-clock-report-output.schema.json",
        path: "schemas/org-clock-report-output.schema.json",
        description: "Schema for the JSON output of 'org clock report --format json'.",
        body: include_str!("../../schemas/org-clock-report-output.schema.json"),
    },
    SchemaEntry {
        name: "jscalendar",
        aliases: &["jscalendar", "jscal", "org-jscalendar-output"],
        id: "https://org-tools.dev/schemas/org-jscalendar-output.schema.json",
        path: "schemas/org-jscalendar-output.schema.json",
        description: "Schema for RFC 8984 JSCalendar Event and Task objects emitted by 'org export jscalendar'.",
        body: JSCALENDAR_SCHEMA,
    },
    SchemaEntry {
        name: "roam",
        aliases: &["roam", "org-roam-output"],
        id: "https://org-tools.dev/schemas/org-roam-output.schema.json",
        path: "schemas/org-roam-output.schema.json",
        description: "Schema for the JSON output of 'org roam node|backlinks|links|graph --format json'.",
        body: include_str!("../../schemas/org-roam-output.schema.json"),
    },
    SchemaEntry {
        name: "fleeting",
        aliases: &["fleeting", "capture", "org-fleeting-output"],
        id: "https://org-tools.dev/schemas/org-fleeting-output.schema.json",
        path: "schemas/org-fleeting-output.schema.json",
        description: "Schema for the JSON output of 'org fleeting list' and fleeting note capture commands.",
        body: include_str!("../../schemas/org-fleeting-output.schema.json"),
    },
    SchemaEntry {
        name: "blocked",
        aliases: &["blocked", "org-blocked-output"],
        id: "https://org-tools.dev/schemas/org-blocked-output.schema.json",
        path: "schemas/org-blocked-output.schema.json",
        description: "Schema for the JSON output of 'org query blocked --format json'.",
        body: include_str!("../../schemas/org-blocked-output.schema.json"),
    },
    SchemaEntry {
        name: "stuck",
        aliases: &["stuck", "org-stuck-output"],
        id: "https://org-tools.dev/schemas/org-stuck-output.schema.json",
        path: "schemas/org-stuck-output.schema.json",
        description: "Schema for the JSON output of 'org query stuck --format json'.",
        body: include_str!("../../schemas/org-stuck-output.schema.json"),
    },
    SchemaEntry {
        name: "deps",
        aliases: &["deps", "dependencies", "org-deps-output"],
        id: "https://org-tools.dev/schemas/org-deps-output.schema.json",
        path: "schemas/org-deps-output.schema.json",
        description: "Schema for the JSON output of 'org query deps --format json'.",
        body: include_str!("../../schemas/org-deps-output.schema.json"),
    },
];

// endregion

// region Lookup & Output Helpers

/// Resolves a schema name or alias to its embedded JSON Schema text.
fn lookup_schema(name: &str) -> Option<&'static str> {
    SCHEMA_REGISTRY
        .iter()
        .find(|entry| entry.aliases.contains(&name))
        .map(|entry| entry.body)
}

/// Renders the registered schema catalog as either human-readable text or JSON.
fn render_schema_list(format: SchemaListFormat) -> String {
    match format {
        SchemaListFormat::Human => {
            let mut out = String::new();
            for entry in SCHEMA_REGISTRY {
                out.push_str(&format!(
                    "{:<14} {:<44} {}\n",
                    entry.name, entry.path, entry.description
                ));
            }
            out
        }
        SchemaListFormat::Json => {
            let rows: Vec<SchemaSummary<'_>> = SCHEMA_REGISTRY
                .iter()
                .map(|entry| SchemaSummary {
                    name: entry.name,
                    aliases: entry.aliases,
                    id: entry.id,
                    path: entry.path,
                    description: entry.description,
                })
                .collect();
            let mut json = serde_json::to_string_pretty(&rows).unwrap_or_default();
            json.push('\n');
            json
        }
    }
}

/// Writes `content` to `output_path` (creating parent directories as needed) or prints to stdout.
fn emit_output(content: &str, output_path: Option<&Path>) -> i32 {
    if let Some(path) = output_path {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            if let Err(err) = fs::create_dir_all(parent) {
                eprintln!("org: error creating directory {}: {err}", parent.display());
                return EXIT_ERROR;
            }
        }
        if let Err(err) = fs::write(path, content) {
            eprintln!("org: error writing {}: {err}", path.display());
            return EXIT_ERROR;
        }
        return EXIT_OK;
    }
    print!("{content}");
    if !content.ends_with('\n') {
        println!();
    }
    EXIT_OK
}

// endregion

// region Command Entrypoint

/// Executes `org schema` (`--list` or `<name>`, with optional `--output <path>`).
pub(crate) fn run_schema(args: &SchemaCommandArgs) -> i32 {
    if args.list {
        let catalog = render_schema_list(args.format);
        return emit_output(&catalog, args.output.as_deref());
    }

    let name = args.schema_name.as_deref().unwrap_or_default();
    let Some(schema) = lookup_schema(name) else {
        eprintln!("org: unknown schema: {name}");
        let names: Vec<&str> = SCHEMA_REGISTRY.iter().map(|e| e.name).collect();
        eprintln!("Available: {}", names.join(", "));
        return EXIT_ERROR;
    };
    emit_output(schema, args.output.as_deref())
}

// endregion
