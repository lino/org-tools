<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Zettelkasten & Fleeting Notes Recipe

## Overview

**What you will learn:**

- How to capture raw thoughts into a Luhmann fleeting note queue using `org fleeting capture`.
- How to inspect unrefined notes and transform them into permanent notes or tasks.
- How to create atomic concept notes using `org roam capture` and interactive templates.
- How to explore note networks using `org roam` node resolution, backlinks, and graph exports.

Capturing insights during reading requires minimal friction.
Permanent notes demand careful synthesis, unique identifiers, and bidirectional linking.
`org fleeting` and `org roam` decouple instant thought capture from permanent knowledge synthesis.
You can maintain a disciplined slip-box note network across multiple vaults without running Emacs.

## Prerequisites

- `org` installed on your workstation (see [Five-Minute Quickstart](../user/quickstart.md)).
- An `.org-tools.toml` file configuring `[roam.directories]` and `[capture.templates.<name>]`.

## Steps

1. Configure your roam vaults and capture templates in `.org-tools.toml`.
   ```toml
   [roam.directories]
   work = "vaults/work"

   [capture.templates.concept]
   description = "Atomic permanent concept note"
   keys = "c"
   kind = "file"
   target = "zettel/${slug}.org"
   roam_type = "concept"
   filetags = ["zettel", "permanent"]
   ```
1. Capture a quick Luhmann fleeting note with source medium and location metadata.
   ```sh
   org fleeting capture "Atomic notes should link by ID rather than file paths" \
     --roam-dir work --medium "How to Take Smart Notes" --location "p. 42"
   ```
1. Inspect your pending queue of unrefined fleeting notes with `org fleeting list`.
   ```sh
   org fleeting list --status unrefined --roam-dir work
   ```
1. Transform the fleeting note into a permanent Zettelkasten concept note using `org fleeting to-zettel`.
   ```sh
   org fleeting to-zettel 1 --title "Atomic Zettelkasten Notes" \
     --parent "Slip-Box Method" --roam-dir work
   ```
1. Create new concept notes directly using `org roam capture` with template selection.
   ```sh
   org roam capture --list-templates
   org roam capture "Bidirectional Note Linking" --roam-dir work --parent "Atomic Zettelkasten Notes"
   ```
1. Process remaining notes or explore backlinks and graph connections with `org roam`.
   ```sh
   org fleeting process --roam-dir work
   org roam node "Atomic Zettelkasten Notes" --roam-dir work
   org roam backlinks "Atomic Zettelkasten Notes" --roam-dir work
   org roam graph --format mermaid --roam-dir work
   ```

## Expected Outcome

Transforming a fleeting note creates an atomic `org-roam` v2 file with a top-level `:ID:` drawer.
`org roam backlinks` resolves incoming links via `.cache/org-cache.db` in milliseconds:

```text
$ org roam backlinks "Atomic Zettelkasten Notes" --roam-dir work
[id:c1d2e3f4-5678-90ab-cdef-1234567890ab] Bidirectional Note Linking (vaults/work/concepts/bidirectional-note-linking.org:14)
```

## Power-User & Scripting Options

Convert fleeting notes to tasks using `org fleeting to-task` or discard unneeded entries with `org fleeting discard`.
Export graph structures or JSON envelopes for scripting:

```sh
# Convert a fleeting note into an actionable GTD task
org fleeting to-task 2 --roam-dir work --context @office

# Discard a fleeting note that is no longer needed
org fleeting discard 3 --roam-dir work

# Inspect the full roam knowledge graph as JSON
org roam graph --format json --roam-dir work | jq '.nodes | length'
```

??? info "Deep Dive: Level-0 Drawers, Folgezettel Linking, and Graph Integrity"

    `OrgDocument` treats files as nodes when line 1 begins with a `:PROPERTIES:` drawer holding `:ID:`.
    The `--parent` flag records `:PARENT_ZETTEL:` properties and inserts a Folgezettel link.
    `org fmt check` enforces rules `E009` (`duplicate-id`) and `W037` (`broken-id-link`).
    These checks keep cross-vault knowledge graphs completely consistent over time.

## Troubleshooting

| Symptom                                          | Cause                                                           | Fix                                                                      |
| ------------------------------------------------ | --------------------------------------------------------------- | ------------------------------------------------------------------------ |
| `warning [W037/broken-id-link]` during `org fmt` | An `[[id:<uuid>]]` link points to a deleted node                | Run `org roam node <title>` to find the correct UUID and update the link |
| `error [E010/invalid-id-seed]`                   | Manual edits modified `:ID:` or `:ID_SEED:` out of sync         | Run `org update add-id --deterministic` to recompute the matching UUIDv5 |
| `org roam node` fails to find newly created note | File lacks a `:PROPERTIES:` `:ID:` drawer                       | Run `org update add-id --file <file>` to insert the missing drawer       |
| `org fleeting list` reports no entries           | All captured fleeting notes were already processed or discarded | Run `org fleeting list --status all` to review processed notes           |
