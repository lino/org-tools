<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# CLI & Rule Reference

## Overview

**What you will learn:**

- Every top-level `org` subcommand, global option, and exit code (`0..3`).
- Where to configure `.org-tools.toml` settings, capture templates, and environment variables.
- Every auto-fixable format rule (`F001..F009`) and lint diagnostic rule (`E001..E010`, `W001..W039`, `I001..I008`).

This page serves as the single-page reference for `org` flags, exit codes, and formatting or lint rule identifiers.
For `.org-tools.toml` settings and environment variables, see the [Configuration Reference](configuration-reference.md) and [Configuration Guide](configuration.md).
For offline terminal reference, run `man 1 org` and `man 5 org-tools.toml`.

## Top-Level Subcommands & Global Options

| Command / Option                   | Purpose                                                                                                                                                                      |
| ---------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `org --version [--format json]`    | Print application version string or structured JSON version and git commit metadata.                                                                                         |
| `org fmt check [PATHS...]`         | Lint `.org` files and report diagnostics (`--id-scope <vault\|roam-dir\|dir\|file>`, `--format json`).                                                                       |
| `org fmt format [PATHS...]`        | Auto-format files in place, check differences (`--check`), or write to standard output (`--stdout`).                                                                         |
| `org next [PATHS...]`              | Navigate actionable GTD next tasks with `--roam-dir`, `--project`, `--context`, `--max-effort`, and `--pick`.                                                                |
| `org query search [QUERY]`         | Search entries with `--fts`, `--include`, `--exclude-properties`, `--scope`, `--for-project`, `--for-landscape`, `--forge-path`, `--strict-scope`, and `--scope-config`.     |
| `org query agenda`                 | Project multi-day timelines with `org-habit` graphs (`--from`, `--days`, `--warning-days`, `--composite`).                                                                   |
| `org query inbox\|next\|waiting`   | Inspect unclarified inbox items, unblocked next actions, and delegated tasks across vaults.                                                                                  |
| `org query blocked\|stuck\|deps`   | Inspect `org-edna` blocked entries, stuck project outlines, and dependency relationship trees.                                                                               |
| `org fleeting capture [TEXT]`      | Capture a Luhmann fleeting thought with `--medium`, `--location`, and target `--roam-dir`.                                                                                   |
| `org fleeting list`                | List pending fleeting notes filtered by `--status <unrefined\|reformed\|converted-to-task\|discarded\|all>`.                                                                 |
| `org fleeting to-zettel <ID>`      | Reform a fleeting note into a permanent Zettelkasten concept note (`--title`, `--parent`, `--link`).                                                                         |
| `org fleeting to-task <ID>`        | Convert a fleeting note into an actionable GTD inbox task with `--context` and `--effort`.                                                                                   |
| `org fleeting discard <ID>`        | Discard or retire an unneeded fleeting note from the active queue.                                                                                                           |
| `org fleeting process`             | Interactively step through unrefined fleeting notes inside your terminal.                                                                                                    |
| `org roam node [TITLE]`            | Resolve an `org-roam` v2 node by UUID, heading title, locator, or alias (`--scope`, `--for-project`, `--for-landscape`, `--forge-path`, `--strict-scope`, `--scope-config`). |
| `org roam backlinks [TITLE]`       | List incoming `[[id:...]]` backlinks pointing to a target node across vaults.                                                                                                |
| `org roam links [TITLE]`           | List outgoing `[[id:...]]` links declared inside a source node.                                                                                                              |
| `org roam graph`                   | Export the workspace knowledge network as `mermaid`, `dot`, or schema-validated `json`.                                                                                      |
| `org roam capture [TITLE]`         | Capture a note using built-in or configured templates (`--template`, `--type`, `--list-templates`, `--edit`).                                                                |
| `org roam gtd inbox [TITLE]`       | Append a new `* TODO` task into `<roam_dir>/<gtd_subdir>/inbox.org` (`--context`, `--effort`).                                                                               |
| `org roam gtd project [TITLE]`     | Scaffold a new GTD project outline node at `<roam_dir>/<gtd_subdir>/projects/<slug>.org`.                                                                                    |
| `org roam sync-agenda-tags`        | Synchronize `:agenda:` filetags across `org-roam` files containing active TODO entries.                                                                                      |
| `org show [LOCATOR]`               | Extract a bounded subtree or file node slice with `--include`, `--exclude-properties`, and `--max-chars`.                                                                    |
| `org update add-id [TARGETS]`      | Mint `:ID:` properties (`--file`, `--headings`, `--max-level`, `--locator`, `--uuidv5-seed`, `--write-seed`).                                                                |
| `org update link --to <TO>`        | Insert an idempotent outbound link to another entry or file node (`--section`, `--label`).                                                                                   |
| `org update set-state <STATE>`     | Transition TODO keyword state with `:LOGBOOK:` logging, repeaters, and `:TRIGGER:` cascades.                                                                                 |
| `org update set-scheduled <D>`     | Set, shift (`+3d`), or clear (`--clear`) `SCHEDULED:` planning dates.                                                                                                        |
| `org update set-deadline <D>`      | Set, shift, or clear `DEADLINE:` planning dates.                                                                                                                             |
| `org update add-todo <TITLE>`      | Insert a new TODO entry with planning dates, tags, and properties.                                                                                                           |
| `org update set-property <K> <V>`  | Insert or update a key-value property inside an entry `:PROPERTIES:` drawer.                                                                                                 |
| `org update delete-property <K>`   | Remove a property key from an entry `:PROPERTIES:` drawer.                                                                                                                   |
| `org update set-tags <TAGS>`       | Set or replace heading tags aligned to column 77.                                                                                                                            |
| `org update dep <add\|remove>`     | Wire or disconnect symmetric `org-edna` `:BLOCKER:` and `:TRIGGER:` properties.                                                                                              |
| `org update add-cookie`            | Recalculate heading statistic cookies (`[/]` and `[%]`).                                                                                                                     |
| `org update calc`                  | Evaluate `#+TBLFM:` formulas across Org spreadsheet tables.                                                                                                                  |
| `org refile <SRC> --to <DST>`      | Move or copy an entry subtree across files and headings while preserving hierarchy.                                                                                          |
| `org clock in\|out\|cancel`        | Manage active `:LOGBOOK:` clocks with running timers and effort estimates.                                                                                                   |
| `org clock report`                 | Generate clocked time summaries grouped by entry, tag, or day with effort variance.                                                                                          |
| `org clock status`                 | Inspect currently running clocks and active durations.                                                                                                                       |
| `org archive [PATHS...]`           | Move completed `DONE` subtrees to archive companion files with archive metadata.                                                                                             |
| `org export ical\|jscal\|md`       | Export entries to iCalendar (`.ics`), JSCalendar (`.json`), or Zensical GitHub Flavored Markdown.                                                                            |
| `org cache status\|rebuild\|clear` | Manage the local incremental SQLite cache index at `.cache/org-cache.db`.                                                                                                    |
| `org mcp [--workspace <DIR>]`      | Serve the built-in zero-Emacs Model Context Protocol JSON-RPC 2.0 server over `stdio`.                                                                                       |
| `org lsp [--workspace <DIR>]`      | Serve the built-in zero-Emacs Language Server Protocol server over `stdio`.                                                                                                  |
| `org schema [--list\|<NAME>]`      | Print embedded JSON Schema Draft 2020-12 definitions (`fleeting`, `agenda`, `query`, etc.).                                                                                  |
| `--config <PATH>`                  | Load configuration from an explicit path instead of searching parent directories.                                                                                            |
| `--no-cache`                       | Bypass `.cache/org-cache.db` and parse `.org` files directly from disk.                                                                                                      |
| `--completions <SHELL>`            | Generate dynamic shell completions (`bash`, `zsh`, `fish`) wired to `org complete`.                                                                                          |

