<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# JSON Schemas & Machine-Readable Contracts (`org schema`)

## Overview

**What you will learn:**

- How to list every embedded JSON Schema Draft 2020-12 contract using `org schema --list`.
- How to print a schema to `stdout` or export it to disk with `org schema <name> --output <path>`.
- How to validate `--format json` CLI outputs against their committed schemas in automated pipelines.

Automation scripts, dashboards, and AI agents break when command-line JSON outputs drift without warning.
`org` embeds strict JSON Schema Draft 2020-12 contracts (`additionalProperties: false`) directly inside the binary.
You can inspect, export, and validate every JSON payload without cloning the source repository.

## Prerequisites

- `org` installed on your workstation or CI runner (see [Five-Minute Quickstart](quickstart.md)).
- Optional: `jq` or a JSON Schema validator (such as `check-jsonschema` or `ajv-cli`) for pipeline checks.

## Steps

1. Run `org schema --list` to view all registered JSON Schema names, aliases, file paths, and descriptions.
   ```sh
   org schema --list
   org schema --list --format json
   ```
1. Print a specific JSON Schema (`fmt`, `query`, `agenda`, `columns`, `show`, `update`, `clock-report`, `jscalendar`, `roam`, `fleeting`) to `stdout`.
   ```sh
   org schema query
   org schema agenda
   org schema columns
   org schema update
   ```
1. Write a JSON Schema directly to a file path (creating parent directories automatically) using `--output` (`-o`).
   ```sh
   org schema columns --output build/schemas/org-columns-output.schema.json
   org schema clock-report -o build/schemas/org-clock-report-output.schema.json
   ```
1. Validate live `--format json` command outputs against the exported schema in your test or CI pipeline.
   ```sh
   org query columns --format json tasks.org > build/columns.json
   jq '.[0] | keys' build/columns.json
   ```

## Expected Outcome

`org schema --list` prints the complete schema catalog and exits with code `0`.
`org schema <name> -o <path>` writes a valid JSON Schema Draft 2020-12 document with `$schema: "https://json-schema.org/draft/2020-12/schema"`:

```text
$ org schema --list
NAME          FILE                                      DESCRIPTION
diagnostic    schemas/org-fmt-output.schema.json        Lint and format diagnostics (org fmt check --format json)
query         schemas/org-query-output.schema.json      Query search, next, inbox, and waiting results
agenda        schemas/org-agenda-output.schema.json     Multi-day agenda timeline and composite GTD sections
columns       schemas/org-columns-output.schema.json    Column view rollups (org query columns --format json)
show          schemas/org-show-output.schema.json       Bounded node slice output (org show --format json)
mutation      schemas/org-mutation-output.schema.json   Uniform MutationResult envelope for write commands
```

## Power-User & Scripting Options

Export the entire schema catalog as JSON via `org schema --list --format json` and dump every schema file in one shell loop.
See [Power-User & Scripting](power-user-and-scripting.md), [CLI, Config & Rule Reference](reference.md), and the [Glossary](../glossary.md) for details:

```sh
# Export every registered schema into ./exported-schemas/ using jq
org schema --list --format json \
  | jq -r '.[] | "\(.name) \(.file)"' \
  | while read -r name file; do
      org schema "$name" --output "exported-schemas/$file"
    done
```

??? info "Deep Dive: Registered Schema Catalog"

    | Canonical Name | Accepted Aliases      | Repository Schema File                        | Matching Command Output                     |
    | -------------- | --------------------- | --------------------------------------------- | ------------------------------------------- |
    | `diagnostic`   | `fmt`, `diagnostics`  | `schemas/org-fmt-output.schema.json`          | `org fmt check --format json`               |
    | `query`        | `entry`, `entries`    | `schemas/org-query-output.schema.json`        | `org query search\|next\|inbox\|waiting`    |
    | `agenda`       | `org-agenda-output`   | `schemas/org-agenda-output.schema.json`       | `org query agenda --format json`            |
    | `columns`      | `org-columns-output`  | `schemas/org-columns-output.schema.json`      | `org query columns --format json`           |
    | `show`         | `org-show-output`     | `schemas/org-show-output.schema.json`         | `org show --format json`                    |
    | `blocked`      | `org-blocked-output`  | `schemas/org-blocked-output.schema.json`      | `org query blocked --format json`           |
    | `stuck`        | `org-stuck-output`    | `schemas/org-stuck-output.schema.json`        | `org query stuck --format json`             |
    | `deps`         | `dependencies`        | `schemas/org-deps-output.schema.json`         | `org query deps --format json`              |
    | `clock-report` | `clock_report`        | `schemas/org-clock-report-output.schema.json` | `org clock report --format json`            |
    | `clock-status` | `clock_status`        | `schemas/org-clock-status-output.schema.json` | `org clock status --format json`            |
    | `mutation`     | `update`, `archive`   | `schemas/org-mutation-output.schema.json`     | `org update *`, `org refile`, `org archive` |
    | `jscalendar`   | `jscal`               | `schemas/org-jscalendar-output.schema.json`   | `org export jscal`                          |
    | `roam`         | `org-roam-output`     | `schemas/org-roam-output.schema.json`         | `org roam node\|backlinks\|links\|graph`    |
    | `fleeting`     | `org-fleeting-output` | `schemas/org-fleeting-output.schema.json`     | `org roam fleeting\|capture --format json`  |

## Troubleshooting

| Symptom                                               | Cause                                                   | Fix                                                                                |
| ----------------------------------------------------- | ------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `unknown schema name` error with exit code `2`        | Positional name does not match a registered schema ID   | Run `org schema --list` to view all valid schema names and aliases                 |
| `missing schema name` error when running `org schema` | Neither `<NAME>` nor `--list` was supplied on the CLI   | Pass `--list` to inspect the catalog or specify a schema name such as `query`      |
| Failed to write `--output` file                       | Current user lacks write permission on target directory | Choose a writable output directory or check parent folder permissions              |
| Schema validation fails on human output               | Command was invoked without `--format json`             | Add `--format json` (or use `org export jscal`) before validating against a schema |
