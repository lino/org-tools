<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Glossary and Abbreviations

## Overview

**What you will learn:**

- What every technical acronym and Org-mode ecosystem term in `org-tools` means in plain language.
- How core Rust types (`SourceFile`, `OrgDocument`, `OrgLocator`, `WorkspaceCache`) map to user concepts.
- Which standards govern identifiers, calendars, configuration files, and machine-readable schemas.

This glossary defines the domain terms, Rust data structures, and protocol acronyms used across `org-tools`.
Entries appear in alphabetical order so you can look up unfamiliar terms quickly.

## Alphabetical Reference Table

| Term or Acronym    | Full Expansion                                                           | Plain-Language Definition                                                                                                 |
| ------------------ | ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------- |
| `AST`              | Abstract Syntax Tree                                                     | Tree representation of parsed query expressions or document syntax nodes.                                                 |
| `CLI`              | Command-Line Interface                                                   | Text-based terminal interface provided by the `org` executable.                                                           |
| `Diagnostic`       | Lint or Format Diagnostic                                                | Structured report describing a file, line, column, severity, rule ID, and message.                                        |
| `Fix`              | Byte-Span Replacement Fix                                                | Non-overlapping byte range `[start, end)` and replacement text produced by a format rule.                                 |
| `FormatRule`       | Auto-Fix Format Rule Trait                                               | Stateless rule (`F001..F009`) that emits `Fix` replacements to normalize Org formatting.                                  |
| `GTD`              | Getting Things Done                                                      | Action-management methodology organizing tasks into `inbox`, `next`, `waiting`, `blocked`, and `stuck` views.             |
| `iCalendar`        | Internet Calendaring and Scheduling Core Object Specification (RFC 5545) | Standard `.ics` calendar text format exported by `org export ical`.                                                       |
| `JSCalendar`       | JSON Representation of Calendar Data (RFC 8984)                          | Modern JSON calendar event and task format exported by `org export jscal`.                                                |
| `JSON-RPC`         | JSON Remote Procedure Call 2.0                                           | Stateless JSON request-response wire protocol used by `org mcp` over `stdio`.                                             |
| `JSONL`            | JSON Lines (Newline-Delimited JSON)                                      | Streaming log format where each line contains one complete JSON object.                                                   |
| `LintRule`         | Diagnostic Lint Rule Trait                                               | Stateless rule (`E*`, `W*`, `I*`) that inspects Org text and emits `Diagnostic` warnings or errors.                       |
| `LSP`              | Language Server Protocol                                                 | Open protocol providing editor diagnostics, formatting, completion, hover, navigation, and workspace rename over `stdio`. |
| `MCP`              | Model Context Protocol                                                   | Open protocol connecting AI assistants to local tools and workspace data over `JSON-RPC` `stdio`.                         |
| `MutationResult`   | Uniform Mutation Output Envelope                                         | Schema-validated JSON response reporting modified files, affected entries, errors, and execution `telemetry`.             |
| `OrgDocument`      | Parsed Org Document Model                                                | In-memory heading tree, file keywords, and level-0 `org-roam` v2 properties for one `.org` file.                          |
| `OrgEntry`         | Parsed Org Heading Entry                                                 | Single heading node storing level, TODO keyword, priority, tags, properties, planning, clocks, and checklist items.       |
| `OrgLocator`       | Universal Org Node Locator                                               | Canonical address (`id:UUID`, `file::#custom-id`, `file::*/Outline`, or `file::line`) targeting an entry or file node.    |
| `org-edna`         | Extensible Dependencies for Org-mode Tasks                               | Dependency syntax using `:BLOCKER:` conditions and `:TRIGGER:` actions across tasks.                                      |
| `org-habit`        | Org-mode Habit Tracking Module                                           | Recurring task workflow using `:STYLE: habit`, `.+` repeaters, and `:LAST_REPEAT:` history logs.                          |
| `org-roam`         | Org-mode Zettelkasten Knowledge Graph v2                                 | Linked-note extension using `:ID:` drawers, `:ROAM_ALIASES:`, `:ROAM_REFS:`, and backlink queries.                        |
| `Predicate`        | Query Filter Predicate                                                   | Boolean condition (`todo:TODO`, `tags:work`, `scheduled:<=today`) evaluated against each `OrgEntry`.                      |
| `Protected Region` | Literal Block Protected Region                                           | Block range (`src`, `example`, `quote`, `verse`, `export`, `center`, `comment`) skipped by format rules.                  |
| `RFC 9562`         | Universally Unique Identifiers Specification                             | Internet standard defining UUID layouts, including random UUIDv4 and deterministic SHA-1 UUIDv5.                          |
| `ROFF`             | Runoff Unix Typesetting Language                                         | Classic Unix markup format used to render `man/org.1` and `man/org-tools.toml.5` manual pages.                            |
| `Runner`           | Format and Lint Pipeline Runner                                          | Engine in `org-tools-core` that runs format rules, applies fixes, and executes lint rules.                                |
| `SourceFile`       | Line-Indexed Source Buffer                                               | In-memory UTF-8 file buffer with precomputed line-start byte offsets for `O(log n)` coordinate lookups.                   |
| `SPDX`             | Software Package Data Exchange                                           | Standard header format (`SPDX-License-Identifier: GPL-3.0-or-later`) declaring file licensing.                            |
| `SQLite FTS5`      | SQLite Full-Text Search Version 5                                        | Embedded SQLite virtual table engine providing fast tokenized text search inside `WorkspaceCache`.                        |
| `TBLFM`            | Org Table Formula Line (`#+TBLFM:`)                                      | Spreadsheet formula directive attached beneath an Org table and evaluated by `org update calc`.                           |
| `TDD`              | Test-Driven Development                                                  | Engineering workflow where contributors write a failing test (**Red**) before writing production code (**Green**).        |
| `TOML`             | Tom's Obvious Minimal Language                                           | Human-readable configuration file format used by `.org-tools.toml`, `Cargo.toml`, and `mise.toml`.                        |
| `TTY`              | Teletypewriter Interactive Terminal                                      | Interactive terminal device detected to toggle ANSI colors and human-friendly output formatting.                          |
| `UTF-16`           | 16-Bit Unicode Transformation Format                                     | Character encoding used for zero-based character offsets in Language Server Protocol range coordinates.                   |
| `UUIDv5`           | Name-Based SHA-1 UUID (Version 5)                                        | Deterministic 128-bit identifier derived by hashing a namespace UUID and a canonical `:ID_SEED:` string.                  |
| `WAL`              | Write-Ahead Logging                                                      | SQLite journaling mode enabling concurrent readers and atomic transactions in `.cache/org-cache.db`.                      |
| `WorkspaceCache`   | Incremental SQLite Workspace Cache                                       | Persistent SQLite `WAL` and `SQLite FTS5` index stored at `.cache/org-cache.db` for fast queries.                         |
| `XDG`              | Cross-Desktop Group Base Directory Specification                         | Standard environment variable conventions governing user configuration and cache paths on Unix systems.                   |
| `Zettelkasten`     | Slip-Box Linked Note-Taking Method                                       | Knowledge management system linking atomic notes via unique IDs, aliases, and bidirectional backlinks.                    |