??? info "Deep Dive: The 16 Built-In MCP Tools"

    The `org mcp` server exposes 16 tools covering the complete Org lifecycle.
    Read tools include `org_roam_node`, `org_roam_backlinks`, `org_roam_search`, `org_query_next`, `org_query_blocked`, `org_query_agenda`, `org_show`, and `org_fleeting_list`.
    Write tools include `org_roam_capture`, `org_fleeting_transform`, `org_update_state`, `org_update_planning`, `org_clock_in`, `org_clock_out`, `org_refile`, and `org_fmt_file`.
    All 16 tools respect workspace boundaries and return telemetry metadata without corrupting drawer structures.

## Exit Codes (`0..3`)

| Code | Constant              | Meaning                                                                              |
| ---- | --------------------- | ------------------------------------------------------------------------------------ |
| `0`  | `EXIT_OK`             | Operation completed cleanly with zero lint or format diagnostics.                    |
| `1`  | `EXIT_ISSUES`         | Query matched zero entries, or `org fmt` reported diagnostics or formatting changes. |
| `2`  | `EXIT_ERROR`          | Operational failure such as missing files, invalid arguments, or parse errors.       |
| `3`  | `EXIT_REQUIRES_EMACS` | Table formula (`#+TBLFM:`) or advanced feature requires an Emacs Lisp runtime.       |

