<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# AI Assistants & MCP Server Recipe (`org mcp`)

## Overview

**What you will learn:**

- How to register `org mcp` as a standard input/output (`stdio`) Model Context Protocol (MCP) server in `.mcp.json`.
- How AI coding and knowledge assistants call the 10 built-in workspace-confined `org_*` MCP tools.
- How the warm SQLite Write-Ahead Logging (`WAL`) `WorkspaceCache` keeps AI queries fast and lint-clean.

Giving an AI assistant raw shell access to edit `.org` files often results in mangled drawers and broken timestamps.
`org mcp` embeds a zero-Emacs JSON Remote Procedure Call 2.0 (`JSON-RPC`) server that confines every path to your workspace.
Assistants can search notes, inspect bounded subtrees, and transition tasks with guaranteed `F005` and `F007` formatting.

## Prerequisites

- `org` installed on your workstation or container host (see [Multi-Platform Installation](../operations/installation.md)).
- An MCP-compatible client (such as Claude Code, Claude Desktop, Cursor, or Zed).
- An Org workspace directory containing `.org` files.

## Steps

1. Warm and verify the workspace SQLite WAL and Full-Text Search (`SQLite FTS5`) cache at `.cache/org-cache.db`.
   ```sh
   org cache rebuild --workspace .
   org cache status --workspace .
   ```
1. Create `.mcp.json` in your project root using portable command resolution (never hardcode user home paths).
   ```json
   {
     "mcpServers": {
       "org": {
         "command": "org",
         "args": ["mcp", "--workspace", "."],
         "type": "stdio"
       }
     }
   }
   ```
1. Test the `stdio` JSON-RPC 2.0 handshake and `tools/list` discovery from your terminal.
   ```sh
   printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}\n{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}\n' \
     | org mcp --workspace .
   ```
1. Invoke read tools (`org_query_next`, `org_query_agenda`, `org_roam_search`, `org_show`) from your AI assistant to gather bounded context.
   ```sh
   printf '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"org_query_next","arguments":{"limit":5}}}\n' \
     | org mcp --workspace .
   ```
1. Invoke write tools (`org_update_state`, `org_update_planning`, `org_fmt_file`) with `dry_run: true` first, then commit the change.
   ```sh
   printf '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"org_update_state","arguments":{"locator":"tasks.org::7","state":"DONE","dry_run":true}}}\n' \
     | org mcp --workspace .
   ```

## Expected Outcome

`org mcp` responds to `initialize`, `tools/list`, and `tools/call` frames over `stdout` and exits with code `0` on `stdin` EOF.
Every `tools/call` response includes `structuredContent` and execution `telemetry` (`duration_us`, `cache_hits`, `cache_misses`):

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "content": [{ "type": "text", "text": "..." }],
    "isError": false,
    "structuredContent": {
      "telemetry": {
        "duration_us": 285,
        "files_scanned": 0,
        "cache_hits": 1,
        "cache_misses": 0
      }
    }
  }
}
```

## Power-User & Scripting Options

Run `org mcp` inside a network-isolated Docker container (`--network none`) when connecting untrusted AI agents.
See [Container & MCP Runtime Operations](../operations/docker-and-mcp-runtime.md) and the [Glossary](../glossary.md) for security details:

```sh
# Launch a zero-network, privilege-dropped container for MCP stdio transport
docker run -i --rm --network none --cap-drop=ALL \
  -u "$(id -u):$(id -g)" -v "$PWD:/workspace" \
  ghcr.io/lino/org-tools:latest mcp --workspace /workspace
```

??? info "Deep Dive: Exposed MCP Tool Catalog"

| MCP Tool Name         | Mode  | Description                                                                         |
| --------------------- | ----- | ----------------------------------------------------------------------------------- |
| `org_roam_node`       | Read  | Resolve an `org-roam` v2 node by `id:<uuid>`, `:ID:`, title, or `:ROAM_ALIASES:`.   |
| `org_roam_backlinks`  | Read  | List incoming `[[id:...]]` backlinks targeting a node.                              |
| `org_roam_search`     | Read  | Run boolean query predicates or `SQLite FTS5` `body:` searches across the vault.    |
| `org_query_next`      | Read  | List actionable GTD next actions filtered by optional `@context`.                   |
| `org_query_blocked`   | Read  | List `org-edna` blocked tasks and their blocking entries.                           |
| `org_query_agenda`    | Read  | Project a multi-day agenda timeline with optional composite GTD sections.           |
| `org_show`            | Read  | Extract a bounded single-node slice (`json`, `org`, or `md`) with `--max-chars`.    |
| `org_update_state`    | Write | Transition a task TODO state with `:LOGBOOK:`, repeaters, and `:TRIGGER:` cascades. |
| `org_update_planning` | Write | Set, shift (`+3d`), or clear `SCHEDULED:` and `DEADLINE:` planning dates.           |
| `org_fmt_file`        | Write | Check or auto-format a workspace `.org` file and refresh `.cache/org-cache.db`.     |

## Troubleshooting

| Symptom                                             | Cause                                                       | Fix                                                                                |
| --------------------------------------------------- | ----------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `path escapes workspace boundary` in tool error     | Requested file or symlink resolves outside `--workspace`    | Pass workspace-relative `.org` paths inside the configured workspace root          |
| AI client fails to start `org mcp`                  | `org` binary is not in the GUI application's default `PATH` | Install via Homebrew (`/opt/homebrew/bin/org`) or add `~/.cargo/bin` to launch env |
| `only .org files are supported` error               | Tool call targeted a `.md` or `.toml` file                  | Restrict `org_*` MCP tool calls to `.org` files                                    |
| Read-only filesystem error on `.cache/org-cache.db` | Workspace volume is mounted `:ro` without `--no-cache`      | Add `"--no-cache"` to `"args"` in `.mcp.json` when using read-only mounts          |