## Detailed Term Definitions

### AST (Abstract Syntax Tree)

An Abstract Syntax Tree (AST) represents structured expressions as a hierarchy of typed nodes.
`org query search` parses filter strings into a `Predicate` AST before evaluating entries.

### CLI (Command-Line Interface)

A Command-Line Interface (CLI) lets users and scripts run commands in a text terminal.
The `org` binary provides subcommands, flags, exit codes (`0..3`), and pipeable streams.

### Diagnostic

A `Diagnostic` records a single formatting or linting issue discovered in an `.org` file.
It includes the file path, 1-based line and column numbers, severity, rule ID, and explanation.

### Fix

A `Fix` defines a half-open byte span `[start, end)` and replacement text.
`formatter.rs` sorts non-overlapping `Fix` values and applies them in one left-to-right pass.

### FormatRule

`FormatRule` is the Rust trait implemented by auto-fixable formatting rules (`F001..F009`).
Each rule inspects a `SourceFile`, skips protected regions, and returns a vector of `Fix` values.

### GTD (Getting Things Done)

Getting Things Done (GTD) is a task workflow separating capture, clarification, and next-action execution.
`org query` provides built-in `inbox`, `next`, `waiting`, `blocked`, and `stuck` GTD subcommands.

### iCalendar (RFC 5545)

