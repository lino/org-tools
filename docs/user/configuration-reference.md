<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Configuration Reference

This page lists every `.org-tools.toml` table, key, accepted value, built-in capture template, interpolation variable, and environment variable in `org-tools`.
For a guided walkthrough explaining why and how to layer these settings, read the [Configuration Guide](configuration.md) and [Workspace & Directory Structure](directory-structure.md).

## Discovery, Precedence & Path Expansion

`org` resolves configuration from highest to lowest precedence:

1. **Explicit CLI Flag (`--config <PATH>`)**: Loads only `<PATH>` and skips global and ancestor file discovery.
2. **Environment Variable (`$ORG_TOOLS_CONFIG`)**: Sets the global user configuration file path when `--config` is omitted.
3. **Vault Layer (`<roam_dir>/.org-tools.toml`)**: Overrides workspace settings when targeting a specific `org-roam` vault.
4. **Workspace Ancestor Chain (`.org-tools.toml`)**: Discovered by walking upward from the target directory to the filesystem root and merged outer-to-inner.
5. **Scope Fallback (`kb.toml`)**: Merged alongside `.org-tools.toml` up to the nearest `.git` root for `[landscapes]`, `[projects]`, and `[project_groups]`.
6. **Global User Config**: `$XDG_CONFIG_HOME/org-tools/config.toml`, or `~/.config/org-tools/config.toml` when `$XDG_CONFIG_HOME` is unset.
7. **Built-In Defaults**: Standard Emacs Org-mode compatible defaults.

Path values inside `[roam]` and `[capture.templates.<name>]` expand `~`, `~/`, `~\`, `$HOME/`, and `${HOME}/` using `$HOME` (or `$USERPROFILE` on Windows).
Relative paths always resolve against the parent directory of the configuration file that declared them.

## `[format]` — Auto-Fixable Formatting Rules

Controls auto-fixable formatting rules executed by `org fmt format` and checked by `org fmt check`:

| Key                           | Type      | Default | Rule   | Description                                                                             |
| ----------------------------- | --------- | ------- | ------ | --------------------------------------------------------------------------------------- |
| `trailing_whitespace`         | `boolean` | `true`  | `F001` | Strip trailing spaces and tabs outside protected literal blocks.                        |
| `blank_lines`                 | `boolean` | `false` | `F002` | Collapse consecutive blank lines down to `max_consecutive_blank_lines`.                 |
| `max_consecutive_blank_lines` | `integer` | `1`     | `F002` | Maximum consecutive blank lines allowed when `blank_lines = true`.                      |
| `heading_blank_lines`         | `boolean` | `false` | `F003` | Require blank lines immediately before headings.                                        |
| `heading_blank_lines_before`  | `integer` | `1`     | `F003` | Number of blank lines inserted before headings when `heading_blank_lines = true`.       |
| `table_format`                | `boolean` | `true`  | `F004` | Align Org pipe table columns and normalize separator rows.                              |
| `property_drawer_align`       | `boolean` | `true`  | `F005` | Align `:KEY:` and value columns inside `:PROPERTIES:` drawers.                          |

Rules `F006` (`keyword-spacing`), `F007` (`tag-alignment`), `F008` (`block-case`), and `F009` (`planning-line`) are active during formatting.
Adjust the target right-margin column for `F007` via the `ORG_TAG_COLUMN` environment variable (default `77`).

## `[lint]`, `[lint.zettel]` & `[lint.authoring]` — Diagnostics & Hygiene

Controls diagnostic rules (`E*`, `W*`, `I*`), cross-file `:ID:` resolution scope, and opt-in Zettelkasten/authoring checks:

| Section & Key                                 | Type       | Default   | Allowed / Example Values                      | Description                                                                            |
| --------------------------------------------- | ---------- | --------- | --------------------------------------------- | -------------------------------------------------------------------------------------- |
| `[lint] disabled_rules`                       | `string[]` | `[]`      | `["W001", "heading-level-gap"]`               | Rule IDs or kebab-case rule names skipped during `org fmt check` and `org lsp`.        |
| `[lint] enable` *(alias: `enabled_rules`)*    | `string[]` | `[]`      | `["W038", "W039", "I008"]`                    | Opt-in rule IDs or names explicitly enabled for the workspace.                         |
| `[lint] id_scope`                             | `string`   | `"vault"` | `"vault"`, `"roam-dir"`, `"dir"`, `"file"`    | Boundary for duplicate `:ID:` (`E002`), `org-edna` `ids(...)` (`W036`), and `W037`.    |
| `[lint.zettel] forbid_tasks`                  | `boolean`  | `false`   | `true` or `false`                             | Enable `W038` (`zettel-task-hygiene`) forbidding TODO states and checklists in notes.  |
| `[lint.authoring] forbid_ephemeral_headers`   | `boolean`  | `false`   | `true` or `false`                             | Enable `W039` (`ephemeral-file-headers`) flagging `#+DATE:` and `#+AUTHOR:` lines.     |
| `[lint.authoring] max_table_cell_chars`       | `integer`  | *unset*   | Positive integer (for example, `120`)         | Enable `I008` (`oversized-table-cell`) when any table cell exceeds this character cap. |

