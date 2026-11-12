// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural integration tests for `ORGFMT-WP-22`:
//! 1. `gtd_roam::sync_roam_agenda_filetag`, `org roam sync-agenda-tags`, and automatic
//!    `:agenda:` upkeep in `org update set-state` and `org refile`.
//! 2. `org query next` and top-level `org next` ("what to do next" navigator, project
//!    resolution, leaf actionability, multi-dimensional GTD filters, urgency ranking,
//!    `--group-by`, and `--pick`).
//! 3. `org roam gtd inbox|project`, roam-aware GTD queries, roam-node `--to` resolution
//!    in `org refile`, and format-agnostic `stdin` piping without `--format locator`.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

use org_tools_core::gtd_roam::sync_roam_agenda_filetag;

// region Helpers

/// Constructs an `org` CLI command builder.
fn org() -> Command {
    Command::cargo_bin("org").expect("org binary should build")
}

/// Writes a multi-vault `.org-tools.toml` with `work` and `private` roam directories.
fn write_roam_workspace_config(root: &Path, work_dir: &Path, private_dir: &Path) {
    fs::create_dir_all(work_dir).expect("create work vault");
    fs::create_dir_all(private_dir).expect("create private vault");
    let cfg = format!(
        r#"[roam]
default_directory = "work"
gtd_subdir = "gtd"
zettel_subdir = "zettel"
fleeting_subdir = "fleeting"
daily_subdir = "daily"

[roam.directories]
work = "{}"
private = "{}"
"#,
        work_dir.display(),
        private_dir.display()
    );
    fs::write(root.join(".org-tools.toml"), cfg).expect("write workspace config");
}

// endregion

// region Slice 1: Dynamic :agenda: Filetag Synchronization

/// Verifies `sync_roam_agenda_filetag` adds `:agenda:` when active TODOs exist, preserves it on base GTD files, and removes it when all TODOs are done.
#[test]
fn slice1_unit_sync_roam_agenda_filetag_adds_preserves_and_removes() {
    let vault = Path::new("/tmp/roam");
    let zettel_path = vault.join("zettel/topic.org");
    let active_org = ":PROPERTIES:\n:ID:       11111111-1111-4111-8111-111111111111\n:END:\n#+title: Topic Note\n#+filetags: :concept:\n\n* TODO Follow up on citation\n";
    let added = sync_roam_agenda_filetag(&zettel_path, active_org, "gtd", Some(vault))
        .expect("should add :agenda: tag");
    assert!(added.contains("#+filetags: :concept:agenda:"));

    let done_org = added.replace("* TODO Follow up", "* DONE Follow up");
    let removed = sync_roam_agenda_filetag(&zettel_path, &done_org, "gtd", Some(vault))
        .expect("should remove :agenda: tag when all TODOs are DONE");
    assert!(removed.contains("#+filetags: :concept:"));
    assert!(!removed.contains(":agenda:"));

    let inbox_path = vault.join("gtd/inbox.org");
    let empty_inbox = "#+TITLE: Inbox\n";
    let inbox_synced = sync_roam_agenda_filetag(&inbox_path, empty_inbox, "gtd", Some(vault))
        .expect("base GTD inbox.org should always receive :agenda:");
    assert!(inbox_synced.contains(":agenda:"));
    assert!(
        sync_roam_agenda_filetag(&inbox_path, &inbox_synced, "gtd", Some(vault)).is_none(),
        "already synced base GTD file should be a no-op"
    );
}

