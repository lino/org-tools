<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Zettelkasten & Fleeting Notes Recipe (`org roam`)

## Overview

**What you will learn:**

- How to capture quick fleeting thoughts and promote them into permanent `org-roam` v2 Zettelkasten notes.
- How to mint Deterministic RFC 9562 Universally Unique Identifier Version 5 (`UUIDv5`) node IDs with `:ID_SEED:` provenance.
- How to resolve nodes by title or `:ROAM_ALIASES:`, inspect bidirectional backlinks, and export Mermaid link graphs.

Building a durable Zettelkasten ("slip-box") knowledge graph requires frictionless capture and reliable cross-note links.
`org roam` indexes level-0 file `:PROPERTIES:` drawers, heading nodes, aliases, references, and `[[id:...]]` backlinks in pure Rust.
You can manage thousands of interconnected permanent notes without running an Emacs daemon.

## Prerequisites

- `org` installed on your workstation (see [Five-Minute Quickstart](../user/quickstart.md)).
- An `.org-tools.toml` file configuring `[roam]` (`roam_dir`, `fleeting_file`, `daily_dir`).

## Steps

1. Configure your `[roam]` vault paths and capture templates in `.org-tools.toml`.
   ```toml
   [roam]
   roam_dir = "roam"
   fleeting_file = "inbox.org"
   daily_dir = "daily"
   ```
1. Capture a fleeting thought into `inbox.org` using `org roam fleeting capture`, then list open fleeting notes.
   ```sh
   org roam fleeting capture "Zettelkasten notes should stay atomic and link by ID" --tags zettelkasten
   org roam fleeting list
   ```
1. Promote a fleeting note into a permanent `org-roam` v2 file node inside `roam/` with `org roam fleeting promote`.
   ```sh
   org roam fleeting promote 'inbox.org::1' --title "Atomic Zettelkasten Notes" --tags permanent
   ```
1. Mint Deterministic RFC 9562 UUIDv5 `:ID:` and `:ID_SEED:` properties across your vault and verify link integrity with `org fmt check`.
   ```sh
   org update add-id --deterministic roam/
   org fmt check roam/
   ```
1. Query nodes by title or `:ROAM_ALIASES:`, inspect incoming `[[id:...]]` backlinks, and render the knowledge graph as Mermaid.
   ```sh
   org roam node "Atomic Zettelkasten Notes" --workspace .
   org roam backlinks "Atomic Zettelkasten Notes" --workspace .
   org roam links "Atomic Zettelkasten Notes" --workspace .
   org roam graph --format mermaid --workspace .
   ```

## Expected Outcome

Promoting a fleeting note creates a level-0 `org-roam` v2 file carrying a top-of-file `:PROPERTIES:` `:ID:` drawer and `#+TITLE:`.
`org roam node` and `org roam backlinks` resolve the node in milliseconds via `.cache/org-cache.db` and exit with code `0`:

```text
$ org roam node "Atomic Zettelkasten Notes" --workspace .
id:9b2d4f10-8c3a-51e2-a411-778899aabbcc  Atomic Zettelkasten Notes (roam/atomic-zettelkasten-notes.org)
$ echo $?
0
```

## Power-User & Scripting Options

Export interconnected `org-roam` notes to Zensical GitHub Flavored Markdown with automatic cross-node `[[id:...]]` link resolution.
See [Updating, Clocking & Exporting](../user/updating-clocking-and-exporting.md) and the [Glossary](../glossary.md) for export details:

```sh
# Export all permanent roam notes to Markdown while resolving [[id:...]] links
org export md --workspace . --expand-includes --output build/roam-md/ roam/

# Inspect the full roam graph as schema-validated JSON
org roam graph --format json --workspace . | jq '.nodes | length'
```

??? info "Deep Dive: Level-0 File Drawers, `:ROAM_ALIASES:`, and `W037` Validation"
In `org-roam` v2, a file becomes a node when line 1 starts with a `:PROPERTIES:` drawer containing `:ID:`.
`OrgDocument` also parses `:ROAM_ALIASES:` (shell-quoted alias tokens), `:ROAM_REFS:`, and `:ROAM_EXCLUDE:`.
Lint rules `E009` (`duplicate-id`), `E010` (`invalid-id-seed`), and `W037` (`broken-id-link`) verify graph integrity on every `org fmt check` run.

## Troubleshooting

| Symptom                                            | Cause                                                           | Fix                                                                               |
| -------------------------------------------------- | --------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| `warning [W037/broken-id-link]` during `org fmt`   | An `[[id:<uuid>]]` link points to a missing or deleted `:ID:`   | Run `org roam node <title>` to find the target UUID and update the link           |
| `error [E010/invalid-id-seed]`                     | Manual edit changed `:ID:` or `:ID_SEED:` out of sync           | Re-run `org update add-id --deterministic` to recompute the matching UUIDv5       |
| `org roam node` does not find a newly created file | File lacks a `:PROPERTIES:` `:ID:` drawer or lies outside vault | Run `org update add-id --deterministic <file>` and pass `--workspace <DIR>`       |
| Alias lookup fails on multi-word `:ROAM_ALIASES:`  | Multi-word alias was not wrapped in double quotes               | Quote multi-word aliases in the drawer (for example, `:ROAM_ALIASES: "Slip Box"`) |