`iCalendar` is the RFC 5545 standard `.ics` text format for calendar events (`VEVENT`) and tasks (`VTODO`).
Run `org export ical` to export scheduled headings and deadlines to external calendar applications.

### JSCalendar (RFC 8984)

`JSCalendar` is the RFC 8984 JSON data model for calendar events and tasks.
Run `org export jscal` to emit schema-validated JSON calendar objects for web and automation pipelines.

### JSON-RPC (JSON Remote Procedure Call 2.0)

JSON Remote Procedure Call 2.0 (`JSON-RPC`) frames method invocations and responses as JSON objects.
`org mcp` exchanges `JSON-RPC` messages over `stdio` with AI assistants and editor hosts.

### JSONL (JSON Lines)

JSON Lines (`JSONL`) stores one self-contained JSON object per line for streaming ingestion.
Operators use `JSONL` records to collect structured execution telemetry and audit logs.

### LintRule

`LintRule` is the Rust trait implemented by diagnostic rules (`E*` errors, `W*` warnings, `I*` info).
Lint rules inspect formatted text and report structural or stylistic problems without mutating files.

### LSP (Language Server Protocol)

The Language Server Protocol (`LSP`) standardizes communication between development tools and language smartness providers.
`org lsp` delivers real-time diagnostics, formatting, code actions, hover, backlinks, and workspace rename to modern editors.

### MCP (Model Context Protocol)

The Model Context Protocol (MCP) standardizes how AI assistants discover and invoke local tools.
`org mcp` runs a built-in zero-Emacs MCP server confined to your configured workspace directories.

### MutationResult

`MutationResult` is the uniform JSON output envelope emitted by `org update`, `org refile`, and `org archive`.
It reports modified file counts, per-entry change details, error lists, and execution `telemetry`.

### OrgDocument

`OrgDocument` is the parsed in-memory model of a single `.org` file in `org-tools-core`.
It stores the heading tree (`Vec<OrgEntry>`), file keywords, `#+PROPERTY:` defaults, and level-0 `:PROPERTIES:` drawers.

### OrgEntry

`OrgEntry` represents a single Org heading inside an `OrgDocument`.
It holds the heading level, TODO state, priority, title, tags, properties, planning timestamps, clocks, and checklist items.

### OrgLocator

`OrgLocator` is a universal address string that pinpoints a heading or level-0 `org-roam` file node.
It supports `id:<uuid>`, `<file>::#<custom-id>`, `<file>::*/<outline>/<path>`, and `<file>::<line>`.

### org-edna

`org-edna` is an Org-mode dependency extension using `:BLOCKER:` and `:TRIGGER:` drawer properties.
`org-tools` evaluates finders (`ids`, `self`, `next-sibling`, `parent`, `children`) and actions (`todo!`, `scheduled!`, `deadline!`, `set-property!`) in pure Rust.

### org-habit

`org-habit` tracks recurring habits using `:STYLE: habit`, `.+Nd` or `.+Nd/Md` repeaters, and `:LAST_REPEAT:` timestamps.
`org query agenda` and `org update set-state` compute habit consistency and advance repeaters automatically.

### org-roam

`org-roam` v2 is a Zettelkasten knowledge graph extension built on standard Org `:ID:` properties.
`org roam` indexes level-0 file nodes, heading nodes, `:ROAM_ALIASES:`, `:ROAM_REFS:`, and bidirectional backlinks.

### Predicate

A `Predicate` is a composable boolean filter evaluated against each `OrgEntry` during queries.
Predicates combine field matchers (`todo:`, `tags:`, `level:`, `scheduled:`, `deadline:`, `property:`) with `AND`, `OR`, and `NOT`.

### Protected Region

A `Protected Region` is a literal block (`src`, `example`, `quote`, `verse`, `export`, `center`, `comment`) delimited by `#+BEGIN_` and `#+END_`.
Format rules call `regions::is_protected` to preserve literal code and quotations without modification.

