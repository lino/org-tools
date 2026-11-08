<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Getting Things Done (GTD) Workflow Recipe

## Overview

**What you will learn:**

- How to capture unclarified tasks into `inbox.org` and review them with `org query inbox`.
- How to clarify next actions with `@context` tags, wire `org-edna` dependencies, and refile into projects.
- How to run a daily composite agenda (`org query agenda --composite`) and a weekly stuck-project review.

Task lists fall apart when inbox captures, blocked dependencies, and actionable next steps sit in one flat pile.
Getting Things Done (GTD) separates capture from clarification so you only see tasks you can act on right now.
`org` provides native `inbox`, `next`, `waiting`, `blocked`, `stuck`, and `agenda` subcommands without requiring Emacs.

## Prerequisites

- `org` installed on your workstation (see [Five-Minute Quickstart](../user/quickstart.md)).
- A workspace directory containing `inbox.org` and `projects.org` with `.org-tools.toml` configured.

## Steps

1. Create `inbox.org` and `projects.org` or capture new raw tasks using `org update add-todo` and `org roam capture`.
   ```sh
   org update add-todo inbox.org "Draft Q3 infrastructure budget"
   org update add-todo inbox.org "Call hardware vendor for quote"
   ```
1. Inspect uncategorized inbox items (entries with a `TODO` keyword but no tags and no planning dates) using `org query inbox`.
   ```sh
   org query inbox .
   ```
1. Clarify each inbox item by assigning `@context` tags, `SCHEDULED:` dates, or `:EFFORT:` estimates, then refile it into `projects.org`.
   ```sh
   org update set-tags 'inbox.org::*/Draft Q3 infrastructure budget' ':@office:finance:'
   org update set-property 'inbox.org::*/Draft Q3 infrastructure budget' EFFORT 1:30
   org refile 'inbox.org::*/Draft Q3 infrastructure budget' --to 'projects.org::*/Q3 Planning'
   ```
1. Wire an `org-edna` dependency between sequential tasks with `org update dep add` so downstream tasks wait until blockers finish.
   ```sh
   org update add-id --deterministic projects.org
   org update dep add 'projects.org::*/Q3 Planning/Call hardware vendor' \
     'projects.org::*/Q3 Planning/Draft Q3 infrastructure budget'
   ```
1. Run `org query next`, `org query blocked`, `org query stuck`, and `org query agenda --composite` for your daily and weekly reviews.
   ```sh
   org query next --context @office .
   org query blocked .
   org query stuck .
   org query agenda --days 7 --composite .
   ```
1. Complete finished tasks with `org update set-state DONE` (automatically firing `org-edna` `:TRIGGER:` transitions) and archive old items.
   ```sh
   org update set-state DONE 'projects.org::*/Q3 Planning/Call hardware vendor'
   org archive projects.org
   ```

## Expected Outcome

`org query next` lists only unblocked actionable tasks grouped by `@context` tag.
When you mark the blocker task `DONE`, its `:TRIGGER:` automatically promotes the dependent task to `NEXT`:

```text
$ org query next --context @office .
@office:
  projects.org:12: ** NEXT Draft Q3 infrastructure budget  :@office:finance:
$ echo $?
0
```

## Power-User & Scripting Options

Materialize a daily GitHub Flavored Markdown agenda report for Zensical or team dashboards using `--format md -o`.
See [Querying & Agenda Views](../user/querying-and-agenda.md) and the [Glossary](../glossary.md) for predicate details:

```sh
# Generate a composite 7-day Markdown agenda report atomically
org query agenda --days 7 --warning-days 14 --composite --format md -o daily-agenda.md .

# Batch-promote all unblocked TODO items under a specific project to NEXT
org query search 'todo:TODO actionable tags:finance' --format locator . \
  | org update set-state NEXT
```

??? info "Deep Dive: How `stuck` Projects and `org-habit` Repeaters Work"
`org query stuck` flags multi-step project headings that have active child tasks but zero `NEXT` or `WAITING` actions.
Recurring habits marked with `:STYLE: habit` and `.+1d` repeaters automatically record `:LAST_REPEAT:` and render consistency graphs in `org query agenda`.

## Troubleshooting

| Symptom                                          | Cause                                                           | Fix                                                                               |
| ------------------------------------------------ | --------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| Item does not appear in `org query inbox`        | Entry already has a tag, `SCHEDULED:` date, or `DEADLINE:` date | Use `org query search 'todo:TODO'` to inspect entries that already carry metadata |
| Dependent task still appears in `org query next` | `:BLOCKER:` property was not wired or blocker is already `DONE` | Run `org query blocked` and `org query deps` to verify the `:BLOCKER:` link       |
| `org refile` fails to find destination heading   | Target outline path or `id:` locator has a typo                 | Run `org query search '<heading>' --format locator` to copy the exact locator     |
| `org query stuck` exits with code `1`            | Zero stuck projects exist in the workspace                      | Treat exit code `1` on `org query stuck` as a healthy GTD review outcome          |
