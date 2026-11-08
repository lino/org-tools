<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Developer Manual

## Overview

**What you will learn:**

- How the `org-tools` Cargo workspace separates core domain logic from the Command-Line Interface (CLI).
- Which verification commands enforce formatting, linting, test coverage, and documentation quality.
- Where to find the architecture dossier, contributor guide, decision catalog, and glossary.

`org-tools` is a standalone, pure-Rust Command-Line Interface (CLI) suite for Org-mode files.
The workspace compiles into a single `org` binary without requiring an Emacs runtime.
Contributors use Test-Driven Development (TDD) and strict complexity bounds to keep modules readable.

## Workspace Crate Map

The Cargo workspace contains two crates with clear module boundaries.
`crates/org-tools-core` holds all parsing, rule evaluation, document indexing, and mutation engines.
`crates/org` provides the `clap` CLI parser, subcommand handlers, and `stdio` server transport.

| Crate            | Path                     | Primary Responsibilities                                                                                              |
| ---------------- | ------------------------ | --------------------------------------------------------------------------------------------------------------------- |
| `org-tools-core` | `crates/org-tools-core/` | `SourceFile`, `OrgDocument`, `OrgLocator`, `FormatRule`, `LintRule`, `Runner`, `WorkspaceCache`, `edna`, and `tblfm`. |
| `org`            | `crates/org/`            | Top-level `clap` CLI, exit codes (`0..3`), modular `commands/*.rs` handlers, exporters, and JSON schemas.             |

### Shared Library (`crates/org-tools-core`)

`crates/org-tools-core` exposes pure-Rust data structures and traits for every Org-mode operation.
It implements line-indexed source buffers, heading trees, and level-0 `org-roam` v2 file drawers.
It also manages the incremental SQLite Write-Ahead Logging (WAL) cache in `.cache/org-cache.db`.

### Umbrella Binary (`crates/org`)

`crates/org` dispatches subcommands (`fmt`, `query`, `roam`, `show`, `refile`, `update`, `clock`, `archive`, `export`, `cache`, `mcp`, `schema`).
Each subcommand handler lives in a dedicated module under `crates/org/src/commands/` within 450 lines.
All mutating commands emit uniform `MutationResult` JSON envelopes with execution telemetry.

## Quick Contributor Workflow

1. Run `mise run setup` to install the Rust toolchain and verification utilities.
1. Write a failing unit or integration test before modifying any production code.
1. Implement the minimal Rust change while keeping functions at or below 50 lines.
1. Run `mise run test` to execute all workspace unit and integration test suites.
1. Run `mise run verify` to check formatting, `clippy` lints, tests, and documentation.

## Verification Gates

Run these commands locally before submitting any change for review.
Every gate must exit with code `0` and produce zero warnings or diagnostics.

| Command                                     | Purpose                                                                                      |
| ------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `mise run test`                             | Run all unit tests in `org-tools-core` and CLI integration tests in `crates/org/tests/`.     |
| `mise run verify`                           | Run formatting checks, `clippy --all-targets`, full test suites, and self-dogfooding checks. |
| `cargo fmt -- --check`                      | Verify standard Rust formatting across both workspace crates.                                |
| `cargo clippy --all-targets -- -D warnings` | Lint library, binary, and test targets with all warnings denied.                             |
| `cargo doc --workspace --no-deps`           | Build `rustdoc` API documentation and verify intra-doc links.                                |
| `cargo run -- fmt check README.org docs/`   | Self-dogfood `org fmt check` across repository `.org` documentation files.                   |

## Developer Manual Chapters

- [Architecture (`arc42`)](architecture.md): Read the 12-section `arc42` architecture dossier and four balanced Mermaid diagrams.
- [Contributing & TDD Guide](contributing.md): Follow step-by-step instructions to add a `FormatRule` or `LintRule` with Red-to-Green TDD.
- [Architecture Decisions](decisions.md): Review Architecture Decision Records (`ADR-0001` through `ADR-0010`) covering parsing, caching, and protocols.
- [Glossary](../glossary.md): Look up alphabetical definitions and acronym expansions across the `org-tools` ecosystem.
