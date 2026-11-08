<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# `org-tools` Cookbook

## Overview

**What you will learn:**

- Which goal-oriented recipe matches your daily task, knowledge-graph, AI assistant, or Continuous Integration (CI) workflow.
- How to combine `org` subcommands (`query`, `roam`, `update`, `refile`, `mcp`, `fmt`) to solve real-world problems.
- How to run multi-vault setups separating Getting Things Done (GTD) tasks from Zettelkasten permanent notes.

Reference manuals explain individual flags, while cookbooks show how commands fit together in daily practice.
Each recipe in this section walks through a complete end-to-end workflow with copy-pasteable commands and expected outputs.
Pick the recipe that matches your immediate goal below.

## Recipe Catalog

| Cookbook Recipe                                         | Primary Goal & Problem Solved                                                                                    | Key Subcommands Used                                                   |
| ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| [Getting Things Done (GTD)](gtd.md)                     | Capture inbox items, clarify next actions by `@context`, wire `org-edna` blockers, and run weekly reviews.       | `org query inbox\|next\|waiting\|blocked\|stuck\|agenda`, `org refile` |
| [Zettelkasten & Fleeting Notes](zettelkasten.md)        | Capture fleeting thoughts, promote notes into `org-roam` v2 nodes, query backlinks, and render knowledge graphs. | `org roam fleeting\|capture\|node\|backlinks\|graph`, `org update`     |
| [AI Assistants & MCP Server](mcp-server.md)             | Connect Claude Code, Cursor, or custom AI hosts to a workspace-confined `stdio` Model Context Protocol server.   | `org mcp`, `org cache`, `org show`                                     |
| [CI Pipelines & Shell Automation](automation-and-ci.md) | Enforce formatting and lint gates in GitHub Actions, pre-commit hooks, and `jq`-powered shell scripts.           | `org fmt`, `org schema`, `org query`, `org update`                     |

## Multi-Vault Workspace Pattern

Many users keep fast-moving GTD task files separate from long-term Zettelkasten (`org-roam`) knowledge notes.
Configure `.org-tools.toml` at your workspace root so every recipe targets the right directory automatically:

```toml
[roam]
roam_dir = "roam"
fleeting_file = "inbox.org"
daily_dir = "daily"

[capture.templates.task]
target = "gtd/inbox.org"
heading = "Inbox"
todo = "TODO"
tags = ["inbox"]

[capture.templates.note]
target = "roam/inbox.org"
heading = "Fleeting Notes"
tags = ["fleeting"]
```

See the [User Manual Quickstart](../user/quickstart.md) and the [Glossary](../glossary.md) for foundational concepts.
