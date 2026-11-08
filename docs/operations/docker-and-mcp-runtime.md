<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Container & MCP Server Runtime Operations

## Overview

**What you will learn:**

- Run the multi-architecture `ghcr.io/lino/org-tools` container across Docker and Podman.
- Confine the standard input/output (`stdio`) Model Context Protocol (`MCP`) server with `--workspace` and `--roam-dir`.
- Manage SQLite Write-Ahead Logging (`WAL`) `WorkspaceCache` (`.cache/org-cache.db`) volume permissions and zero-network isolation.

Running `org` inside an isolated container or bounded subprocess prevents untrusted AI assistants from accessing files outside your Org workspace. The built-in MCP server enforces canonical path confinement and opens zero network sockets. Operators gain sub-millisecond SQLite Full-Text Search (`FTS5`) queries while keeping host filesystems secure.

## Prerequisites

- Docker Engine 24+ or Podman 4+ (or a local `org` binary for direct subprocess execution).
- An Org-mode workspace directory containing `.org` files and an optional `.org-tools.toml` configuration file.
- Matching host user and group IDs (`uid:gid`) on mounted volumes when `.cache/org-cache.db` caching is active.

## Steps

1. Pull the multi-architecture container image from GitHub Container Registry (`GHCR`) or build it from the repository `Dockerfile`.

```sh
docker pull ghcr.io/lino/org-tools:latest
docker build -t org-tools:local .
```

2. Run formatting checks and agenda queries against a bind-mounted host workspace using your host user ID.

```sh
docker run --rm -u "$(id -u):$(id -g)" \
  -v "$PWD:/workspace" \
  ghcr.io/lino/org-tools:latest fmt check .

docker run --rm -u "$(id -u):$(id -g)" \
  -v "$PWD:/workspace" \
  ghcr.io/lino/org-tools:latest query agenda /workspace
```

3. Configure workspace boundary confinement and `org-roam` directory paths via `--workspace`, `ORG_TOOLS_WORKSPACE`, and `--roam-dir`.

```sh
# Confine roam indexing and backlink resolution to a specific subdirectory
org roam sync --roam-dir /workspace/notes
org roam node "Project Architecture" --roam-dir /workspace/notes --format json
```

```toml
# .org-tools.toml
[roam]
roam_dir = "notes"
```

4. Start the `stdio` JSON-RPC 2.0 (`JSON-RPC`) MCP server inside a zero-network, privilege-dropped container.

```sh
docker run -i --rm \
  --network none \
  --cap-drop=ALL \
  --security-opt=no-new-privileges \
  -u "$(id -u):$(id -g)" \
  -v "$PWD:/workspace" \
  ghcr.io/lino/org-tools:latest mcp --workspace /workspace
```

5. Verify the MCP handshake and incremental SQLite WAL `WorkspaceCache` (`.cache/org-cache.db`) over `stdio`.

```sh
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}\n' \
  | org mcp --workspace "$PWD"
```

## Expected Outcome

The MCP server writes a JSON-RPC 2.0 `initialize` response to `stdout` and exits `0` upon `stdin` EOF. Subsequent tool calls maintain a warm SQLite WAL connection at `.cache/org-cache.db` inside the confined workspace.

```text
{"id":1,"jsonrpc":"2.0","result":{"capabilities":{"tools":{"listChanged":false}},"protocolVersion":"2024-11-05","serverInfo":{"name":"org-tools","version":"0.1.4"}}}
$ echo $?
0
```

## Power-User & Scripting Options

Pass the global `--no-cache` flag when mounting a workspace volume read-only (`-v "$PWD:/workspace:ro"`) so `org` never creates `.cache/org-cache.db`. You can also set `ORG_TOOLS_CONFIG` to load a shared `.org-tools.toml` outside the mounted directory. See the [MCP Server Cookbook](../cookbook/mcp-server.md) and the [Glossary and Abbreviations](../glossary.md) for client integration patterns.

```sh
# Execute read-only MCP queries over a read-only bind mount without SQLite disk writes
docker run -i --rm --network none --read-only \
  -v "$PWD:/workspace:ro" \
  ghcr.io/lino/org-tools:latest --no-cache mcp --workspace /workspace
```

??? info "Deep Dive: Path Confinement & SQLite WAL Cache Mechanics"
`org mcp` resolves the startup workspace from `--workspace`, `ORG_TOOLS_WORKSPACE`, or ancestor project markers (`.org-tools.toml`, `.git`). Every tool file path or `OrgLocator` must carry a `.org` extension and contain zero `..` segments. Canonical paths must stay inside the workspace root, and external symlinks return an immediate JSON-RPC error.

```
When caching is enabled, `WorkspaceCache` opens `.cache/org-cache.db` in SQLite Write-Ahead Logging (`WAL`) mode alongside `.cache/org-cache.db-wal` and `.cache/org-cache.db-shm`. Ensure the container user (`-u "$(id -u):$(id -g)"`) has write access to `.cache/`, or pass `--no-cache` for immutable volumes.
```

## Troubleshooting

| Symptom                                                  | Cause                                                           | Fix                                                                                |
| -------------------------------------------------------- | --------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `path escapes workspace boundary: <path>`                | Target path or symlink resolves outside `--workspace`           | Move the `.org` file inside the workspace root or adjust `--workspace`             |
| `path traversal outside workspace is not allowed`        | Tool argument contains `..` parent directory segments           | Pass workspace-relative paths without `..` segments                                |
| `only .org files are supported: <path>`                  | MCP tool received a non-`.org` file path                        | Restrict MCP tool calls to `.org` files inside the configured workspace            |
| `attempt to write a readonly database` on `org-cache.db` | Container `uid:gid` lacks write permission on `.cache/`         | Pass `-u "$(id -u):$(id -g)"` to `docker run` or add `--no-cache` for `:ro` mounts |
| `workspace directory not found: <path>`                  | Host volume was not mounted at the path passed to `--workspace` | Match `-v "$PWD:/workspace"` with `--workspace /workspace`                         |
