<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Operator Manual

## Overview

**What you will learn:**

- How `org` deploys as a static binary, system package, Homebrew Cask, or rootless container.
- How workspace confinement (`--workspace`, `--roam-dir`) and `.cache/org-cache.db` SQLite permissions work in production.
- Where to find installation, container/MCP runtime, GoReleaser v2 release, and Prometheus observability runbooks.

`org-tools` requires zero background daemons, external databases, or network sockets.
Systems operators manage a single static executable (`org`) and an optional local SQLite Write-Ahead Logging (`WAL`) cache.
This manual covers multi-platform rollout, container isolation, release automation, and Prometheus telemetry.

## Runtime Topology

Every deployment mode executes the same pure-Rust `org` binary against local `.org` files and `.cache/org-cache.db`:

| Deployment Mode             | Typical Environment               | Security & Storage Characteristics                                                                    |
| --------------------------- | --------------------------------- | ----------------------------------------------------------------------------------------------------- |
| Local Workstation CLI       | macOS, Linux, Windows terminals   | Reads ancestor `.org-tools.toml` and stores `.cache/org-cache.db` inside the workspace.               |
| `stdio` MCP Subprocess      | Claude Code, Cursor, IDE AI hosts | Enforces canonical `--workspace` path confinement over JSON Remote Procedure Call 2.0 (`JSON-RPC`).   |
| Rootless Docker / Podman    | `ghcr.io/lino/org-tools:latest`   | Runs with `--network none`, `--cap-drop=ALL`, and bind-mounted `/workspace` (`--no-cache` for `:ro`). |
| GitHub Actions & CI Runners | Pull request & release pipelines  | Executes `org fmt`, `org query`, and `scripts/verify-release-packages.sh` with Prometheus telemetry.  |

??? info "Deep Dive: Rootless Container Isolation and SQLite WAL Safety"
The official container image contains no shell utilities or package managers.
Executing with `--network none` and `--cap-drop=ALL` prevents network access and privilege escalation.
When mounting workspaces read-only, pass `--no-cache` to avoid Write-Ahead Logging failures.
The binary parses files directly from memory without creating disk lock files.

## Operator Runbooks

- [Multi-Platform Installation & Verification](installation.md): Install via Homebrew Cask, `.deb`, `.rpm`, Winget, Scoop, tarballs, or Cargo, and verify `man 1 org` / `man 5 org-tools.toml`.
- [Container & MCP Server Runtime Operations](docker-and-mcp-runtime.md): Confine `org mcp` with `--workspace` and `--roam-dir`, manage SQLite WAL permissions, and run zero-network containers.
- [Release & Homebrew Tap Runbook](release-and-tap.md): Build six cross-platform targets, package manpages and completions, verify SHA-256 digests, and update `lino/homebrew-tap`.
- [Prometheus Metrics & Troubleshooting](metrics-and-troubleshooting.md): Scrape `orgfmt_release_*` Prometheus metrics, query W3C trace-correlated JSON Lines (`JSONL`) logs, and triage alerts.

See the [Glossary and Abbreviations](../glossary.md) for definitions of all operational and protocol terms.
