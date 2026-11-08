<!--
Copyright (C) 2026 org-tools contributors
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Updating, Clocking & Exporting

## Overview

**What you will learn:**

- How to mutate TODO states, planning dates, drawers, tags, checklists, `org-edna` dependencies, and `#+TBLFM:` tables (`org update`).
- How to refile subtrees (`org refile`), track time clocks with `:EFFORT:` variance (`org clock`), and archive done work (`org archive`).
- How to extract bounded node slices (`org show`) and export calendars or Zensical Markdown (`org export ical|jscal|md`).

Editing task states, repeating timestamps, `:LOGBOOK:` drawers, and statistic cookies by hand is tedious and error-prone.
`org` performs atomic, lint-clean mutations that preserve `F005` drawer alignment and `F007` column-77 tag alignment.
Every write operation supports `--dry-run` previews and emits a structured `MutationResult` JSON envelope.

## Prerequisites

- `org` installed on your workstation or CI runner (see [Five-Minute Quickstart](quickstart.md)).
- Target `.org` files addressed by file path or universal `OrgLocator` (`id:<uuid>`, `file::#custom-id`, `file::*/Outline`, `file::line`).
- Optional: `--dry-run` flag to preview mutations before writing to disk.

## Steps

1. Mint `:ID:` properties on headings (`UUIDv4` or Deterministic RFC 9562 Universally Unique Identifier Version 5 (`UUIDv5`) with `:ID_SEED:`).
   ```sh
   org update add-id tasks.org
   org update add-id --deterministic --namespace 6ba7b810-9dad-11d1-80b4-00c04fd430c8 tasks.org
   ```
1. Transition TODO states, update planning dates (`SCHEDULED:`, `DEADLINE:`), and modify properties, tags, or checklist items.
   ```sh
   org update set-state DONE 'tasks.org::*/Project Launch/Draft release announcement'
   org update set-scheduled '+3d' 'tasks.org::*/Project Launch/Verify package checksums'
   org update set-property 'tasks.org::*/Project Launch' EFFORT 2:00
   org update set-tags 'tasks.org::*/Project Launch' ':launch:q2:'
   org update check-item 'tasks.org::*/Project Launch' --index 0 --check
   ```
1. Recalculate `[n/m]` / `[n%]` statistic cookies and pure-Rust `#+TBLFM:` spreadsheet formulas (`org update add-cookie`, `org update calc`).
   ```sh
   org update add-cookie --recursive tasks.org
   org update calc budget.org
   ```
1. Clock in and out of tasks (`org clock`), refile subtrees (`org refile`), and archive completed `DONE` subtrees (`org archive`).
   ```sh
   org clock in 'tasks.org::*/Project Launch/Verify package checksums'
   org clock out tasks.org
   org clock report --group-by tag tasks.org
   org refile 'inbox.org::*/New Item' --to 'tasks.org::*/Project Launch'
   org archive tasks.org
   ```
1. Extract bounded node slices (`org show`) or export iCalendar (`.ics`), JSCalendar (`.json`), and Markdown (`org export`).
   ```sh
   org show 'tasks.org::*/Project Launch' --format md --max-chars 1500
   org export ical --output calendar.ics tasks.org
   org export jscal --output calendar.json tasks.org
   org export md --expand-includes --output build/docs/ tasks.org
   ```

## Expected Outcome

Mutating commands update target `.org` files atomically, invalidate `.cache/org-cache.db`, and exit with code `0`.
Passing `--format json` prints a schema-validated `MutationResult` envelope with `telemetry` metrics:

```json
{
  "command": "set-state",
  "files_modified": 1,
  "entries_affected": 1,
  "dry_run": false,
  "details": [
    {
      "locator": "tasks.org::7",
      "action": "TODO -> DONE"
    }
  ],
  "errors": [],
  "telemetry": {
    "duration_us": 412,
    "files_scanned": 1,
    "cache_hits": 1,
    "cache_misses": 0
  }
}
```

## Power-User & Scripting Options

Wire symmetric `org-edna` `:BLOCKER:` and `:TRIGGER:` dependencies with `org update dep add` and preview any mutation with `--dry-run`.
See [Power-User & Scripting](power-user-and-scripting.md) and [JSON Schemas & Contracts](schemas.md) for automation patterns:

```sh
# Wire an acyclic blocker -> trigger dependency between two tasks
org update dep add 'id:11111111-1111-4111-8111-111111111111' 'id:22222222-2222-4222-8222-222222222222'

# Preview archiving completed tasks as JSON without touching disk
org archive --dry-run --format json ~/org/
```

??? info "Deep Dive: Repeaters, Habit Logging & Emacs Calc Fallback"
When `org update set-state DONE` transitions an entry with a repeater (`+1w`, `++1m`, `.+1d`), it resets the state to `TODO`.
It also advances the planning timestamp, updates `:LAST_REPEAT:`, and appends a `:LOGBOOK:` state note.
Pass `--no-repeat` to close a repeating task permanently.
If `org update calc` encounters an Emacs Lisp formula (`'(...)`), it leaves the file untouched and exits with code `3` (`EXIT_REQUIRES_EMACS`).

## Troubleshooting

| Symptom                                                   | Cause                                                          | Fix                                                                         |
| --------------------------------------------------------- | -------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `org update calc` exits with code `3`                     | Table `#+TBLFM:` uses Emacs Lisp or unsupported Calc functions | Simplify the formula to standard arithmetic/aggregates or run batch Emacs   |
| Repeating task resets to `TODO` instead of staying `DONE` | Entry has a `+`, `++`, or `.+` repeater on its timestamp       | Pass `--no-repeat` to `org update set-state DONE` to close it permanently   |
| `org update dep add` rejects dependency with cycle error  | Adding the link would create a circular `:BLOCKER:` chain      | Run `org query deps` to inspect the graph and break the dependency loop     |
| `locator not found` error on `id:<uuid>`                  | Target UUID lies outside the current directory                 | Pass `--workspace <DIR>` so `org` resolves `:ID:` across the full workspace |
