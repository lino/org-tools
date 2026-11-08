<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# CLI, Configuration & Rule Reference

## Overview

**What you will learn:**

- Every top-level `org` subcommand, global flag, and exit code (`0..3`).
- Every `.org-tools.toml` configuration section (`[format]`, `[lint]`, `[todo]`, `[agenda]`, `[roam]`, `[capture.templates.<name>]`).
- Every auto-fixable format rule (`F001..F009`) and lint diagnostic rule (`E001..E010`, `W001..W037`, `I001..I004`).

This page serves as the single-page reference for `org` flags, configuration keys, and rule identifiers.
For offline terminal reference, consult the native Unix `roff` (`ROFF`) manual pages via `man 1 org` and `man 5 org-tools.toml`.
See the [Glossary](../glossary.md) for definitions of all domain terms and acronyms.

## Top-Level Subcommands & Global Options

| Command / Option                 | Purpose                                                                                                      |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `org fmt check\|format`          | Lint `.org` files (`check`) or auto-format files in place (`format`, `--check`, `--stdout`).                 |
| `org query search\|agenda\|...`  | Search headings, project agendas, evaluate `columns`, render `deps`, or view `inbox`/`next`/`waiting`.       |
| `org roam node\|backlinks\|...`  | Query `org-roam` v2 nodes, backlinks, outgoing links, graphs, `capture` templates, and `fleeting` notes.     |
| `org show <LOCATOR>`             | Extract a bounded heading or file-node slice as `org`, `md`, or `json` (`--expand-includes`).                |
| `org update <SUBCOMMAND>`        | Mutate `add-id`, `set-state`, `set-scheduled`, `set-deadline`, `add-todo`, `check-item`, `dep`, or `calc`.   |
| `org refile <SRC> --to <DST>`    | Move or copy a heading subtree across files or parent headings and recalculate statistic cookies.            |
| `org clock in\|out\|cancel\|...` | Manage active `:LOGBOOK:` clocks, check running `status`, and generate `report` tables with `:EFFORT:`.      |
| `org archive [PATHS...]`         | Move completed `DONE` subtrees to `.org_archive` files with `:ARCHIVE_*:` metadata properties.               |
| `org export ical\|jscal\|md`     | Export scheduled/deadline entries to iCalendar (`.ics`), JSCalendar (`.json`), or Zensical Markdown.         |
| `org cache status\|rebuild\|...` | Inspect, rebuild, or clear the incremental SQLite Write-Ahead Logging (`WAL`) cache (`.cache/org-cache.db`). |
| `org mcp [--workspace <DIR>]`    | Run the built-in zero-Emacs JSON Remote Procedure Call 2.0 (`JSON-RPC`) Model Context Protocol server.       |
| `org schema [--list\|<NAME>]`    | List or print embedded JSON Schema Draft 2020-12 contracts (`--output <PATH>`).                              |
| `--config <PATH>`                | Load `.org-tools.toml` from an explicit path instead of searching ancestor directories.                      |
| `--no-cache`                     | Bypass `.cache/org-cache.db` and parse `.org` files directly from disk.                                      |
| `--completions <SHELL>`          | Generate shell completion scripts for `bash`, `zsh`, `fish`, `elvish`, or `powershell`.                      |

## Exit Codes (`0..3`)

| Code | Constant              | Meaning                                                                             |
| ---- | --------------------- | ----------------------------------------------------------------------------------- |
| `0`  | `EXIT_OK`             | Operation completed cleanly with zero lint/format issues.                           |
| `1`  | `EXIT_ISSUES`         | Query matched zero entries, or `org fmt` reported diagnostics / formatting changes. |
| `2`  | `EXIT_ERROR`          | Operational failure (missing file, invalid arguments, parse or I/O error).          |
| `3`  | `EXIT_REQUIRES_EMACS` | Table formula (`#+TBLFM:`) or feature requires Emacs Lisp or full Emacs Calc.       |

## `.org-tools.toml` Configuration Keys

```toml
[format]
trailing_whitespace = true       # F001: strip trailing spaces (default: true)
blank_lines = false              # F002: collapse consecutive blank lines (default: false)
max_consecutive_blank_lines = 1  # F002: maximum consecutive blank lines
heading_blank_lines = false      # F003: require blank line before headings (default: false)
heading_blank_lines_before = 1   # F003: blank lines required before headings
table_format = true              # F004: align Org pipe tables (default: true)
property_drawer_align = true     # F005: align property drawer values (default: true)
tag_column = 77                  # F007: target column for heading tags (default: 77)

[lint]
disabled_rules = ["W001"]        # Disable rules by ID or name

[todo]
active_keywords = ["TODO", "NEXT", "WAITING"]
done_keywords = ["DONE", "CANCELLED"]

[agenda]
deadline_warning_days = 14       # Default lead-time warning window in days

[roam]
roam_dir = "roam"                # Relative or absolute org-roam v2 directory
fleeting_file = "inbox.org"      # Default capture file for fleeting notes
daily_dir = "daily"              # Subdirectory for daily notes

[capture.templates.default]
target = "inbox.org"
heading = "Inbox"
todo = "TODO"
tags = ["inbox"]
```

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

