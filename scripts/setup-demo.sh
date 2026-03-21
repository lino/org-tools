#!/usr/bin/env bash
# Copyright (C) 2026 org-tools contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Set up a demo environment with sample org files.
# Usage: bash scripts/setup-demo.sh [target_dir]

set -euo pipefail

DEMO_DIR="${1:-/tmp/org-tools-demo}"

echo "Setting up org-tools demo in: $DEMO_DIR"
mkdir -p "$DEMO_DIR"

# --- Work tasks ---
cat > "$DEMO_DIR/work.org" << 'ORG'
#+TITLE: Work Tasks
#+FILETAGS: :work:
#+TODO: TODO NEXT WAITING | DONE CANCELLED
#+TAGS: { @office @remote } meeting code review
#+ARCHIVE: work_archive.org::* Archived

* TODO [#A] Prepare quarterly report :review:
DEADLINE: <2024-06-30 Sun>
:PROPERTIES:
:ID: demo-report-01
:END:

* TODO [#B] Code review for auth module :code:review:
SCHEDULED: <2024-06-16 Sun 10:00>
:LOGBOOK:
CLOCK: [2024-06-15 Sat 14:00]--[2024-06-15 Sat 15:30] =>  1:30
CLOCK: [2024-06-14 Fri 09:00]--[2024-06-14 Fri 10:00] =>  1:00
:END:

** TODO Check test coverage
** TODO Review error handling

* NEXT Sprint planning :meeting:@office:
SCHEDULED: <2024-06-17 Mon 09:00>

* WAITING Design review feedback :review:
:PROPERTIES:
:ID: demo-design-01
:END:

* DONE Set up CI pipeline :code:
CLOSED: [2024-06-10 Mon 16:00]
:LOGBOOK:
CLOCK: [2024-06-10 Mon 10:00]--[2024-06-10 Mon 16:00] =>  6:00
:END:

* DONE Update deployment docs
CLOSED: [2024-06-12 Wed 11:00]

* CANCELLED Old migration task
CLOSED: [2024-06-05 Wed 09:00]
ORG

# --- Personal tasks ---
cat > "$DEMO_DIR/personal.org" << 'ORG'
#+TITLE: Personal
#+FILETAGS: :personal:
#+TAGS: home errands health

* TODO Grocery shopping :errands:
SCHEDULED: <2024-06-16 Sun>

* TODO [#B] Schedule dentist appointment :health:
DEADLINE: <2024-06-20 Thu>

* TODO Fix kitchen light :home:
:LOGBOOK:
CLOCK: [2024-06-15 Sat 10:00]
:END:

* Project: Home renovation [0/3] :home:
** TODO Get paint samples
** TODO Measure bathroom
** DONE Order shelving
CLOSED: [2024-06-11 Tue 14:00]
ORG

# --- Notes ---
cat > "$DEMO_DIR/notes.org" << 'ORG'
#+TITLE: Notes
#+CONSTANTS: pi=3.14159 tax_rate=0.19
#+LINK: wp https://en.wikipedia.org/wiki/

* Reading list

- [[wp:Org-mode][Org-mode on Wikipedia]]
- [[wp:Literate_programming][Literate Programming]]

* Measurements

| Item   | Length | Width | Area          |
|--------+--------+-------+---------------|
| Desk   |    120 |    60 |          7200 |
| Shelf  |     80 |    30 |          2400 |
| Window |    150 |   100 |         15000 |
#+TBLFM: $4=$2*$3
ORG

echo ""
echo "Demo files created:"
echo "  $DEMO_DIR/work.org      — work tasks with clock entries"
echo "  $DEMO_DIR/personal.org  — personal tasks with statistic cookies"
echo "  $DEMO_DIR/notes.org     — notes with tables and links"
echo ""
echo "Try these commands:"
echo "  org fmt check $DEMO_DIR/"
echo "  org query search 'todo:TODO' $DEMO_DIR/"
echo "  org query agenda $DEMO_DIR/ --days 14"
echo "  org clock report $DEMO_DIR/ --group-by tag"
echo "  org clock status $DEMO_DIR/"
echo "  org export ical $DEMO_DIR/"
echo "  org update add-cookie $DEMO_DIR/ --recursive --dry-run"
echo "  org archive $DEMO_DIR/ --dry-run"