??? info "Deep Dive: Choosing an `id_scope` Resolution Boundary"

    - `"vault"` *(default)*: Resolves `[[id:...]]` links and duplicate IDs across the entire workspace and all configured `[roam.directories]`.
    - `"roam-dir"`: Confines link resolution strictly to the owning `org-roam` vault directory so vaults stay self-contained.
    - `"dir"`: Confines resolution to `.org` files inside the immediate target directory.
    - `"file"`: Validates `:ID:` links and duplicates strictly within each individual file.

## `[todo]` — Fallback Workflow Sequences

Defines workspace-level TODO keywords used whenever an `.org` file omits `#+TODO:`, `#+SEQ_TODO:`, `#+TYP_TODO:`, or `#+SETUPFILE:` directives:

| Key         | Type       | Default | Example                                              | Description                                                                        |
| ----------- | ---------- | ------- | ---------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `sequences` | `string[]` | `[]`    | `["TODO(t) NEXT(n) WAITING(w!) \| DONE(d) CANC(c)"]` | Additive Org sequence strings (`\|` separates active states from done states).     |
| `keywords`  | `string[]` | `[]`    | `["TODO", "NEXT", "\|", "DONE"]`                     | Token list joined and parsed as a single sequence when `sequences` is empty.       |
| `todo`      | `string[]` | `[]`    | `["TODO", "NEXT", "WAITING", "BLOCKED"]`             | Explicit active ("not done") keywords used when `sequences` and `keywords` are empty. |
| `done`      | `string[]` | `[]`    | `["DONE", "CANCELLED"]`                              | Explicit terminal ("done") keywords paired with `todo`.                            |

When `[todo]` is unconfigured and a file contains no `#+TODO:` header, `org-tools` uses built-in defaults (`TODO`, `NEXT`, `WAITING`, `WAIT`, `BLOCKED` | `DONE`, `CANCELLED`, `CANCELED`).
Fast-access and logging suffixes in parentheses—such as `WAITING(w@/!)`—are stripped automatically when matching heading keywords and honored during state transitions.

## `[agenda]` — Timeline Projection

Controls default warning windows in `org query agenda`:

| Key                     | Type      | Default | Description                                                                                 |
| ----------------------- | --------- | ------- | ------------------------------------------------------------------------------------------- |
| `deadline_warning_days` | `integer` | `14`    | Lead-time warning window in days for `DEADLINE:` timestamps without an explicit `-Nd` flag. |

## `[roam]` & `[roam.directories]` — Multi-Vault Knowledge Graph

Configures `org-roam` v2 directories, default subdirectories, interactive capture editor, and daily audit logging:

| Key / Table          | Type                     | Default      | Description                                                                                 |
| -------------------- | ------------------------ | ------------ | ------------------------------------------------------------------------------------------- |
| `[roam.directories]` | `table` or `string[]`    | `{}`         | Named vault map (`work = "./work"`) or path array (`directories = ["./work", "./priv"]`).   |
| `default_directory`  | `string`                 | *unset*      | Default vault name or path used when `--roam-dir` is omitted.                               |
| `gtd_subdir`         | `string`                 | `"gtd"`      | Vault-relative subdirectory for GTD files (`inbox.org`, `gtd.org`, `projects/`, etc.).      |
| `zettel_subdir`      | `string`                 | `"zettel"`   | Vault-relative subdirectory for permanent Zettelkasten concept notes.                       |
| `fleeting_subdir`    | `string`                 | `"fleeting"` | Vault-relative subdirectory for Luhmann fleeting thought notes.                             |
| `daily_subdir`       | `string`                 | `"daily"`    | Vault-relative subdirectory for `YYYY-MM-DD.org` daily journal notes.                       |
| `editor`             | `string`                 | *unset*      | Editor command spawned for interactive captures (overrides `$VISUAL` and `$EDITOR`).        |
| `log_to_daily`       | `boolean`                | `false`      | Append a timestamped link under `* Captured today` in `daily/YYYY-MM-DD.org` on capture.    |

## `[capture.templates.<name>]` — Capture Templates

Defines custom capture templates or overrides any of the 10 built-in templates by matching `<name>`:

| Key              | Type                    | Default                       | Description                                                                              |
| ---------------- | ----------------------- | ----------------------------- | ---------------------------------------------------------------------------------------- |
| `description`    | `string`                | `"Custom capture template …"` | Summary displayed by `org roam capture --list-templates`.                                |
| `keys`           | `string`                | *unset*                       | Short selector usable via `org roam capture -t <keys>` (for example, `"a"`).             |
| `kind`           | `"file"` \| `"entry"`   | `"file"`                      | `"file"` creates a level-0 `org-roam` file node; `"entry"` appends a heading entry.      |
| `target`         | `string`                | `"<zettel_subdir>/${slug}.org"`| Vault-relative destination path supporting `${...}` placeholders.                        |
| `target_heading` | `string`                | *unset*                       | Parent heading under which `kind = "entry"` items are filed (for example, `"* Inbox"`).  |
| `roam_type`      | `string`                | *unset*                       | Sets `:TYPE:` inside the node's `:PROPERTIES:` drawer (for example, `"concept"`).        |
| `properties`     | `table<string, string>` | `{}`                          | Extra `:PROPERTIES:` pairs merged onto the template (values support `${...}` expansion). |
| `filetags`       | `string[]`              | `[]`                          | `#+filetags:` tokens (when `kind = "file"`) or heading tags (when `kind = "entry"`).     |
| `todo_header`    | `string`                | *unset*                       | Optional `#+TODO:` line written when creating a new target file.                         |
| `head`           | `string`                | *unset*                       | Additional preamble lines inserted into newly created files.                             |
| `body`           | `string`                | *unset*                       | Inline Org body scaffold supporting `${...}` variable interpolation.                     |
| `body_file`      | `string`                | *unset*                       | Path to an external `.org` body scaffold file (used when `body` is omitted).             |
| `edit`           | `boolean`               | *unset*                       | Default interactive editor launch behavior when neither `--edit` nor `--no-edit` is set. |

### Built-In Capture Template Catalog

