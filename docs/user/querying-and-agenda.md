<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Querying, GTD Views & Agendas (`org query`)

## Overview

**What you will learn:**

- How to search headings and body text using composable boolean predicates (`org query search`).
- How to inspect Getting Things Done (GTD) views (`inbox`, `next`, `waiting`, `blocked`, `stuck`) and dependency graphs (`deps`).
- How to project multi-day agenda timelines (`org query agenda`) and evaluate `#+COLUMNS:` summary tables (`org query columns`).

Finding actionable tasks across dozens of `.org` files is difficult without structured search and date projection.
`org query` combines boolean predicates, tag inheritance, and an incremental SQLite Full-Text Search Version 5 (`SQLite FTS5`) cache.
You can inspect daily agendas, `org-habit` consistency streaks, and property rollups from any terminal or script.

## Prerequisites

- `org` installed on your system (see [Five-Minute Quickstart](quickstart.md)).
- An Org workspace containing `.org` files with TODO keywords, tags, or planning timestamps (`SCHEDULED:`, `DEADLINE:`).
- Optional: `[agenda]` and `[todo]` settings in `.org-tools.toml`.

## Steps

1. Run `org query search` with key-value predicates (`todo:`, `tags:`, `scheduled:`, `deadline:`, `priority:`, `body:`) to find matching headings.
   ```sh
   org query search 'todo:TODO tags:work' ~/org/
   org query search '(todo:TODO or todo:NEXT) and deadline:<=+7d' ~/org/
   org query search 'body:"telemetry"' ~/org/
   ```
1. Inspect dedicated GTD workflow views using `inbox`, `next`, `waiting`, `blocked`, and `stuck`.
   ```sh
   org query inbox ~/org/
   org query next --context @office ~/org/
   org query waiting ~/org/
   org query blocked ~/org/
   org query stuck ~/org/
   ```
1. Project a multi-day agenda timeline with `org query agenda`, including overdue items, deadline warnings, repeaters, and `org-habit` graphs.
   ```sh
   org query agenda --from 2026-04-06 --days 7 --warning-days 14 ~/org/
   org query agenda --composite --format md -o agenda.md ~/org/
   ```
1. Evaluate `#+COLUMNS:` property, effort (`:EFFORT:`), and clocked-time rollups across a file or subtree with `org query columns`.
   ```sh
   org query columns ~/org/projects.org
   org query columns --columns '%40ITEM %TODO %EFFORT{:} %CLOCKSUM' ~/org/projects.org
   ```
1. Render `org-edna` `:BLOCKER:` and `:TRIGGER:` task dependencies as Mermaid, Graphviz DOT, PlantUML, or JSON with `org query deps`.
   ```sh
   org query deps --format mermaid ~/org/projects/
   ```

## Expected Outcome

`org query` commands print matching entries, agenda timelines, or column tables and exit with code `0`.
If a search or GTD view matches zero entries, `org query` exits with code `1` (`EXIT_ISSUES`) for clean shell branching:

```text
$ org query agenda --from 2026-04-06 --days 2 tasks.org
Agenda (2026-04-06 .. 2026-04-07)
2026-04-06 Mon
  Scheduled:  TODO Draft release announcement  :@writing:work:
  Deadline:   TODO Verify package checksums (in 2 d.)  :@ops:work:
```

## Power-User & Scripting Options

Emit `--format locator` to pipe matched entries into `org update`, `org show`, or `org refile`, or emit `--format json` for `jq`.
See the [Getting Things Done (GTD) Cookbook](../cookbook/gtd.md) and [JSON Schemas & Contracts](schemas.md) for output schemas:

```sh
# Pipe overdue TODO locators directly into set-scheduled to reschedule them to today
org query search 'todo:TODO scheduled:past' --format locator ~/org/ \
  | org update set-scheduled today

# Check or warm the incremental SQLite WAL + FTS5 cache (.cache/org-cache.db)
org cache status ~/org/
```

??? info "Deep Dive: Query Predicate Reference & Tag Inheritance"

    Child headings automatically inherit tags from parent headings and `#+FILETAGS:` directives during queries.
    Supported predicates include `todo`, `done`, `tags`, `heading`, `body`, `property:K=V`, `priority`, `level`, `scheduled`, `deadline`, `closed`, `clocked`, `blocked`, `actionable`, and `waiting`.

## Troubleshooting

| Symptom                                              | Cause                                                           | Fix                                                                           |
| ---------------------------------------------------- | --------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `org query search` exits with code `1` and no output | Zero entries matched the boolean query expression               | Test individual predicates or check custom TODO keywords in `.org-tools.toml` |
| Custom TODO state (such as `REVIEW`) is not matched  | Keyword is not listed under `[todo].active_keywords`            | Add `active_keywords = ["TODO", "NEXT", "REVIEW"]` to `.org-tools.toml`       |
| `body:` search misses recent edits on network mounts | Coarse filesystem timestamp prevented cache invalidation        | Run `org cache rebuild` or pass `--no-cache` to scan files directly from disk |
| `org query columns` shows empty summary values       | No `#+COLUMNS:` line exists in the file and `--columns` omitted | Add `#+COLUMNS:` to the `.org` file or pass `--columns '<SPEC>'` on the CLI   |
