// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=800 reason="Cohesive end-to-end CLI behavioural suite covering all 3 slices of ORGFMT-WP-03"

//! Behavioural CLI integration tests for `ORGFMT-WP-03`: org-edna multi-finder
//! scoping, condition negation, cross-file `:TRIGGER:` cascades, `org update dep`,
//! and static `E008` / `W036` lint rules.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::Command;
use org_tools_core::files::join_slash_rel;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

// region Constants & Path Helpers

/// Builds an [`assert_cmd::Command`] targeting the compiled `org` binary.
fn org() -> Command {
    Command::from_std(StdCommand::new(env!("CARGO_BIN_EXE_org")))
}

/// Runs `org` expecting exit code 0 and parses stdout as JSON.
fn run_org_json(args: &[&str]) -> Value {
    let out = org()
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).expect("stdout must be valid JSON")
}

/// Runs `org` expecting exit code 1 (`EXIT_ISSUES`) and parses stdout as JSON.
fn run_org_issues_json(args: &[&str]) -> Value {
    let out = org()
        .args(args)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).expect("diagnostic stdout must be valid JSON")
}

/// Asserts that `val` satisfies `org-mutation-output.schema.json`.
fn assert_valid_mutation_json(val: &Value, expected_cmd: &str, expected_dry_run: bool) {
    assert_eq!(val["command"].as_str(), Some(expected_cmd));
    assert_eq!(val["dry_run"].as_bool(), Some(expected_dry_run));
    assert!(val["files_modified"].is_u64());
    assert!(val["entries_affected"].is_u64());
    assert!(val["errors"].is_array());
    let details = val["details"].as_array().expect("details must be an array");
    for item in details {
        assert!(item["file"].is_string());
        assert!(item["line"].as_u64().unwrap_or(0) >= 1);
        assert!(item["action"].is_string());
        assert!(item["detail"].is_string() || item["detail"].is_null());
    }
    let telemetry = val["telemetry"]
        .as_object()
        .expect("telemetry object must be present");
    assert!(telemetry["duration_us"].is_u64());
    assert!(telemetry["files_scanned"].is_u64());
}

