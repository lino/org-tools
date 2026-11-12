<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# `org-tools` Cookbook

## Overview

**What you will learn:**

- Which goal-oriented recipe matches your daily task, knowledge-graph, editor, AI assistant, or Continuous Integration (CI) workflow.
- How to combine `org` subcommands (`next`, `fleeting`, `roam`, `query`, `update`, `mcp`, `lsp`, `fmt`) to solve real-world problems.
- How to run multi-vault setups separating Getting Things Done (GTD) tasks from Zettelkasten permanent notes.

Reference manuals explain individual flags, while cookbooks show how commands fit together in daily practice.
Each recipe in this section walks through a complete end-to-end workflow with copy-pasteable commands and expected outputs.
Pick the recipe that matches your immediate goal below.

## Recipe Catalog

| Cookbook Recipe                                         | Primary Goal & Problem Solved                                                                  | Key Subcommands Used                                                  |
| ------------------------------------------------------- | ---------------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| [Getting Things Done (GTD)](gtd.md)                     | Capture inbox tasks, scaffold project nodes, pick next actions, and run weekly reviews.        | `org roam gtd`, `org next`, `org query`, `org update`, `org archive`  |
| [Zettelkasten & Fleeting Notes](zettelkasten.md)        | Capture Luhmann fleeting thoughts, process queues, create atomic notes, and inspect backlinks. | `org fleeting`, `org roam capture`, `org roam node\|backlinks\|graph` |
| [AI Assistants & MCP Server](mcp-server.md)             | Connect Antigravity, Claude, Cursor, or Zed to a workspace-confined 16-tool MCP server.        | `org mcp`, `org cache`, `org schema fleeting`                         |
| [Editors & LSP Server](lsp-editor-setup.md)             | Connect Neovim, Helix, VS Code, Zed, or Emacs `eglot` to the Language Server Protocol server.  | `org lsp`, `org cache`, `org fmt`                                     |
| [CI Pipelines & Shell Automation](automation-and-ci.md) | Enforce formatting and lint gates in GitHub Actions, pre-commit hooks, and shell scripts.      | `org fmt`, `org schema`, `org query`, `org update`                    |

## Multi-Vault Workspace Pattern

Many users keep fast-moving GTD task files separate from long-term Zettelkasten (`org-roam`) knowledge notes.
Configure `.org-tools.toml` at your workspace root so every recipe targets the right directory automatically:

```toml
[todo]
sequences = ["TODO NEXT WAITING | DONE CANCELLED"]
todo = ["TODO", "NEXT", "WAITING"]
done = ["DONE", "CANCELLED"]

[roam.directories]
work = "vaults/work"
private = "vaults/private"

[capture.templates.task]
description = "Quick inbox task entry"
keys = "t"
kind = "entry"
target = "gtd/inbox.org"
target_heading = "* Inbox"
todo_header = "TODO"
filetags = ["inbox"]

[capture.templates.concept]
description = "Atomic permanent concept note"
keys = "c"
kind = "file"
target = "zettel/${slug}.org"
roam_type = "concept"
filetags = ["zettel", "permanent"]
```

See the [User Manual Quickstart](../user/quickstart.md) and the [Glossary](../glossary.md) for foundational concepts.
