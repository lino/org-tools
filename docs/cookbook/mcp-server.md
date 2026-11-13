<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# AI Assistants & MCP Server Recipe

## Overview

**What you will learn:**

- Why structured Org-aware tools protect drawers, planning timestamps, and link integrity from AI hallucination.
- How to configure portable zero-path Model Context Protocol (MCP) clients in two minutes.
- How assistants call `org_*` tools to review next actions, capture notes, and process queues.
- How to configure multi-vault confinement and inspect schemas with `org schema fleeting`.

Giving an AI assistant raw text editing access to `.org` files frequently corrupts drawers and broken timestamps.
The built-in `org mcp` server exposes 16 high-level semantic tools over standard input and output (`stdio`).
These tools enforce strict formatting and confine operations to your workspace boundary.
Your notes stay clean while your AI assistant gains full context across your vaults.

## Prerequisites

- `org` installed on your workstation or container host (see [Multi-Platform Installation](../operations/installation.md)).
- An MCP-compatible client such as Antigravity, Claude Code, Claude Desktop, Cursor, or Zed.
- An Org workspace directory with an `.org-tools.toml` configuration file.

## Steps

1. Warm the workspace SQLite index at `.cache/org-cache.db` to accelerate tool execution.
   ```sh
   org cache rebuild --workspace .
   org cache status --workspace .
   ```
1. Configure your editor or agent with a portable `"command": "org"` declaration in its MCP configuration.
   For Antigravity, Claude Desktop, or Cursor (`.mcp.json`):
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
   For Zed (`settings.json`):
   ```json
   {
     "experimental.context_servers": {
       "org": {
         "command": "org",
         "args": ["mcp", "--workspace", "."]
       }
     }
   }
   ```
1. Test tool discovery over `stdio` by sending an `initialize` and `tools/list` request.
   ```sh
   printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}\n{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}\n' \
     | org mcp --workspace .
   ```
1. Query unblocked next actions or review fleeting thoughts with `org_query_next` and `org_fleeting_list`.
   ```sh
   printf '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"org_query_next","arguments":{"limit":3}}}\n' \
     | org mcp --workspace .
   printf '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"org_fleeting_list","arguments":{"status":"unrefined"}}}\n' \
     | org mcp --workspace .
   ```
1. Capture structured concept notes or transform fleeting items with `org_roam_capture` and `org_fleeting_transform`.
   ```sh
   printf '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"org_roam_capture","arguments":{"title":"Transformer Attention","body":"Attention mechanisms route information dynamically.","type":"concept"}}}\n' \
     | org mcp --workspace .
   printf '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"org_fleeting_transform","arguments":{"target":"1","action":"to_zettel","title":"Attention Routing"}}}\n' \
     | org mcp --workspace .
   ```
1. Transition tasks, refile subtrees, or manage clock timers with workspace-confined tools like `org_update_state`.
   ```sh
   printf '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"org_update_state","arguments":{"locator":"vaults/work/tasks.org::7","state":"DONE"}}}\n' \
     | org mcp --workspace .
   ```

## Expected Outcome

`org mcp` returns structured JSON-RPC responses containing execution results and telemetry metadata.
Transitions advance state and write clean `:LOGBOOK:` entries without disrupting surrounding text:

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "result": {
    "content": [{ "type": "text", "text": "Transitioned task to DONE" }],
    "isError": false,
    "structuredContent": {
      "telemetry": { "duration_us": 320, "files_scanned": 0, "cache_hits": 1, "cache_misses": 0 }
    }
  }
}
```

## Power-User & Scripting Options

Inspect the schema contract for fleeting note tools using `org schema fleeting`.
Enforce multi-vault isolation by scoping tool invocations with `roam_dir`:

```sh
# Print the JSON Schema Draft 2020-12 contract for fleeting tools
org schema fleeting --output schemas/fleeting.schema.json

# Confine an MCP session strictly to a container workspace
docker run -i --rm --network none --cap-drop=ALL \
  -u "$(id -u):$(id -g)" -v "$PWD:/workspace" \
  ghcr.io/lino/org-tools:latest mcp --workspace /workspace
```

??? info "Deep Dive: Exposed MCP Tool Catalog and Vault Confinement"

    The built-in MCP server exposes 16 tools covering the complete Org lifecycle.
    Each tool operates within configured `--workspace` paths and validates file extensions.
    Pass `roam_dir` arguments to confine operations to a designated vault.
    Tools return structured envelopes with execution telemetry and zero drawer formatting drift.

    | MCP Tool Name            | Mode  | Description                                            |
    | ------------------------ | ----- | ------------------------------------------------------ |
    | `org_roam_node`          | Read  | Resolve an `org-roam` node by ID, title, or alias.     |
    | `org_roam_backlinks`     | Read  | List incoming backlinks pointing to a target node.     |
    | `org_roam_search`        | Read  | Run queries or full-text searches across the vault.    |
    | `org_roam_capture`       | Write | Capture a note or task using a built-in template.      |
    | `org_fleeting_list`      | Read  | List pending or refined fleeting notes across vaults.  |
    | `org_fleeting_transform` | Write | Reform a fleeting note into a concept note or task.    |
    | `org_query_next`         | Read  | List actionable next actions filtered by context.      |
    | `org_query_blocked`      | Read  | List tasks blocked by unresolved dependencies.         |
    | `org_query_agenda`       | Read  | Project a multi-day agenda timeline with habit graphs. |
    | `org_show`               | Read  | Extract a bounded subtree or file node by locator.     |
    | `org_update_state`       | Write | Transition a TODO state and trigger cascades.          |
    | `org_update_planning`    | Write | Set, shift, or clear planning timestamps.              |
    | `org_clock_in`           | Write | Start a running clock inside an entry logbook.         |
    | `org_clock_out`          | Write | Stop an active clock and record the duration.          |
    | `org_refile`             | Write | Move an entry subtree to a new target outline.         |
    | `org_fmt_file`           | Write | Format an Org file in place and refresh cache.         |

## Troubleshooting

| Symptom                                    | Cause                                                   | Fix                                                            |
| ------------------------------------------ | ------------------------------------------------------- | -------------------------------------------------------------- |
| `path escapes workspace boundary` error    | Requested file resolves outside `--workspace`           | Pass paths located within the workspace root directory         |
| Client fails to start `org mcp`            | The `org` binary is missing from GUI application `PATH` | Ensure the `org` binary directory is included in system `PATH` |
| `only .org files are supported` error      | Tool targeted an unsupported file type                  | Restrict `org_*` tool invocations to `.org` files              |
| Read-only filesystem error on SQLite cache | Workspace is mounted read-only without `--no-cache`     | Pass `--no-cache` to the `org mcp` command arguments           |