## Configuration & Environment Reference

All `.org-tools.toml` tables (`[format]`, `[lint]`, `[lint.zettel]`, `[lint.authoring]`, `[todo]`, `[agenda]`, `[roam]`, `[capture.templates.<name>]`, `[landscapes.<slug>]`) and environment variables now live on dedicated pages:

- [Workspace & Directory Structure](directory-structure.md): Opinionated multi-vault GTD, fleeting-note, and Zettelkasten directory layout.
- [Configuration Guide](configuration.md): Why to configure `org-tools` and how global, workspace, and per-vault layers merge.
- [Configuration Reference](configuration-reference.md): Exhaustive tables of every TOML key, built-in template, `${...}` placeholder, and environment variable.

??? info "Deep Dive: Multi-Vault Scoping and Landscape Architecture"

    `[roam.directories]` binds named vault slugs to distinct file paths.
    This lets commands like `org next --roam-dir work` scope searches without mixing private tasks.
    The `[lint] id_scope` setting controls cross-file `[[id:...]]` (`W037`) and `org-edna` `ids(...)` (`W036`) link resolution scope.
    `[landscapes.<slug>]` partitions large corporate monorepos into scoped engineering landscapes.

## Format Rules (`F001..F009`)

| ID     | Name                    | Description                                                           |
| ------ | ----------------------- | --------------------------------------------------------------------- |
| `F001` | `trailing-whitespace`   | Remove trailing spaces and tabs outside protected literal blocks.     |
| `F002` | `blank-lines`           | Collapse excessive consecutive blank lines when enabled.              |
| `F003` | `heading-spacing`       | Enforce blank lines before headings when enabled.                     |
| `F004` | `table-format`          | Align Org table columns and normalize separator rows.                 |
| `F005` | `property-drawer-align` | Align `:KEY:` and value columns inside `:PROPERTIES:` drawers.        |
| `F006` | `keyword-spacing`       | Normalize spacing after `#+KEYWORD:` buffer directives.               |
| `F007` | `tag-alignment`         | Align heading `:tag:` blocks to column 77 (or `tag_column`).          |
| `F008` | `list-format`           | Normalize plain-list bullet and checkbox spacing.                     |
| `F009` | `timestamp-spacing`     | Normalize internal spacing inside active and inactive Org timestamps. |

## Lint Rules (`E001..E010`, `W001..W037`, `I001..I004`)

### Structural Error Rules (`E*`)

| ID     | Name                     | Description                                                             |
| ------ | ------------------------ | ----------------------------------------------------------------------- |
| `E001` | `unclosed-block`         | `#+BEGIN_*` block without a matching `#+END_*` delimiter.               |
| `E002` | `duplicate-custom-id`    | Duplicate `:CUSTOM_ID:` property within a file.                         |
| `E003` | `duplicate-name`         | Duplicate `#+NAME:` affiliated keyword value.                           |
| `E003` | `include-validity`       | Missing, workspace-escaping, or cyclic `#+INCLUDE:` directive.          |
| `E004` | `duplicate-target`       | Duplicate `<<target>>` radio or dedicated anchor.                       |
| `E005` | `duplicate-footnote-def` | Duplicate `[fn:label]` footnote definition.                             |
| `E006` | `incomplete-drawer`      | `:DRAWER:` opened without a matching `:END:` line.                      |
| `E007` | `drawer-nesting`         | Drawer opened inside another unclosed drawer.                           |
| `E008` | `circular-edna-blocker`  | Circular `org-edna` `:BLOCKER:` dependency cycle across entries.        |
| `E009` | `duplicate-id`           | Duplicate `:ID:` across file drawers or headings in the workspace.      |
| `E010` | `invalid-id-seed`        | `:ID:` does not match the Deterministic RFC 9562 UUIDv5 of `:ID_SEED:`. |

### Correctness & Style Warning Rules (`W001..W037`)

