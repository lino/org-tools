<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Getting Things Done (GTD) Workflow Recipe

## Overview

**What you will learn:**

- How to isolate `work` and `private` vaults using multi-vault `[roam.directories]` configuration.
- How to capture raw inbox items and scaffold structured project nodes.
- How to pick your next action with `org next` using context, effort, and project filters.
- How to conduct a comprehensive weekly review covering stuck projects, dependencies, clocks, and agenda tags.

Task lists quickly become chaotic when captures, blocked tasks, and active next steps share a flat list.
Getting Things Done (GTD) separates capture from clarification.
It ensures you focus solely on actionable tasks that match your current context and available time.
`org` provides dedicated GTD commands for multi-vault navigation without requiring an Emacs runtime.

## Prerequisites

- `org` installed on your workstation (see [Five-Minute Quickstart](../user/quickstart.md)).
- An `.org-tools.toml` file at your workspace root configuring multi-vault directories and TODO keywords.

## Steps

1. Configure multi-vault directories and custom TODO states in `.org-tools.toml`.
   ```toml
   [todo]
   sequences = ["TODO NEXT WAITING | DONE CANCELLED"]
   todo = ["TODO", "NEXT", "WAITING"]
   done = ["DONE", "CANCELLED"]

   [roam.directories]
   work = "vaults/work"
   private = "vaults/private"
   ```
1. Capture raw thoughts into your work inbox using `org roam gtd inbox`.
   ```sh
   org roam gtd inbox "Draft Q3 budget" --roam-dir work --context @office --effort 0:30
   org roam gtd inbox "Call hardware vendor for quote" --roam-dir work --context @phone --effort 0:15
   ```
1. Scaffold a project node with `org roam gtd project` and refile clarified tasks into it.
   ```sh
   org roam gtd project "Q3 Infrastructure" --roam-dir work
   org refile 'vaults/work/gtd/inbox.org::*/Draft Q3 budget' \
     --to 'vaults/work/gtd/projects/q3-infrastructure.org::* Tasks'
   ```
1. Wire sequential task dependencies with `org update dep add` so downstream actions stay blocked until prerequisites finish.
   ```sh
   org update add-id --deterministic vaults/work/gtd/projects/q3-infrastructure.org
   org update dep add 'vaults/work/gtd/projects/q3-infrastructure.org::*/Call hardware vendor' \
     'vaults/work/gtd/projects/q3-infrastructure.org::*/Draft Q3 budget'
   ```
1. Find your immediate next action using `org next` with project, context, and effort filters.
   ```sh
   org next --roam-dir work --project "Q3 Infrastructure" --context @office --max-effort 1:00 --pick
   ```
1. Run your weekly review across inbox, waiting, blocked, stuck projects, clocks, archives, and agenda tags.
   ```sh
   org query inbox --roam-dir work
   org query waiting --roam-dir work
   org query blocked --roam-dir work
   org query stuck --roam-dir work
   org clock report --from -7d vaults/work/
   org archive vaults/work/gtd/projects/
   org roam sync-agenda-tags --roam-dir work
   ```

## Expected Outcome

`org next` selects the single highest-priority unblocked task matching your constraints:

```text
$ org next --roam-dir work --project "Q3 Infrastructure" --context @office --max-effort 1:00 --pick
@office:
  vaults/work/gtd/projects/q3-infrastructure.org:18: ** NEXT Draft Q3 budget  :@office:finance:
```

## Power-User & Scripting Options

Generate composite Markdown agenda digests for daily planning or terminal status widgets.
Export structured JSON envelopes with `--format json` to drive custom status bars or notification hooks:

```sh
# Generate a composite 7-day Markdown agenda report atomically
org query agenda --days 7 --warning-days 14 --composite --format md -o daily-agenda.md vaults/work/

# Recommend the top next action as a machine-readable locator
org next --roam-dir work --pick --format locator
```

??? info "Deep Dive: Multi-Vault Isolation, Custom TODO States, and Stuck Projects"

    `[roam.directories]` separates distinct life domains such as work and private life.
    This separation prevents professional tasks from cluttering personal contexts during focused queries.
    Files can define additive custom `#+TODO:`, `#+SEQ_TODO:`, or `#+TYP_TODO:` states.
    `org query stuck` identifies project outlines lacking any active `NEXT` or `WAITING` children.

## Troubleshooting

| Symptom                                        | Cause                                                           | Fix                                                                       |
| ---------------------------------------------- | --------------------------------------------------------------- | ------------------------------------------------------------------------- |
| Item does not appear in `org query inbox`      | Entry already has tags or planning dates                        | Use `org query search 'todo:TODO'` to find entries with existing metadata |
| Dependent task still appears in `org next`     | `:BLOCKER:` property was not wired or blocker is already `DONE` | Run `org query blocked` and `org query deps` to verify the blocker link   |
| `org refile` fails to find destination heading | Target outline path or `id:` locator has a typo                 | Run `org query search '<heading>' --format locator` to obtain the locator |
| `org query stuck` exits with code `1`          | Zero stuck projects exist in the scanned directory              | Treat exit code `1` on `org query stuck` as a healthy review outcome      |