| ID     | Name                           | Description                                                          |
| ------ | ------------------------------ | -------------------------------------------------------------------- |
| `W001` | `heading-level-gap`            | Heading depth jumps by more than one level (such as `*` to `***`).   |
| `W002` | `missing-src-language`         | `#+BEGIN_SRC` block is missing a language identifier.                |
| `W003` | `misplaced-property-drawer`    | `:PROPERTIES:` drawer is not placed immediately after heading/plan.  |
| `W004` | `orphaned-footnotes`           | Footnote reference lacks a definition or definition is unreferenced. |
| `W005` | `invalid-keyword-syntax`       | Malformed `#+KEYWORD:` line syntax.                                  |
| `W006` | `obsolete-affiliated-keywords` | Deprecated affiliated keyword syntax.                                |
| `W007` | `deprecated-export-blocks`     | Legacy `#+BEGIN_HTML` / `#+BEGIN_LATEX` export block syntax.         |
| `W008` | `deprecated-category-setup`    | Deprecated `#+CATEGORY:` placement.                                  |
| `W009` | `trailing-bracket-after-link`  | Stray closing `]` immediately following an Org link.                 |
| `W010` | `invalid-effort-property`      | Malformed `:Effort:` / `:EFFORT:` duration value.                    |
| `W011` | `invalid-id-property`          | Empty or malformed `:ID:` property value.                            |
| `W012` | `spurious-colons`              | Unpaired or malformed colons in heading tag block.                   |
| `W013` | `missing-export-backend`       | `#+BEGIN_EXPORT` block without a backend target name.                |
| `W014` | `planning-inactive`            | Inactive `[...]` timestamp used in `SCHEDULED:` or `DEADLINE:`.      |
| `W015` | `misplaced-planning-info`      | `SCHEDULED:` / `DEADLINE:` / `CLOSED:` line not right after heading. |
| `W016` | `file-application`             | Malformed `file` header argument on source block.                    |
| `W017` | `obsolete-include-markup`      | Obsolete markup token inside `#+INCLUDE:` directive.                 |
| `W018` | `quote-section`                | Deprecated `QUOTE` keyword on heading line.                          |
| `W019` | `special-property-in-drawer`   | Reserved special property (`TODO`, `TAGS`, `SCHEDULED`) in drawer.   |
| `W020` | `misplaced-heading`            | Heading line placed inside a block or plain list.                    |
| `W021` | `orphaned-affiliated-keywords` | Affiliated keyword (`#+NAME:`, `#+CAPTION:`) at end of section.      |
| `W022` | `non-existent-setupfile`       | `#+SETUPFILE:` references a missing local file.                      |
| `W023` | `link-to-local-file`           | `file:` link points to a non-existent local path.                    |
| `W024` | `invalid-babel-call`           | Malformed `#+CALL:` Babel invocation syntax.                         |
| `W025` | `percent-encoding-link`        | Unnecessary percent-encoding in local file link.                     |
| `W026` | `invalid-image-alignment`      | Invalid alignment value in `#+ATTR_*` keyword.                       |
| `W027` | `timestamp-validity`           | Invalid calendar date or time inside an Org timestamp.               |
| `W028` | `link-syntax`                  | Unclosed or malformed `[[...]]` link brackets.                       |
| `W029` | `list-consistency`             | Mixed bullet styles (`-`, `+`, `*`) at the same plain-list level.    |
| `W030` | `priority-validity`            | Malformed `[#X]` priority cookie on heading.                         |
| `W031` | `clock-entry-validity`         | Malformed `CLOCK:` timestamp range or duration sum.                  |
| `W032` | `affiliated-keyword-placement` | Misplaced affiliated keyword line.                                   |
| `W033` | `undeclared-tag`               | Heading uses a tag not listed in `#+TAGS:` when strict tags apply.   |
| `W034` | `invalid-table-formula`        | Malformed `#+TBLFM:` spreadsheet formula expression.                 |
| `W035` | `invalid-edna-syntax`          | Unbalanced or unknown `org-edna` `:BLOCKER:` / `:TRIGGER:` token.    |
| `W036` | `unresolved-edna-ids`          | `org-edna` `ids(...)` finder references a missing `:ID:` UUID.       |
| `W037` | `broken-id-link`               | `[[id:...]]` link targets an `:ID:` that does not exist in vault.    |

### Informational Rules (`I001..I004`)

| ID     | Name                   | Description                                                      |
| ------ | ---------------------- | ---------------------------------------------------------------- |
| `I001` | `suspicious-language`  | Possibly misspelled source-block language name on `#+BEGIN_SRC`. |
| `I002` | `unknown-options-item` | Unrecognized option key inside `#+OPTIONS:`.                     |
| `I003` | `keyword-validity`     | Unrecognized or misspelled `#+KEYWORD:` directive.               |
| `I004` | `block-type-validity`  | Unrecognized `#+BEGIN_*` special block type.                     |
