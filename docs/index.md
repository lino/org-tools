<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# `org-tools` Documentation

## Overview

**What you will learn:**

- What the standalone `org` Command-Line Interface (CLI) does for Org-mode workspaces without requiring Emacs.
- Why individuals, teams, and AI agents use `org-tools` for fast formatting, querying, and task mutations.
- Which manual to read based on your role as an end user, systems operator, or Rust contributor.

`org-tools` provides a single, pure-Rust binary (`org`) for linting, formatting, querying, updating, clocking, and exporting `.org` files.
It runs in milliseconds on local workstations, inside Continuous Integration (CI) pipelines, and inside isolated containers.
Every rule and mutator follows the authoritative Org-mode specification while keeping literal code blocks safe.

## Choose Your Manual

This documentation site is organized into three role-specific manuals, a goal-oriented Cookbook, and a shared Glossary:

| Manual or Section                         | Target Audience                 | What You Will Find                                                                                              |
| ----------------------------------------- | ------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| [User Manual](user/quickstart.md)         | End users & power users         | Five-minute quickstart, formatting, querying, agendas, task mutations, Unix piping, JSON schemas, and CLI args. |
| [Cookbook](cookbook/index.md)             | Task, knowledge & CI builders   | Step-by-step recipes for Getting Things Done (GTD), Zettelkasten notes, Model Context Protocol (MCP), and CI.   |
| [Operator Manual](operations/index.md)    | Systems operators & release eng | Multi-OS installation, container & MCP sandboxing, GoReleaser v2 runbooks, and Prometheus metrics.              |
| [Developer Manual](developer/index.md)    | Contributors & Rust integrators | Workspace crate map, 12-section `arc42` architecture, Red-to-Green Test-Driven Development (TDD), and ADRs.     |
| [Glossary and Abbreviations](glossary.md) | All readers                     | Alphabetical definitions and plain-language expansions for every Org-mode and protocol acronym.                 |

## Core Capabilities at a Glance

- **Format and Lint (`org fmt`)**: Auto-fix 9 formatting rules (`F001..F009`) and check 38 lint rules (`E*`, `W*`, `I*`) while preserving literal blocks.
- **Query and Agenda (`org query`)**: Search headings, project multi-day timelines with `org-habit` graphs, and evaluate `#+COLUMNS:` rollups.
- **GTD Action Navigation (`org next`, `org roam gtd`)**: Pick immediate next actions across multi-vault directories and capture unclarified inbox items.
- **Zettelkasten and Fleeting Notes (`org fleeting`, `org roam`, `org show`)**: Process Luhmann fleeting queues, capture concept notes, and inspect backlinks.
- **Safe Mutations (`org update`, `org refile`, `org clock`, `org archive`)**: Transition TODO states, mint deterministic UUIDv5 IDs, and track effort clocks.
- **Built-In MCP & LSP Servers (`org mcp`, `org lsp`)**: Expose 16 confined MCP tools and a Language Server Protocol server over `stdio`.

??? info "Deep Dive: Single Pure-Rust Executable Topology"
`org` combines formatting, querying, knowledge graphs, and editor protocols into a single native binary.
The tool requires zero external database servers, background processes, or runtime interpreters.
All operations run against local `.org` files and an incremental SQLite cache located at `.cache/org-cache.db`.

## Quick Navigation by Goal

- **Install `org` and run your first command**: Start with the [Five-Minute Quickstart](user/quickstart.md) or [Multi-Platform Installation](operations/installation.md).
- **Clean up tables, drawers, and tags**: Read [Formatting & Linting](user/formatting-and-linting.md).
- **Search tasks and build daily agendas**: Read [Querying & Agenda Views](user/querying-and-agenda.md) and the [GTD Cookbook](cookbook/gtd.md).
- **Connect an AI assistant via `org mcp`**: Follow the [AI Assistants & MCP Server Cookbook](cookbook/mcp-server.md).
- **Look up flags, exit codes, and manpages**: Open the [CLI, Config & Rule Reference](user/reference.md) or run `man 1 org` and `man 5 org-tools.toml`.