| Template Name | Key | Kind    | Default Target                        | `:TYPE:`    | Default Tags          |
| ------------- | --- | ------- | ------------------------------------- | ----------- | --------------------- |
| `zettel`      | `z` | `file`  | `<zettel_subdir>/${slug}.org`         | `concept`   | `[]`                  |
| `concept`     | `c` | `file`  | `<zettel_subdir>/${slug}.org`         | `concept`   | `[]`                  |
| `technical`   | `t` | `file`  | `tech/${slug}.org`                    | `technical` | `[]`                  |
| `howto`       | `h` | `file`  | `howto/${slug}.org`                   | `howto`     | `[]`                  |
| `person`      | `P` | `file`  | `people/${slug}.org`                  | `person`    | `[]`                  |
| `place`       | `l` | `file`  | `places/${slug}.org`                  | `place`     | `[]`                  |
| `project`     | `p` | `file`  | `<gtd_subdir>/projects/${slug}.org`   | `project`   | `["project","agenda"]`|
| `fleeting`    | `f` | `file`  | `<fleeting_subdir>/${timestamp}-${slug}.org` | `fleeting` | `["fleeting"]` |
| `daily`       | `d` | `file`  | `<daily_subdir>/${date}.org`          | `daily`     | `["daily"]`           |
| `inbox`       | `i` | `entry` | `<gtd_subdir>/inbox.org`              | *none*      | `[]`                  |

### Template Interpolation Placeholders (`${...}`)

| Placeholder             | Example Output                           | Source                                                      |
| ----------------------- | ---------------------------------------- | ----------------------------------------------------------- |
| `${id}`                 | `550e8400-e29b-41d4-a716-446655440000`   | Minted `:ID:` UUID for the new file node or heading entry.  |
| `${title}`              | `Write-Ahead Logging`                    | Note or task title passed on the command line.              |
| `${slug}`               | `write-ahead-logging`                    | Lowercase hyphenated filename slug (or `--slug` override).  |
| `${date}`               | `2026-11-13`                             | Local ISO calendar date (`YYYY-MM-DD`).                     |
| `${year}` / `${month}`  | `2026` / `11`                            | Four-digit year and zero-padded two-digit month.            |
| `${timestamp}`          | `20261113093000`                         | Compact 14-digit local timestamp (`YYYYMMDDHHMMSS`).        |
| `${timestamp_inactive}` | `[2026-11-13 Fri 09:30]`                 | Inactive Org timestamp suitable for `:CAPTURED_AT:`.        |
| `${timestamp_active}`   | `<2026-11-13 Fri 09:30>`                 | Active Org timestamp projected onto agenda timelines.       |
| `${roam_dir}`           | `work`                                   | Active vault name or directory selector.                    |
| `${body}`               | `Initial notes...`                       | Body text supplied via `--body` or `stdin`.                 |
| `${medium}`             | `[[id:...][Designing Data-Intensive…]]`  | Resolved source medium link or citation (`--medium`).       |
| `${location}`           | `Chapter 3, p. 84`                       | Source location anchor (`--location`).                      |
| `${parent}`             | `[[id:...][Storage Engines]]`            | Resolved parent Zettel Folgezettel link (`--parent`).       |
| `${<custom_key>}`       | *user value*                             | Custom binding supplied via repeatable `--var KEY=VALUE`.   |

## `[landscapes.<slug>]` & `[projects.<slug>]` — Monorepo Scoping

Configures composable project landscapes in `.org-tools.toml` or `kb.toml` for `--scope <EXPR>` and `--strict-scope` filtering:

| Section & Key                                        | Type       | Default | Description                                                                   |
| ---------------------------------------------------- | ---------- | ------- | ----------------------------------------------------------------------------- |
| `[landscapes.<slug>] description`                    | `string`   | *unset* | Human-readable summary of the engineering landscape.                          |
| `[landscapes.<slug>] projects`                       | `string[]` | `[]`    | Member project slugs belonging directly to `<slug>`.                          |
| `[landscapes.<slug>] forges` *(alias `forge_prefixes`)* | `string[]` | `[]` | Repository forge URL or path globs (for example, `["github.com/acme/pay-*"]`).|
| `[landscapes.<slug>] includes`                       | `string[]` | `[]`    | Child landscape slugs transitively expanded with cycle protection.            |
| `[projects.<slug>] description`                      | `string`   | *unset* | Human-readable summary of an individual project.                              |
| `[projects.<slug>] forge`                            | `string`   | *unset* | Canonical forge path for the project (for example, `"github.com/acme/api"`).  |
| `[project_groups] <slug>`                            | `string[]` | `[]`    | Legacy shorthand mapping `<slug> = ["proj-a", "proj-b"]` into `landscapes`.   |

