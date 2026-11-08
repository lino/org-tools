<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Architecture Decisions (ADR Catalog)

## Overview

**What you will learn:**

- Why `org-tools` uses a pure-Rust line-based engine instead of requiring an Emacs runtime.
- How `ADR-0001` through `ADR-0010` govern formatting, deterministic UUIDv5 identity, caching, and Model Context Protocol (MCP) tools.
- What trade-offs each architectural decision makes to keep `org` fast, safe, and composable.

Architecture Decision Records (ADRs) capture the context, choice, and consequences behind core technical trade-offs.
Reviewing these records helps contributors understand why `org-tools` works the way it does before proposing changes.
See [Architecture (`arc42`)](architecture.md) for the structural view and [Glossary](../glossary.md) for domain definitions.

## Decision Catalog Summary

| ID         | Status   | Title                                              | Core Outcome                                                        |
| ---------- | -------- | -------------------------------------------------- | ------------------------------------------------------------------- |
| `ADR-0001` | Accepted | Pure-Rust Line-Based Engine Without Emacs          | Scan lines directly in Rust with zero Emacs dependency.             |
| `ADR-0002` | Accepted | Two-Phase Pipeline & Single-Pass `Fix` Spans       | Run format rules first, apply sorted byte spans, then lint.         |
| `ADR-0003` | Accepted | `GPL-3.0-or-later` & Protected Literal Regions     | Honor Org-mode licensing and skip literal `#+BEGIN_*` blocks.       |
| `ADR-0004` | Accepted | Two-Crate Cargo Workspace & Umbrella `org` Binary  | Split `org-tools-core` library from the unified `org` CLI.          |
| `ADR-0005` | Accepted | Deterministic RFC 9562 UUIDv5 IDs & `:ID_SEED:`    | Mint reproducible `:ID:` values from canonical seed strings.        |
| `ADR-0006` | Accepted | `org-roam` v2 File-Level Node & Backlink Graph     | Index level-0 `:PROPERTIES:` drawers, aliases, refs, and backlinks. |
| `ADR-0007` | Accepted | Incremental SQLite WAL & FTS5 `WorkspaceCache`     | Cache workspace graphs and search tokens in `.cache/org-cache.db`.  |
| `ADR-0008` | Accepted | Deterministic `org-edna` & Pure-Rust `#+TBLFM:`    | Evaluate task triggers and table formulas without Emacs Lisp.       |
| `ADR-0009` | Accepted | Built-In `stdio` JSON-RPC 2.0 MCP Server           | Expose confined read/write Org tools directly via `org mcp`.        |
| `ADR-0010` | Accepted | Multi-Vault Workflows & Three-Manual Zensical Docs | Support GTD, Zettelkasten, and fleeting notes with native manpages. |

## ADR-0001: Pure-Rust Line-Based Engine Without Emacs

### Context

`org-tools` must lint, format, query, and mutate Org-mode files in CI pipelines and lightweight containers.
No mature, standalone Rust Abstract Syntax Tree (AST) parser covers the full Emacs `org-element.el` specification.
Requiring a headless Emacs batch process adds hundred-millisecond startup latency and external runtime dependencies.

### Decision

Implement a pure-Rust line-based scanner (`SourceFile` and `OrgDocument`) with shared helpers for headings, lists, and timestamps.
Compile the entire tool suite into a single native binary with zero Emacs runtime dependencies.

### Consequences

- **Benefits**: Sub-millisecond startup, simple self-contained rule modules, and trivial cross-platform installation.
- **Trade-offs**: Covers 38 of Emacs `org-lint`'s 58 checks while deferring checks that need deep cross-file AST evaluation.

## ADR-0002: Two-Phase Pipeline and Single-Pass Non-Overlapping `Fix` Spans

### Context

`org-tools` supports both auto-fixable `FormatRule` passes (`F001..F009`) and report-only `LintRule` checks (`E*`, `W*`, `I*`).
Running lint rules on unformatted text produces noisy false positives when whitespace or alignment is slightly off.
Applying text replacements iteratively across rules risks corrupting byte offsets when multiple rules touch nearby lines.

### Decision

Execute a two-phase pipeline inside `Runner`:

1. Run all enabled `FormatRule` structs against `SourceFile` to collect byte-range `Fix` structs (`[start, end)`).
1. Sort fixes by `span.start`, drop overlapping spans, and apply replacements in a single left-to-right pass.
1. Run all enabled `LintRule` structs against the formatted buffer to emit `Diagnostic` records.

### Consequences

- **Benefits**: Lint rules always inspect normalized input, and single-pass span application prevents offset drift.
- **Trade-offs**: Rules remain strictly independent and never depend on another rule's intermediate mutations.

## ADR-0003: `GPL-3.0-or-later` License and Protected Literal Regions

### Context

`org-tools` derives its rule semantics from studying the Org Mode Manual, `org-lint.el`, and `org-element.el`.
Furthermore, Org files frequently embed source code (`src`), shell output (`example`), poetry (`verse`), and prose quotes (`quote`).
Auto-formatting whitespace or headings inside those literal blocks would silently corrupt user code and quotations.

### Decision

License all repository code and documentation under `GPL-3.0-or-later` with standard Software Package Data Exchange (SPDX) headers.
Scan every buffer with `regions::protected_regions` (`#+BEGIN_` to `#+END_`) and require formatters to skip `regions::is_protected` lines.

### Consequences

- **Benefits**: Full license alignment with the Org-mode community and guaranteed safety for literal code blocks.
- **Trade-offs**: Every new rule author must check `regions::is_protected` and test literal block preservation.

## ADR-0004: Two-Crate Cargo Workspace and Umbrella `org` Binary

### Context

As `org-tools` grew from a formatter into a query, clocking, `org-roam`, and export suite, binary packaging needed structure.
Shipping separate binaries (`org-fmt`, `org-query`, `org-clock`) duplicates shared parsing code and clutters user `PATH` directories.

### Decision

Organize the repository as a two-crate Cargo workspace:

- `crates/org-tools-core`: Reusable library containing `SourceFile`, `OrgDocument`, `OrgLocator`, rules, mutators, and `WorkspaceCache`.
- `crates/org`: Unified umbrella binary exposing modular `clap` subcommands under `crates/org/src/commands/`.

### Consequences

- **Benefits**: Users install one `org` binary with consistent flags, exit codes (`0..3`), and JSON schemas.
- **Trade-offs**: Each CLI command module stays capped at 450 lines to prevent a monolithic binary crate.

## ADR-0005: Deterministic RFC 9562 UUIDv5 Identity Minting and `:ID_SEED:` Provenance

### Context

Random UUIDv4 `:ID:` values change on every generation run, breaking reproducible fixtures and idempotent migrations.
Automation scripts need stable node identifiers derived from a deterministic namespace and a human-auditable seed string.

### Decision

Support Deterministic RFC 9562 Universally Unique Identifier Version 5 (UUIDv5) minting in `property.rs` and `org update add-id`.
Compute the SHA-1 namespace hash over the seed and write both `:ID:` and `:ID_SEED:` into `:PROPERTIES:`.
Keep drawers aligned under `F005` and heading tags aligned to column 77 under `F007`.

### Consequences

- **Benefits**: Identical seeds always mint identical `:ID:` UUIDs, and `:ID_SEED:` preserves clear provenance in Git diffs.
- **Trade-offs**: Drawers carrying `:ID_SEED:` store one extra property line per deterministically minted node.

## ADR-0006: `org-roam` v2 File-Level Node, Alias, Reference, and Backlink Graph

### Context

Many Zettelkasten knowledge bases use `org-roam` v2, where an entire `.org` file acts as a node via a top-of-file `:PROPERTIES:` drawer.
A heading-only parser misses level-0 file `:ID:` properties, `:ROAM_ALIASES:`, `:ROAM_REFS:`, and `:ROAM_EXCLUDE:` markers.

### Decision

Extend `OrgDocument` and `OrgLocator` to parse level-0 file drawers, `#+TITLE:`, `#+FILETAGS:`, and `KEY+` property continuations.
Provide `org roam node`, `backlinks`, `links`, `graph`, `capture`, and `fleeting` commands for zero-Emacs Zettelkasten management.

### Consequences