### RFC 9562

`RFC 9562` is the Internet Engineering Task Force (IETF) standard governing Universally Unique Identifiers (UUIDs).
`org-tools` implements `RFC 9562` Version 4 (random) and Version 5 (deterministic SHA-1 namespace) identifiers.

### ROFF (Runoff Unix Manpage Format)

`ROFF` is the standard Unix typesetting macro language read by the `man` command.
`org-tools` ships native `ROFF` manuals in `man/org.1` (`man 1 org`) and `man/org-tools.toml.5` (`man 5 org-tools.toml`).

### Runner

`Runner` orchestrates rule execution across `.org` files in `crates/org-tools-core/src/runner.rs`.
It runs enabled `FormatRule` passes, applies non-overlapping fixes, and runs enabled `LintRule` passes.

### SourceFile

`SourceFile` wraps an `.org` file path and its raw UTF-8 text buffer.
It precomputes line-start byte offsets so rules can convert byte spans to line and column numbers in `O(log n)` time.

### SPDX (Software Package Data Exchange)

Software Package Data Exchange (`SPDX`) provides standardized machine-readable license identifiers.
Every source file and documentation page in `org-tools` carries `SPDX-License-Identifier: GPL-3.0-or-later`.

### SQLite FTS5

`SQLite FTS5` is the Full-Text Search Version 5 extension built into SQLite.
`WorkspaceCache` uses `SQLite FTS5` virtual tables to accelerate full-text heading and body searches.

### TBLFM (Table Formula Line)

`TBLFM` refers to the `#+TBLFM:` spreadsheet formula line placed directly below an Org table.
`org update calc` evaluates column and cell formulas in pure Rust and returns exit code `3` when a formula requires Emacs Lisp.

### TDD (Test-Driven Development)

Test-Driven Development (`TDD`) is an iterative cycle: write a failing test (**Red**), make it pass (**Green**), and clean up (**Refactor**).
All `org-tools` rules, queries, and mutations require automated behavioral tests before implementation.

### TOML (Tom's Obvious Minimal Language)

Tom's Obvious Minimal Language (`TOML`) is a typed configuration format with `[section]` headers.
`org-tools` reads workspace settings from `.org-tools.toml` using `TOML` syntax.

### TTY (Teletypewriter Interactive Terminal)

A `TTY` is an interactive terminal attached to standard input or output.
`org` checks `TTY` status and the `NO_COLOR` environment variable to format terminal output cleanly.

### UTF-16 (16-Bit Unicode Transformation Format)

UTF-16 encodes Unicode code points as one or two 16-bit code units.
The Language Server Protocol specifies UTF-16 code units for document range character offsets.
`org lsp` accurately translates UTF-16 character positions to UTF-8 byte spans.

### UUIDv5 (Universally Unique Identifier Version 5)

`UUIDv5` generates a deterministic 128-bit UUID by hashing a namespace UUID and a seed string with SHA-1.
`org update add-id --deterministic` mints reproducible `:ID:` values and records the seed in `:ID_SEED:`.

### WAL (Write-Ahead Logging)

Write-Ahead Logging (`WAL`) is a transactional SQLite mode that appends changes to a write-ahead log.
`WAL` lets concurrent CLI and MCP processes read `.cache/org-cache.db` without locking conflicts.

### WorkspaceCache

`WorkspaceCache` is the incremental SQLite `WAL` and `SQLite FTS5` cache stored at `.cache/org-cache.db`.
It indexes files, headings, properties, and `org-roam` links while validating freshness via `mtime`, size, and SHA-256 digests.

### XDG (Cross-Desktop Group Base Directory Specification)

The Cross-Desktop Group (`XDG`) specification defines standard directories for user configuration and cache data.
Tools and shells use `XDG` conventions when locating user-level configuration files and completion scripts.

### Zettelkasten

A `Zettelkasten` (German for "slip box") is a note-taking method built around small, interconnected notes.
`org roam` supports `Zettelkasten` workflows through permanent notes, fleeting captures, aliases, and backlink graphs.