## Environment Variables

| Variable                | Default / Fallback                  | Effect                                                                                      |
| ----------------------- | ----------------------------------- | ------------------------------------------------------------------------------------------- |
| `ORG_TOOLS_CONFIG`      | Ancestor / XDG discovery            | Explicit path to global user `.org-tools.toml` configuration file.                          |
| `ORG_TOOLS_WORKSPACE`   | Project root or `cwd`               | Default workspace root for `id:` resolution, `org cache`, `org roam`, `org mcp`, `org lsp`. |
| `ORG_ROAM_DIRECTORIES`  | `[roam.directories]`                | Comma- or colon-separated vault overrides (`work=/path/a,private=/path/b`).                 |
| `ORG_TOOLS_CACHE_PATH`  | `<root>/.cache/org-cache.db`        | Explicit file path override for the SQLite WAL + FTS5 workspace cache.                      |
| `ORG_ID_NAMESPACE`      | `6ba7b811-9dad-11d1-80b4-00c04fd430c8` | Root RFC 9562 UUID namespace for `org update add-id --deterministic` (`UUIDv5`).         |
| `ORG_ID_PREFIX`         | `urn:org`                           | URN prefix prepended to `:ID_SEED:` strings during deterministic ID minting.                |
| `ORG_TAG_COLUMN`        | `77`                                | Right-margin display column used by `F007` (`tag-alignment`) and tag mutators.              |
| `ORG_TOOLS_NOW`         | System local clock                  | Fixed timestamp (`YYYY-MM-DDTHH:MM` or `YYYY-MM-DD HH:MM`) for reproducible captures/clocks.|
| `ORG_LSP_TELEMETRY_DIR` | *disabled*                          | Directory where `org lsp` writes `orgfmt_lsp.prom` and `events.jsonl` telemetry.            |
| `VISUAL` / `EDITOR`     | `vi`                                | Fallback interactive editor command when `[roam] editor` is unset (`VISUAL` checked first). |
| `NO_COLOR` / `TERM`     | *unset* / terminal                  | Disables ANSI color escapes when `NO_COLOR` is non-empty or `TERM=dumb`.                    |
| `XDG_CONFIG_HOME`       | `~/.config`                         | Base directory searched for `org-tools/config.toml`.                                        |
| `HOME` / `USERPROFILE`  | OS user profile                     | Home directory used to expand `~/` and `$HOME/` (`USERPROFILE` fallback on Windows).        |

## Complete `.org-tools.toml` Example

```toml
[format]
trailing_whitespace = true
blank_lines = true
max_consecutive_blank_lines = 1
heading_blank_lines = true
heading_blank_lines_before = 1
table_format = true
property_drawer_align = true

[lint]
disabled_rules = []
enable = ["W038", "W039", "I008"]
id_scope = "vault"

[lint.zettel]
forbid_tasks = true

[lint.authoring]
forbid_ephemeral_headers = true
max_table_cell_chars = 120

[agenda]
deadline_warning_days = 14

[todo]
sequences = ["TODO(t) NEXT(n) WAITING(w) | DONE(d) CANCELLED(c)"]

[roam]
default_directory = "work"
gtd_subdir = "gtd"
zettel_subdir = "zettel"
fleeting_subdir = "fleeting"
daily_subdir = "daily"
editor = "nvim"
log_to_daily = true

[roam.directories]
work = "./work"
private = "./private"

[capture.templates.inbox]
description = "Quick GTD inbox task"
keys = "i"
kind = "entry"
target = "gtd/inbox.org"
target_heading = "* Inbox"
properties = { CAPTURED_AT = "${timestamp_inactive}" }
body = "${body}"

[projects.payments-api]
description = "Core payments REST service"
forge = "github.com/acme/payments-api"

[landscapes.billing-platform]
description = "Billing and settlement services"
projects = ["payments-api"]
forges = ["github.com/acme/billing-*"]
includes = ["platform-core"]
```
