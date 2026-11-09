<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Editors & LSP Server Recipe (`org lsp`)

## Overview

**What you will learn:**

- How to start `org lsp` as a standard input/output (`stdio`) Language Server Protocol server.
- How to configure Neovim, Helix, VS Code, Zed, and Emacs `eglot` for Org editing.
- How real-time diagnostics, formatting, completion, hover, navigation, and workspace rename work without Emacs.

Modern text editors provide rich Language Server Protocol (`LSP`) client integration.
Running `org lsp` brings zero-Emacs diagnostics, formatting, completion, hover, and workspace rename to your preferred editor.
An incremental SQLite Write-Ahead Logging (`WAL`) cache keeps symbol lookups fast across large vaults.

## Prerequisites

- `org` binary installed and accessible in your system `PATH`.
- An LSP-capable editor such as Neovim, Helix, VS Code, Zed, or Emacs 29+.
- An Org workspace directory containing `.org` files.

## Steps

1. Warm the workspace SQLite WAL cache at `.cache/org-cache.db`.
   ```sh
   org cache rebuild --workspace .
   ```
1. Configure Neovim using `nvim-lspconfig` or native `vim.lsp.config`.
   ```lua
   -- ~/.config/nvim/after/ftplugin/org.lua
   vim.lsp.start({
     name = 'org-lsp',
     cmd = { 'org', 'lsp', '--workspace', vim.fn.getcwd() },
     root_dir = vim.fn.getcwd(),
   })
   ```
1. Configure Helix in `~/.config/helix/languages.toml`.
   ```toml
   [language-server.org-lsp]
   command = "org"
   args = ["lsp", "--workspace", "."]

   [[language]]
   name = "org"
   scope = "source.org"
   injection-regex = "org"
   file-types = ["org"]
   language-servers = ["org-lsp"]
   ```
1. Configure VS Code or Zed to run `org lsp --workspace .` over stdio.
   In Zed, register `org lsp` as an external language server in settings:
   ```json
   {
     "lsp": {
       "org-lsp": {
         "binary": { "path": "org", "arguments": ["lsp", "--workspace", "."] }
       }
     }
   }
   ```
1. Configure Emacs 29+ `eglot` for Org buffers.
   ```elisp
   ;; Add to ~/.emacs.d/init.el
   (with-eval-after-load 'eglot
     (add-to-list 'eglot-server-programs '(org-mode . ("org" "lsp"))))
   ```

## Expected Outcome

Your editor receives real-time `F*` format and `E*`/`W*`/`I*` lint diagnostics on buffer change.
Triggering code actions auto-fixes formatting issues and adds missing `:ID:` properties.
Typing `[[id:` or `[[roam:` suggests vault nodes with Markdown hover previews.
Renaming a heading updates all incoming cross-vault backlinks and semantic tokens highlight drawer keywords.

Test the stdio JSON-RPC handshake directly from your terminal:

```sh
printf 'Content-Length: 85\r\n\r\n{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}' \
  | org lsp --workspace .
```

`org lsp` replies with server capabilities and protocol version before exiting on stdin EOF.

## Power-User & Scripting Options

Run `org lsp` with explicit directory flags to control cache scope:

```sh
# Bind LSP to an isolated workspace and external roam directory
org lsp --workspace /path/to/workspace --roam-dir /path/to/roam
```

Set `ORG_LSP_TELEMETRY_DIR` to record Prometheus metrics (`orgfmt_lsp.prom`) and JSONL events (`events.jsonl`).
Pass `--no-cache` to bypass `.cache/org-cache.db` when working on immutable mounts.

??? info "Deep Dive: Supported LSP Capabilities"

| Capability      | LSP Method                         | Description                                         |
| --------------- | ---------------------------------- | --------------------------------------------------- |
| Diagnostics     | `textDocument/publishDiagnostics`  | Real-time `F*`, `E*`, `W*`, and `I*` diagnostics.   |
| Formatting      | `textDocument/formatting`          | In-place file formatting matching `org fmt format`. |
| Code Actions    | `textDocument/codeAction`          | Quick-fixes for auto-fixable rules and missing IDs. |
| Completion      | `textDocument/completion`          | Targets for `[[id:...]]` and `[[roam:...]]` links.  |
| Hover           | `textDocument/hover`               | Heading preview and metadata summary for links.     |
| Definition      | `textDocument/definition`          | Jump to target heading or `org-roam` file node.     |
| References      | `textDocument/references`          | List all incoming backlinks across the vault.       |
| Rename          | `textDocument/rename`              | Workspace-wide heading title and custom ID rename.  |
| Semantic Tokens | `textDocument/semanticTokens/full` | Highlighting for drawers, tags, and timestamps.     |

## Troubleshooting

| Symptom                                 | Cause                                    | Fix                                                                |
| --------------------------------------- | ---------------------------------------- | ------------------------------------------------------------------ |
| Editor reports `org: command not found` | `org` executable not in editor `PATH`    | Add `~/.cargo/bin` or `/opt/homebrew/bin` to your environment path |
| Completion shows zero `[[id:` targets   | Cache is empty or pointing to wrong root | Run `org cache rebuild --workspace .` to index the workspace       |
| Read-only database error on cache file  | Directory lacks write permissions        | Pass `--no-cache` or grant write access to `.cache/org-cache.db`   |
| Diagnostics lag behind buffer edits     | High disk latency or huge files          | Check `orgfmt_lsp_request_duration_seconds` in telemetry output    |