/// Verifies `org roam sync-agenda-tags` and automatic `:agenda:` upkeep in `set-state` and `refile`.
#[test]
fn slice1_cli_sync_agenda_tags_and_automatic_upkeep_on_set_state_and_refile() {
    let tmp = TempDir::new().expect("tempdir");
    let work = tmp.path().join("work");
    let priv_dir = tmp.path().join("private");
    write_roam_workspace_config(tmp.path(), &work, &priv_dir);

    let note_a = work.join("note_a.org");
    let note_b = work.join("note_b.org");
    fs::write(
        &note_a,
        ":PROPERTIES:\n:ID:       aaaaaaaa-1111-4111-8111-111111111111\n:END:\n#+title: Note A\n#+filetags: :note:\n\n* TODO Action in A\n:PROPERTIES:\n:ID:       aaaaaaaa-2222-4222-8222-222222222222\n:END:\n",
    )
    .expect("write note A");
    fs::write(
        &note_b,
        ":PROPERTIES:\n:ID:       bbbbbbbb-1111-4111-8111-111111111111\n:END:\n#+title: Note B\n#+filetags: :note:agenda:\n\n* DONE Completed in B\n",
    )
    .expect("write note B");

    let out = org()
        .current_dir(tmp.path())
        .args(["roam", "sync-agenda-tags", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&out).expect("valid JSON");
    assert_eq!(json["command"], "sync-agenda-tags");
    assert_eq!(json["files_modified"], 2);
    assert!(fs::read_to_string(&note_a)
        .expect("read A")
        .contains(":agenda:"));
    assert!(!fs::read_to_string(&note_b)
        .expect("read B")
        .contains(":agenda:"));

    // Refile active TODO from Note A to Note B -> Note A loses :agenda:, Note B gains :agenda:.
    org()
        .current_dir(tmp.path())
        .args([
            "refile",
            "id:aaaaaaaa-2222-4222-8222-222222222222",
            "--to",
            "Note B",
        ])
        .assert()
        .success();
    assert!(!fs::read_to_string(&note_a)
        .expect("read A after refile")
        .contains(":agenda:"));
    assert!(fs::read_to_string(&note_b)
        .expect("read B after refile")
        .contains(":agenda:"));

    // Complete the refiled task in Note B via `org update set-state DONE` -> Note B loses :agenda:.
    org()
        .current_dir(tmp.path())
        .args([
            "update",
            "set-state",
            "DONE",
            "id:aaaaaaaa-2222-4222-8222-222222222222",
        ])
        .assert()
        .success();
    assert!(!fs::read_to_string(&note_b)
        .expect("read B after set-state DONE")
        .contains(":agenda:"));
}

// endregion

// region Slice 2: "What to Do Next" Navigator (`org [query] next`)

/// Verifies `org next` / `org query next` project resolution, leaf actionability, filters, ranking, `--group-by`, and `--pick`.
#[test]
fn slice2_org_next_navigator_filters_ranking_grouping_and_pick() {
    let tmp = TempDir::new().expect("tempdir");
    let work = tmp.path().join("work");
    let priv_dir = tmp.path().join("private");
    write_roam_workspace_config(tmp.path(), &work, &priv_dir);

    fs::create_dir_all(work.join("gtd/projects")).expect("create work projects dir");
    fs::write(
        work.join("gtd/projects/apollo.org"),
        concat!(
            ":PROPERTIES:\n",
            ":ID:       10000000-0000-4000-8000-000000000001\n",
            ":TYPE:     project\n",
            ":ROAM_ALIASES: \"Apollo Launch\"\n",
            ":END:\n",
            "#+title: Project Apollo\n",
            "#+filetags: :project:agenda:\n\n",
            "* Tasks\n",
            "** NEXT [#A] Calibrate guidance computer :@computer:telemetry:\n",
            "DEADLINE: <2020-01-10 Fri>\n",
            ":PROPERTIES:\n",
            ":ID:       10000000-0000-4000-8000-000000000002\n",
            ":EFFORT:   0:30\n",
            ":ENERGY:   high\n",
            ":WORKMODE: deep\n",
            ":GOAL:     Q1-Launch\n",
            ":END:\n",
            "** TODO [#B] Draft telemetry checklist\n",
            ":PROPERTIES:\n",
            ":ID:       10000000-0000-4000-8000-000000000003\n",
            ":CONTEXT:  computer\n",
            ":EFFORT:   0:15\n",
            ":ENERGY:   low\n",
            ":WORKMODE: shallow\n",
            ":END:\n",
            "** WAIT Vendor hardware signoff\n",
            "** TODO Future orbital simulation\n",
            "SCHEDULED: <2099-12-31 Thu>\n",
        ),
    )
    .expect("write apollo.org");

    fs::write(
        priv_dir.join("home.org"),
        concat!(
            ":PROPERTIES:\n",
            ":ID:       20000000-0000-4000-8000-000000000001\n",
            ":END:\n",
            "#+title: Home Improvements\n",
            "#+filetags: :agenda:\n\n",
            "* PROJECT Kitchen Renovation :project:\n",
            ":PROPERTIES:\n",
            ":ID:       20000000-0000-4000-8000-000000000002\n",
            ":END:\n",
            "** TODO [#A] Measure cabinet dimensions :@home:\n",
            ":PROPERTIES:\n",
            ":ID:       20000000-0000-4000-8000-000000000003\n",
            ":EFFORT:   0:20\n",
            ":ENERGY:   medium\n",
            ":WORKMODE: shallow\n",
            ":END:\n",
        ),
    )
    .expect("write home.org");

    // 1. Top-level `org next --format json` excludes container project, WAIT, and future-scheduled tasks,
    //    and ranks NEXT [#A] overdue task first.
    let out = org()
        .current_dir(tmp.path())
        .args(["next", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let items: Vec<Value> = serde_json::from_slice(&out).expect("parse next json");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["title"], "Calibrate guidance computer");
    assert_eq!(items[0]["project"], "Project Apollo");
    assert_eq!(items[0]["roam_dir"], "work");

    // 2. Filter by `--project "Apollo Launch"` (alias), `--context computer` (matches both @computer tag and :CONTEXT: property),
    //    `--max-effort 0:20`, `--energy low`, `--workmode shallow`.
    let filtered = org()
        .current_dir(tmp.path())
        .args([
            "query",
            "next",
            "--project",
            "Apollo Launch",
            "--context",
            "computer",
            "--max-effort",
            "0:20",
            "--energy",
            "low",
            "--workmode",
            "shallow",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let f_items: Vec<Value> = serde_json::from_slice(&filtered).expect("parse filtered json");
    assert_eq!(f_items.len(), 1);
    assert_eq!(f_items[0]["title"], "Draft telemetry checklist");

    // 3. `--group-by project` and `--pick` recommendation output.
    org()
        .current_dir(tmp.path())
        .args(["next", "--group-by", "project"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Project Apollo (2)"))
        .stdout(predicate::str::contains("Kitchen Renovation (1)"));

    org()
        .current_dir(tmp.path())
        .args(["next", "--roam-dir", "work", "--pick"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Calibrate guidance computer"))
        .stdout(predicate::str::contains("Project Apollo"))
        .stdout(predicate::str::contains(
            "Locator:   id:10000000-0000-4000-8000-000000000002",
        ));
}

// endregion

// region Slice 3: Roam GTD Capture, Roam Refile & Format-Agnostic Piping

/// Verifies `org roam gtd inbox|project`, roam-aware queries, roam-node refile, and format-agnostic stdin piping.
#[test]
fn slice3_roam_gtd_commands_roam_refile_and_format_agnostic_piping() {
    let tmp = TempDir::new().expect("tempdir");
    let work = tmp.path().join("work");
    let priv_dir = tmp.path().join("private");
    write_roam_workspace_config(tmp.path(), &work, &priv_dir);

    // 1. `org roam gtd inbox` creates `<work>/gtd/inbox.org` with `#+filetags: :agenda:` and `* TODO`.
    org()
        .current_dir(tmp.path())
        .args([
            "roam",
            "gtd",
            "inbox",
            "Review security audit",
            "--context",
            "computer",
            "--effort",
            "0:45",
        ])
        .assert()
        .success();
    let inbox_file = work.join("gtd/inbox.org");
    assert!(inbox_file.is_file());
    let inbox_text = fs::read_to_string(&inbox_file).expect("read inbox");
    assert!(inbox_text.contains(":agenda:"));
    assert!(inbox_text.contains("* TODO Review security audit"));
    assert!(inbox_text.contains(":CAPTURED_AT:"));

    // 2. `org roam gtd project` creates `<work>/gtd/projects/zero-trust-rollout.org`.
    org()
        .current_dir(tmp.path())
        .args([
            "roam",
            "gtd",
            "project",
            "Zero Trust Rollout",
            "--category",
            "security",
            "--first-task",
            "Inventory service identities",
        ])
        .assert()
        .success();
    let proj_file = work.join("gtd/projects/zero-trust-rollout.org");
    assert!(proj_file.is_file());
    let proj_text = fs::read_to_string(&proj_file).expect("read project");
    assert!(proj_text.contains(":TYPE:     project"));
    assert!(proj_text.contains(":STATUS:   active"));
    assert!(proj_text.contains("#+filetags: :project:agenda:"));
    assert!(proj_text.contains("** NEXT Inventory service identities"));

    // 3. Pipe default human output of `org query search` directly into `org refile --to "Zero Trust Rollout"` (roam node title!).
    let search_out = org()
        .current_dir(tmp.path())
        .args([
            "query",
            "search",
            "heading:security",
            work.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    org()
        .current_dir(tmp.path())
        .args(["refile", "--to", "Zero Trust Rollout"])
        .write_stdin(search_out)
        .assert()
        .success();
    assert!(fs::read_to_string(&proj_file)
        .expect("read project after refile")
        .contains("Review security audit"));

    // 4. Pipe `org next --pick` (human recommendation output) directly into `org clock in` and `org show`.
    let pick_out = org()
        .current_dir(tmp.path())
        .args(["next", "--project", "Zero Trust Rollout", "--pick"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    org()
        .current_dir(tmp.path())
        .args(["clock", "in", work.to_str().unwrap()])
        .write_stdin(pick_out.clone())
        .assert()
        .success();
    org()
        .current_dir(tmp.path())
        .args(["show"])
        .write_stdin(pick_out)
        .assert()
        .success()
        .stdout(predicate::str::contains("Inventory service identities"));

    // 5. Pipe default human output of `org query search` directly into `org update set-state DONE` without `--format locator`.
    let search_done = org()
        .current_dir(tmp.path())
        .args(["query", "search", "todo:TODO", work.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    org()
        .current_dir(tmp.path())
        .args(["update", "set-state", "DONE"])
        .write_stdin(search_done)
        .assert()
        .success();
    assert!(fs::read_to_string(&proj_file)
        .expect("read project after piped set-state")
        .contains("DONE Review security audit"));
}

// endregion