- **Benefits**: `org` natively queries, links, captures, and promotes `org-roam` v2 notes without an Emacs daemon.
- **Trade-offs**: Locator resolution must check both level-0 file `:ID:` drawers and heading `:ID:` properties.

## ADR-0007: Incremental SQLite WAL and FTS5 `WorkspaceCache` (`.cache/org-cache.db`)

### Context

Scanning thousands of `.org` files from disk on every CLI query or MCP tool call adds unnecessary I/O latency.
However, any cache must never serve stale results after a user or command edits a file.

### Decision

Maintain an incremental SQLite Write-Ahead Logging (WAL) and Full-Text Search Version 5 (SQLite FTS5) index at `.cache/org-cache.db`.
Validate cached file records using modification time (`mtime`), byte length, and SHA-256 content hashes.
Invalidate modified paths immediately after mutations and provide `--no-cache` plus `org cache status|rebuild|clear`.

### Consequences

- **Benefits**: Warm workspace queries and backlink lookups complete in milliseconds with concurrent WAL reader support.
- **Trade-offs**: Mutations must call `invalidate_paths` so rapid sub-second writes never leave stale cache rows.

## ADR-0008: Deterministic `org-edna` Dependency Engine and Pure-Rust `#+TBLFM:` Evaluator

### Context

Power users rely on `org-edna` (`BLOCKER` and `TRIGGER` properties) for task workflows and `#+TBLFM:` for spreadsheet tables.
Evaluating arbitrary Emacs Lisp inside `.org` files poses security risks and requires a running Emacs process.

### Decision

Implement pure-Rust evaluators for `org-edna` finders (`ids`, `self`, `next-sibling`, `parent`, `children`) and actions (`todo!`, `scheduled!`, `deadline!`, `set-property!`).
Evaluate `#+TBLFM:` formulas (`vsum`, `vmean`, `vmin`, `vmax`, `vcount`, arithmetic, printf flags) in `tblfm.rs`.
Return exit code `3` (`EXIT_REQUIRES_EMACS`) whenever a formula requires Emacs Lisp or full Emacs Calc.

### Consequences

- **Benefits**: Safe, deterministic dependency transitions and table recalculations in pure Rust with explicit fallback codes.
- **Trade-offs**: Arbitrary `(elisp ...)` table formulas intentionally exit with code `3` instead of executing untrusted code.

## ADR-0009: Built-In Zero-Emacs `stdio` Model Context Protocol (MCP) Server

### Context

AI coding assistants and knowledge agents need structured, safe access to local `.org` task lists and `org-roam` vaults.
Shelling out to ad-hoc regex scripts often corrupts property drawers or leaks files outside the intended workspace.

### Decision

Embed a JSON-RPC 2.0 `stdio` Model Context Protocol (MCP) server directly into `org mcp`.
Confine all file operations to `--workspace` and `--roam-dir`, and return schema-validated `MutationResult` envelopes with execution telemetry.

### Consequences

- **Benefits**: AI hosts connect to `org mcp` with zero external wrappers, strict path sandboxing, and lint-clean writes.
- **Trade-offs**: All diagnostic logs during MCP execution must route to `stderr` so `stdout` carries pure JSON-RPC frames.

## ADR-0010: Multi-Vault Workflows and Three-Manual Zensical Documentation

### Context

Users often separate Getting Things Done (GTD) task directories from long-term Zettelkasten (`org-roam`) vaults and fleeting capture inboxes.
Meanwhile, end users, systems operators, and Rust contributors need distinct documentation tailored to their roles.

### Decision

Support independent or unified vault paths via `.org-tools.toml` (`[roam]`, `[capture.templates.<name>]`) and CLI flags.
Publish documentation across three audience-scoped Zensical manuals (`User`, `Operator`, `Developer`), a dedicated [Glossary](../glossary.md), Unix `roff` manpages (`man/org.1`, `man/org-tools.toml.5`), and `org schema` contracts.

### Consequences

- **Benefits**: Readers find task recipes, operator runbooks, and `arc42` contributor guides without wading through mixed concerns.
- **Trade-offs**: Documentation changes must pass `uvx zensical build --strict` and readability verification tests.