/// Writes `content` to `dir/rel_path`, creating parent directories automatically.
fn write_org(dir: &Path, rel_path: &str, content: &str) -> std::path::PathBuf {
    let path = join_slash_rel(dir, rel_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(&path, content).expect("write org fixture");
    path
}

/// Extracts blocked entry titles from `org query blocked --format json` output.
fn blocked_titles(json: &Value) -> Vec<&str> {
    json.as_array()
        .expect("blocked output must be a JSON array")
        .iter()
        .filter_map(|i| i["title"].as_str())
        .collect()
}

// endregion

// region Slice 1: Multi-Finder Scoping, Negation & Finders

/// Verifies consecutive finders union into one scope and reset after an explicit condition.
#[test]
fn slice1_consecutive_finders_union_scope_in_blocker() {
    let dir = TempDir::new().unwrap();
    let file = write_org(
        dir.path(),
        "union.org",
        concat!(
            "* TODO Task A\n:PROPERTIES:\n:ID: id-a\n:END:\n",
            "* DONE Task B\n:PROPERTIES:\n:ID: id-b\n:END:\n",
            "* TODO Combined Target\n:PROPERTIES:\n:ID: id-c\n:BLOCKER: ids(\"id-a\") ids(\"id-b\")\n:END:\n",
            "* WAITING Scoped A\n:PROPERTIES:\n:ID: sc-a\n:END:\n",
            "* TODO Scoped B\n:PROPERTIES:\n:ID: sc-b\n:END:\n",
            "* TODO Multi Scope Target\n:PROPERTIES:\n:BLOCKER: ids(\"sc-a\") todo-state?(\"WAITING\") ids(\"sc-b\") todo-state?(\"DONE\")\n:END:\n",
        ),
    );

    let json = run_org_json(&[
        "query",
        "blocked",
        "--format",
        "json",
        file.to_str().unwrap(),
    ]);
    let titles = blocked_titles(&json);
    assert!(
        titles.contains(&"Combined Target"),
        "ids(\"id-a\") must not be overwritten by ids(\"id-b\"): {titles:?}"
    );
    assert!(
        titles.contains(&"Multi Scope Target"),
        "second scope after condition must check sc-b independently: {titles:?}"
    );
}

/// Verifies `!` negation prefix on `done?`, `todo-state?`, and `has-property?`.
#[test]
fn slice1_negated_conditions_and_done_predicate() {
    let dir = TempDir::new().unwrap();
    let file = write_org(
        dir.path(),
        "negation.org",
        concat!(
            "* TODO Active Guard\n:PROPERTIES:\n:ID: g-todo\n:HOLD: yes\n:END:\n",
            "* DONE Closed Guard\n:PROPERTIES:\n:ID: g-done\n:END:\n",
            "* TODO Unblocked By NotDone\n:PROPERTIES:\n:BLOCKER: ids(\"g-todo\") !done?\n:END:\n",
            "* TODO Blocked By NotDone On Done\n:PROPERTIES:\n:ID: g-b1\n:BLOCKER: ids(\"g-done\") !done?\n:END:\n",
            "* TODO Blocked By Negated Property\n:PROPERTIES:\n:ID: g-b2\n:BLOCKER: ids(\"g-todo\") !has-property?(\"HOLD\" \"yes\")\n:END:\n",
            "* TODO Unblocked By Explicit Done\n:PROPERTIES:\n:BLOCKER: ids(\"g-done\") done?\n:END:\n",
        ),
    );

    let json = run_org_json(&[
        "query",
        "blocked",
        "--format",
        "json",
        file.to_str().unwrap(),
    ]);
    let titles = blocked_titles(&json);
    assert!(!titles.contains(&"Unblocked By NotDone"));
    assert!(!titles.contains(&"Unblocked By Explicit Done"));
    assert!(titles.contains(&"Blocked By NotDone On Done"));
    assert!(titles.contains(&"Blocked By Negated Property"));
}

/// Verifies `re-search?` inspects entry subtree text and handles invalid regexes safely.
#[test]
fn slice1_re_search_evaluates_body_and_handles_invalid_regex() {
    let dir = TempDir::new().unwrap();
    let file = write_org(
        dir.path(),
        "regex.org",
        concat!(
            "* TODO Spec Entry\n:PROPERTIES:\n:ID: spec-1\n:END:\n- [ ] Pending signoff\n",
            "* TODO Approved Spec\n:PROPERTIES:\n:ID: spec-2\n:END:\n- [X] Approved by QA\n",
            "* TODO Needs Signoff\n:PROPERTIES:\n:BLOCKER: ids(\"spec-1\") re-search?(\"\\\\[X\\\\] Approved\")\n:END:\n",
            "* TODO Has Signoff\n:PROPERTIES:\n:BLOCKER: ids(\"spec-2\") re-search?(\"\\\\[X\\\\] Approved\")\n:END:\n",
            "* TODO Bad Regex Task\n:PROPERTIES:\n:BLOCKER: ids(\"spec-2\") re-search?(\"[unclosed\")\n:END:\n",
        ),
    );

    let json = run_org_json(&[
        "query",
        "blocked",
        "--format",
        "json",
        file.to_str().unwrap(),
    ]);
    let titles = blocked_titles(&json);
    assert!(titles.contains(&"Needs Signoff"));
    assert!(!titles.contains(&"Has Signoff"));
    assert!(titles.contains(&"Bad Regex Task"));
}

/// Verifies `file()`, cross-file/same-file `olp()`, and level-0 `org-roam` file `:ID:` resolution.
#[test]
fn slice1_file_olp_and_roam_file_id_finders() {
    let dir = TempDir::new().unwrap();
    write_org(
        dir.path(),
        "prereqs.org",
        concat!(
            "* Milestone 1\n",
            "** TODO Gate Open\n",
            "** DONE Gate Closed\n",
        ),
    );
    write_org(
        dir.path(),
        "roam.org",
        ":PROPERTIES:\n:ID: roam-doc-1\n:STATUS: draft\n:END:\n#+TITLE: Roam Doc\n",
    );
    write_org(
        dir.path(),
        "main.org",
        concat!(
            "* Local Section\n** TODO Local Gate\n",
            "* TODO Blocked By File\n:PROPERTIES:\n:BLOCKER: file(\"prereqs.org\")\n:END:\n",
            "* TODO Blocked By Cross Olp\n:PROPERTIES:\n:BLOCKER: olp(\"prereqs.org\" \"Milestone 1\" \"Gate Open\")\n:END:\n",
            "* TODO Unblocked By Cross Olp\n:PROPERTIES:\n:BLOCKER: olp(\"prereqs.org\" \"Milestone 1\" \"Gate Closed\")\n:END:\n",
            "* TODO Blocked By Local Olp\n:PROPERTIES:\n:BLOCKER: olp(\"Local Section\" \"Local Gate\")\n:END:\n",
            "* TODO Blocked By Roam Prop\n:PROPERTIES:\n:BLOCKER: ids(\"roam-doc-1\") has-property?(\"STATUS\" \"approved\")\n:END:\n",
        ),
    );

    let json = run_org_json(&[
        "query",
        "blocked",
        "--format",
        "json",
        dir.path().to_str().unwrap(),
    ]);
    let titles = blocked_titles(&json);
    assert!(titles.contains(&"Blocked By File"));
    assert!(titles.contains(&"Blocked By Cross Olp"));
    assert!(!titles.contains(&"Unblocked By Cross Olp"));
    assert!(titles.contains(&"Blocked By Local Olp"));
    assert!(titles.contains(&"Blocked By Roam Prop"));
}

// endregion

// region Slice 2: Cross-File Trigger Cascade & Actions

/// Verifies `set-state` fires cross-project `:TRIGGER:` across sibling subdirectories via `.org-tools.toml`.
#[test]
fn slice2_set_state_executes_cross_file_trigger_cascade_with_workspace() {
    let dir = TempDir::new().unwrap();
    write_org(dir.path(), ".org-tools.toml", "[format]\n");
    let alpha = write_org(
        dir.path(),
        "proj_a/alpha.org",
        concat!(
            "* TODO Design API\n",
            ":PROPERTIES:\n",
            ":ID:      alpha-1\n",
            ":TRIGGER: ids(\"beta-1\") ids(\"beta-2\") todo!(\"NEXT\")\n",
            ":END:\n",
        ),
    );
    let beta = write_org(
        dir.path(),
        "proj_b/beta.org",
        concat!(
            "* TODO Implement Client\n:PROPERTIES:\n:ID: beta-1\n:END:\n",
            "* TODO Implement Server\n:PROPERTIES:\n:ID: beta-2\n:END:\n",
        ),
    );

    // Transitioning TODO -> NEXT (non-done) must NOT fire :TRIGGER:.
    org()
        .args(["update", "set-state", "NEXT", alpha.to_str().unwrap()])
        .assert()
        .success();
    assert!(fs::read_to_string(&beta)
        .unwrap()
        .contains("* TODO Implement Client"));

    // Transitioning NEXT -> DONE (!was_done && is_new_done) MUST fire :TRIGGER: across workspace.
    let json = run_org_json(&[
        "update",
        "set-state",
        "DONE",
        alpha.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_valid_mutation_json(&json, "set-state", false);
    assert_eq!(json["files_modified"], 2);
    assert_eq!(json["entries_affected"], 3);

    let beta_after = fs::read_to_string(&beta).unwrap();
    assert!(beta_after.contains("* NEXT Implement Client"));
    assert!(beta_after.contains("* NEXT Implement Server"));
}

/// Verifies recursive downstream `todo!` cascades and deterministic loop halting on cyclic triggers.
#[test]
fn slice2_recursive_todo_trigger_and_loop_guard() {
    let dir = TempDir::new().unwrap();
    let chain_file = write_org(
        dir.path(),
        "cascade.org",
        concat!(
            "* TODO Step 1\n:PROPERTIES:\n:ID: s-1\n:TRIGGER: ids(\"s-2\") todo!(\"DONE\")\n:END:\n",
            "* TODO Step 2\n:PROPERTIES:\n:ID: s-2\n:TRIGGER: ids(\"s-3\") todo!(\"NEXT\")\n:END:\n",
            "* TODO Step 3\n:PROPERTIES:\n:ID: s-3\n:END:\n",
        ),
    );

    org()
        .args([
            "update",
            "set-state",
            "DONE",
            &format!("{}::*/Step 1", chain_file.display()),
            "--workspace",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let after = fs::read_to_string(&chain_file).unwrap();
    assert!(after.contains("* DONE Step 2\nCLOSED:"));
    assert!(after.contains("* NEXT Step 3"));

    // Cyclic :TRIGGER: loop must terminate cleanly without hanging or panicking.
    let cyc_dir = TempDir::new().unwrap();
    let cyc_file = write_org(
        cyc_dir.path(),
        "loop.org",
        concat!(
            "* TODO Ping\n:PROPERTIES:\n:ID: ping\n:TRIGGER: ids(\"pong\") todo!(\"TODO\") ids(\"pong\") todo!(\"DONE\")\n:END:\n",
            "* TODO Pong\n:PROPERTIES:\n:ID: pong\n:TRIGGER: ids(\"ping\") todo!(\"TODO\") ids(\"ping\") todo!(\"DONE\")\n:END:\n",
        ),
    );
    org()
        .args([
            "update",
            "set-state",
            "DONE",
            &format!("{}::*/Ping", cyc_file.display()),
            "--workspace",
            cyc_dir.path().to_str().unwrap(),
        ])
        .assert()
        .success();
}

/// Asserts `file` passes `org fmt check` with zero formatting diagnostics (allowing only `W036`).
fn assert_fmt_clean_ignoring_w036(file: &Path) {
    let diags_out = org()
        .args(["fmt", "check", "--format", "json", file.to_str().unwrap()])
        .output()
        .unwrap();
    if !diags_out.status.success() {
        let diags: Value = serde_json::from_slice(&diags_out.stdout).unwrap();
        for d in diags.as_array().unwrap() {
            assert_eq!(
                d["rule_id"].as_str(),
                Some("W036"),
                "unexpected diagnostic: {d}"
            );
        }
    }
}

/// Verifies all `:TRIGGER:` actions (`set-property!`, `delete-property!`, `tag!`, `scheduled!`, `deadline!`, `set-priority!`, `set-effort!`, `chain!`, `archive!`).
#[test]
fn slice2_all_trigger_actions_execute_and_stay_fmt_clean() {
    let dir = TempDir::new().unwrap();
    let fixture = concat!(
        "* TODO Source Task\n:PROPERTIES:\n:ID:      act-src\n:SPRINT:  2026-W45\n",
        ":TRIGGER: ids(\"act-tgt\") set-property!(\"OWNER\" \"alice\") delete-property!(\"LEGACY\") tag!(\"ready:ops\") scheduled!(\"2026-11-15\") deadline!(\"2026-11-20\") set-priority!(\"A\") set-effort!(\"2:00\") chain!(\"SPRINT\") ids(\"act-arch\") archive!\n",
        ":END:\n\n* TODO Target Task\n:PROPERTIES:\n:ID:     act-tgt\n:LEGACY: drop-me\n:END:\n\n",
        "* DONE Old Completed Task\n:PROPERTIES:\n:ID: act-arch\n:END:\n",
    );
    let file = write_org(dir.path(), "actions.org", fixture);
    let loc = format!("{}::*/Source Task", file.display());
    org()
        .args([
            "update",
            "set-state",
            "DONE",
            &loc,
            "--workspace",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let updated = fs::read_to_string(&file).unwrap();
    for expected in [
        "* TODO [#A] Target Task",
        ":ready:ops:",
        "SCHEDULED: <2026-11-15 Sun>",
        "DEADLINE: <2026-11-20 Fri>",
        ":OWNER:  alice",
        ":EFFORT: 2:00",
        ":SPRINT: 2026-W45",
    ] {
        assert!(updated.contains(expected), "missing {expected}");
    }
    assert!(!updated.contains(":LEGACY:") && !updated.contains("Old Completed Task"));
    let archive_file = dir.path().join("actions_archive.org");
    assert!(fs::read_to_string(&archive_file)
        .unwrap()
        .contains("Old Completed Task"));
    org()
        .args(["fmt", "check", archive_file.to_str().unwrap()])
        .assert()
        .success();
    org()
        .args(["fmt", "check", dir.path().to_str().unwrap()])
        .assert()
        .success();
    assert_fmt_clean_ignoring_w036(&file);
}

/// Verifies primary `org update set-state DONE` preserves `F007` column-77 tag alignment.
#[test]
fn slice2_primary_set_state_preserves_f007_tag_alignment() {
    let dir = TempDir::new().unwrap();
    let heading = format!(
        "* TODO [#A] Ship Release{} :release:ops:\n",
        " ".repeat(77 - 24 - 14)
    );
    let file = write_org(dir.path(), "tagged.org", &heading);
    org()
        .args(["fmt", "check", file.to_str().unwrap()])
        .assert()
        .success();

    org()
        .args(["update", "set-state", "DONE", file.to_str().unwrap()])
        .assert()
        .success();

    org()
        .args(["fmt", "check", file.to_str().unwrap()])
        .assert()
        .success();
}

/// Verifies `--dry-run` computes the full trigger cascade in memory with zero disk writes.
#[test]
fn slice2_dry_run_reports_triggered_details_without_disk_writes() {
    let dir = TempDir::new().unwrap();
    let src_content = "* TODO Gate\n:PROPERTIES:\n:ID:      dry-src\n:TRIGGER: ids(\"dry-tgt\") todo!(\"NEXT\") set-property!(\"READY\" \"true\")\n:END:\n";
    let tgt_content = "* TODO Downstream\n:PROPERTIES:\n:ID: dry-tgt\n:END:\n";
    let src_file = write_org(dir.path(), "src.org", src_content);
    let tgt_file = write_org(dir.path(), "tgt.org", tgt_content);

    let json = run_org_json(&[
        "update",
        "set-state",
        "DONE",
        src_file.to_str().unwrap(),
        "--workspace",
        dir.path().to_str().unwrap(),
        "--dry-run",
        "--format",
        "json",
    ]);
    assert_valid_mutation_json(&json, "set-state", true);
    assert_eq!(json["files_modified"], 2);
    let actions: Vec<&str> = json["details"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["action"].as_str())
        .collect();
    assert!(actions.contains(&"state-changed"));
    assert!(actions.contains(&"triggered-todo"));
    assert!(actions.contains(&"triggered-property"));

    assert_eq!(fs::read_to_string(&src_file).unwrap(), src_content);
    assert_eq!(fs::read_to_string(&tgt_file).unwrap(), tgt_content);
}

// endregion

// region Slice 3: Symmetric Dep Subcommands & Static Lints

/// Executes `org update dep <subcmd> <blocker> <blocked> --workspace <ws>` with optional flags.
fn exec_dep(subcmd: &str, blocker: &str, blocked: &str, ws: &str, extra: &[&str]) {
    let mut args = vec!["update", "dep", subcmd, blocker, blocked, "--workspace", ws];
    args.extend_from_slice(extra);
    org().args(&args).assert().success();
}

/// Executes `org update dep <subcmd> ... --format json` and parses the JSON output.
fn exec_dep_json(subcmd: &str, blocker: &str, blocked: &str, ws: &str, dry_run: bool) -> Value {
    let mut args = vec!["update", "dep", subcmd, blocker, blocked, "--workspace", ws];
    if dry_run {
        args.push("--dry-run");
    }
    args.extend_from_slice(&["--format", "json"]);
    run_org_json(&args)
}

/// Verifies `org update dep add` supports `--dry-run`, idempotency, multi-blocker merging, and `F005` cleanliness.
#[test]
fn slice3_dep_add_wires_symmetric_properties_fmt_clean() {
    let dir = TempDir::new().unwrap();
    let f1 = write_org(
        dir.path(),
        "plan.org",
        "* TODO Blocker One\n:PROPERTIES:\n:ID: blk-1\n:END:\n\n* TODO Blocker Two\n:PROPERTIES:\n:ID: blk-2\n:END:\n",
    );
    let f2 = write_org(
        dir.path(),
        "work.org",
        "* TODO Blocked Work\n:PROPERTIES:\n:ID: wrk-1\n:END:\n",
    );
    let ws = dir.path().to_str().unwrap();

    let dry = exec_dep_json("add", "id:blk-1", "id:wrk-1", ws, true);
    assert_valid_mutation_json(&dry, "dep-add", true);
    assert!(!fs::read_to_string(&f2).unwrap().contains(":BLOCKER:"));

    let add1 = exec_dep_json("add", "id:blk-1", "id:wrk-1", ws, false);
    assert_valid_mutation_json(&add1, "dep-add", false);
    assert_eq!(add1["files_modified"], 2);

    // Repeating same add is idempotent; adding blk-2 merges into ids("blk-1" "blk-2").
    exec_dep("add", "id:blk-1", "id:wrk-1", ws, &[]);
    exec_dep("add", "id:blk-2", "id:wrk-1", ws, &[]);

    assert!(fs::read_to_string(&f2)
        .unwrap()
        .contains(":BLOCKER: ids(\"blk-1\" \"blk-2\")"));
    assert!(fs::read_to_string(&f1)
        .unwrap()
        .contains(":TRIGGER: ids(\"wrk-1\") todo!(\"NEXT\")"));
    org().args(["fmt", "check", ws]).assert().success();
}

/// Verifies `org update dep remove` prunes individual IDs and deletes `:BLOCKER:` / `:TRIGGER:` when empty.
#[test]
fn slice3_dep_remove_prunes_and_deletes_empty_properties() {
    let dir = TempDir::new().unwrap();
    let f1 = write_org(
        dir.path(),
        "plan.org",
        concat!(
            "* TODO Blocker One\n:PROPERTIES:\n:ID:      blk-1\n:TRIGGER: ids(\"wrk-1\") todo!(\"NEXT\")\n:END:\n\n",
            "* TODO Blocker Two\n:PROPERTIES:\n:ID:      blk-2\n:TRIGGER: ids(\"wrk-1\") todo!(\"NEXT\")\n:END:\n",
        ),
    );
    let f2 = write_org(
        dir.path(),
        "work.org",
        "* TODO Blocked Work\n:PROPERTIES:\n:ID:      wrk-1\n:BLOCKER: ids(\"blk-1\" \"blk-2\")\n:END:\n",
    );
    let ws = dir.path().to_str().unwrap();

    org()
        .args([
            "update",
            "dep",
            "remove",
            "id:blk-1",
            "id:wrk-1",
            "--workspace",
            ws,
        ])
        .assert()
        .success();
    assert!(fs::read_to_string(&f2)
        .unwrap()
        .contains(":BLOCKER: ids(\"blk-2\")"));

    let rem2 = run_org_json(&[
        "update",
        "dep",
        "remove",
        "id:blk-2",
        "id:wrk-1",
        "--workspace",
        ws,
        "--format",
        "json",
    ]);
    assert_valid_mutation_json(&rem2, "dep-remove", false);
    assert!(!fs::read_to_string(&f2).unwrap().contains(":BLOCKER:"));
    assert!(!fs::read_to_string(&f1).unwrap().contains(":TRIGGER:"));
    org().args(["fmt", "check", ws]).assert().success();
}

/// Verifies `org update dep add` refuses missing `:ID:`s and self/2-node/3-node cycles with exit 2.
#[test]
fn slice3_dep_add_rejects_cycles_and_missing_ids() {
    let dir = TempDir::new().unwrap();
    let f1 = write_org(
        dir.path(),
        "a.org",
        "* TODO No Id Entry\n\n* TODO Node A\n:PROPERTIES:\n:ID:      node-a\n:BLOCKER: ids(\"node-b\")\n:END:\n",
    );
    let f2 = write_org(
        dir.path(),
        "b.org",
        "* TODO Node B\n:PROPERTIES:\n:ID:      node-b\n:BLOCKER: ids(\"node-c\")\n:END:\n\n* TODO Node C\n:PROPERTIES:\n:ID: node-c\n:END:\n",
    );
    let ws = dir.path().to_str().unwrap();
    let no_id_loc = format!("{}::*/No Id Entry", f1.display());

    for (blocker, blocked) in [
        (&no_id_loc as &str, "id:node-c"),
        ("id:node-c", "id:node-c"),
    ] {
        org()
            .args(["update", "dep", "add", blocker, blocked, "--workspace", ws])
            .assert()
            .code(2);
    }

    let before_f2 = fs::read_to_string(&f2).unwrap();
    org()
        .args([
            "update",
            "dep",
            "add",
            "id:node-a",
            "id:node-c",
            "--workspace",
            ws,
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cycle").or(predicate::str::contains("circular")));
    assert_eq!(fs::read_to_string(&f2).unwrap(), before_f2);
}

/// Verifies `E008` (`circular-edna-blocker`) detects self, 2-node, and cross-file cycles while skipping code blocks.
#[test]
fn slice3_lint_e008_detects_self_two_node_and_cross_file_cycles() {
    let dir = TempDir::new().unwrap();
    write_org(
        dir.path(),
        "one.org",
        concat!(
            "* TODO Self Cycle\n:PROPERTIES:\n:ID:      s-1\n:BLOCKER: ids(\"s-1\")\n:END:\n\n",
            "* TODO Cross A\n:PROPERTIES:\n:ID:      cx-a\n:BLOCKER: ids(\"cx-b\")\n:END:\n\n",
            "#+begin_src org\n* TODO Protected\n:PROPERTIES:\n:ID: p-1\n:BLOCKER: ids(\"p-1\")\n:END:\n#+end_src\n",
        ),
    );
    write_org(
        dir.path(),
        "two.org",
        concat!(
            "* TODO Cross B\n:PROPERTIES:\n:ID:      cx-b\n:BLOCKER: ids(\"cx-c\")\n:END:\n\n",
            "* TODO Cross C\n:PROPERTIES:\n:ID:      cx-c\n:BLOCKER: ids(\"cx-a\")\n:END:\n",
        ),
    );

    let diags = run_org_issues_json(&[
        "fmt",
        "check",
        "--format",
        "json",
        dir.path().to_str().unwrap(),
    ]);
    let e008: Vec<&Value> = diags
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule_id"].as_str() == Some("E008"))
        .collect();
    assert!(
        e008.len() >= 4,
        "expected E008 on self-loop + 3 cross-file cycle nodes, got: {e008:?}"
    );
    for d in &e008 {
        assert_eq!(d["rule"].as_str(), Some("circular-edna-blocker"));
        assert_eq!(d["severity"].as_str(), Some("error"));
    }
}

/// Verifies `W036` (`unresolved-edna-ids`) warns on missing IDs, resolves workspace/roam IDs, and skips protected blocks.
#[test]
fn slice3_lint_w036_warns_unresolved_ids_and_respects_workspace_and_roam() {
    let dir = TempDir::new().unwrap();
    write_org(
        dir.path(),
        "roam_target.org",
        ":PROPERTIES:\n:ID: roam-valid-id\n:END:\n#+TITLE: Roam Target\n",
    );
    write_org(
        dir.path(),
        "peer.org",
        "* TODO Peer\n:PROPERTIES:\n:ID: peer-valid-id\n:END:\n",
    );
    write_org(
        dir.path(),
        "refs.org",
        concat!(
            "* TODO Valid Cross Refs\n",
            ":PROPERTIES:\n",
            ":BLOCKER: ids(\"roam-valid-id\" \"peer-valid-id\")\n",
            ":END:\n\n",
            "* TODO Missing Ref\n",
            ":PROPERTIES:\n",
            ":TRIGGER: ids(\"ghost-missing-uuid\") todo!(\"NEXT\")\n",
            ":END:\n\n",
            "#+begin_example\n:BLOCKER: ids(\"ignored-in-example\")\n#+end_example\n",
        ),
    );

    let diags = run_org_issues_json(&[
        "fmt",
        "check",
        "--format",
        "json",
        dir.path().to_str().unwrap(),
    ]);
    let w036: Vec<&Value> = diags
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["rule_id"].as_str() == Some("W036"))
        .collect();
    assert_eq!(
        w036.len(),
        1,
        "expected exactly 1 W036 warning, got: {w036:?}"
    );
    assert_eq!(w036[0]["rule"].as_str(), Some("unresolved-edna-ids"));
    assert_eq!(w036[0]["severity"].as_str(), Some("warning"));
    assert!(w036[0]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("ghost-missing-uuid"));
}

// endregion