| ID     | Name                           | Description                                                                     |
| ------ | ------------------------------ | ------------------------------------------------------------------------------- |
| `W001` | `heading-level-gap`            | Heading depth jumps by more than one level (such as `*` to `***`).              |
| `W002` | `missing-src-language`         | `#+BEGIN_SRC` block is missing a language identifier.                           |
| `W003` | `misplaced-property-drawer`    | `:PROPERTIES:` drawer is not placed immediately after heading or planning line. |
| `W004` | `orphaned-footnotes`           | Footnote reference lacks a definition or definition is unreferenced.            |
| `W005` | `invalid-keyword-syntax`       | Malformed `#+KEYWORD:` line syntax.                                             |
| `W006` | `obsolete-affiliated-keywords` | Deprecated affiliated keyword syntax.                                           |
| `W007` | `deprecated-export-blocks`     | Legacy `#+BEGIN_HTML` or `#+BEGIN_LATEX` export block syntax.                   |
| `W008` | `deprecated-category-setup`    | Deprecated `#+CATEGORY:` placement.                                             |
| `W009` | `trailing-bracket-after-link`  | Stray closing `]` immediately following an Org link.                            |
| `W010` | `invalid-effort-property`      | Malformed `:Effort:` or `:EFFORT:` duration value.                              |
| `W011` | `invalid-id-property`          | Empty or malformed `:ID:` property value.                                       |
| `W012` | `spurious-colons`              | Unpaired or malformed colons in heading tag block.                              |
| `W013` | `missing-export-backend`       | `#+BEGIN_EXPORT` block without a backend target name.                           |
| `W014` | `planning-inactive`            | Inactive `[...]` timestamp used in `SCHEDULED:` or `DEADLINE:`.                 |
| `W015` | `misplaced-planning-info`      | `SCHEDULED:`, `DEADLINE:`, or `CLOSED:` line not right after heading.           |
| `W016` | `file-application`             | Malformed `file` header argument on source block.                               |
| `W017` | `obsolete-include-markup`      | Obsolete markup token inside `#+INCLUDE:` directive.                            |
| `W018` | `quote-section`                | Deprecated `QUOTE` keyword on heading line.                                     |
| `W019` | `special-property-in-drawer`   | Reserved special property (`TODO`, `TAGS`, `SCHEDULED`) in drawer.              |
| `W020` | `misplaced-heading`            | Heading line placed inside a block or plain list.                               |
| `W021` | `orphaned-affiliated-keywords` | Affiliated keyword (`#+NAME:`, `#+CAPTION:`) at end of section.                 |
| `W022` | `non-existent-setupfile`       | `#+SETUPFILE:` references a missing local file.                                 |
| `W023` | `link-to-local-file`           | `file:` link points to a non-existent local path.                               |
| `W024` | `invalid-babel-call`           | Malformed `#+CALL:` Babel invocation syntax.                                    |
| `W025` | `percent-encoding-link`        | Unnecessary percent-encoding in local file link.                                |
| `W026` | `invalid-image-alignment`      | Invalid alignment value in `#+ATTR_*` keyword.                                  |
| `W027` | `timestamp-validity`           | Invalid calendar date or time inside an Org timestamp.                          |
| `W028` | `link-syntax`                  | Unclosed or malformed `[[...]]` link brackets.                                  |
| `W029` | `list-consistency`             | Mixed bullet styles (`-`, `+`, `*`) at the same plain-list level.               |
| `W030` | `priority-validity`            | Malformed `[#X]` priority cookie on heading.                                    |
| `W031` | `clock-entry-validity`         | Malformed `CLOCK:` timestamp range or duration sum.                             |
| `W032` | `affiliated-keyword-placement` | Misplaced affiliated keyword line.                                              |
| `W033` | `undeclared-tag`               | Heading uses a tag not listed in `#+TAGS:` when strict tags apply.              |
| `W034` | `invalid-table-formula`        | Malformed `#+TBLFM:` spreadsheet formula expression.                            |
| `W035` | `invalid-edna-syntax`          | Unbalanced or unknown `org-edna` `:BLOCKER:` or `:TRIGGER:` token.              |
| `W036` | `unresolved-edna-ids`          | `org-edna` `ids(...)` finder references a missing `:ID:` UUID.                  |
| `W037` | `broken-id-link`               | `[[id:...]]` link targets an `:ID:` that does not exist in vault.               |

### Informational Rules (`I001..I004`)

| ID     | Name                   | Description                                                      |
| ------ | ---------------------- | ---------------------------------------------------------------- |
| `I001` | `suspicious-language`  | Possibly misspelled source-block language name on `#+BEGIN_SRC`. |
| `I002` | `unknown-options-item` | Unrecognized option key inside `#+OPTIONS:`.                     |
| `I003` | `keyword-validity`     | Unrecognized or misspelled `#+KEYWORD:` directive.               |
| `I004` | `block-type-validity`  | Unrecognized `#+BEGIN_*` special block type.                     |
