// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Red-to-Green integration tests for workspace-aware dynamic shell completions (`ORGFMT-WP-36`).

use std::fs;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::time::Instant;

use assert_cmd::Command;
use org_tools_core::cache::WorkspaceCache;
use tempfile::TempDir;

// region Helpers & Fixtures

/// Warmup and measured sample counts (`p95` index `7` of `9`).
const BENCH_WARMUP_MEASURED_P95: (usize, usize, usize) = (3, 9, 7);

/// Release warm completion query budget in microseconds (`5 ms`).
const RELEASE_WARM_BUDGET_US: u128 = 5_000;

/// Debug warm completion query budget in microseconds (`50 ms`).
const DEBUG_WARM_BUDGET_US: u128 = 50_000;

/// Constructs an `assert_cmd` runner for the `org` binary.
fn org() -> Command {
    Command::cargo_bin("org").expect("org binary must compile")
}

/// Runs `org` with `args`, asserts exit code `0`, and returns UTF-8 stdout.
fn run_ok(args: &[&str]) -> String {
    let out = org().args(args).assert().success();
    String::from_utf8(out.get_output().stdout.clone()).expect("stdout must be UTF-8")
}

/// Writes UTF-8 Org content to `path`, creating parent directories when needed.
fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, content).expect("write fixture file");
}

/// Seeds a multi-file workspace containing roam nodes, excluded IDs, custom IDs, tags, properties, and custom TODOs.
fn seed_completion_workspace(root: &Path) {
    write_file(
        &root.join(".org-tools.toml"),
        "[todo]\nsequences = [\"TODO NEXT CONFIG_STATE | DONE CANCELED\"]\n",
    );
    write_file(
        &root.join("specs/alpha.org"),
        ":PROPERTIES:\n\
         :ID: 11111111-1111-4111-8111-111111111111\n\
         :ID_SEED: urn:kb:orgfmt:spec:alpha\n\
         :ROAM_ALIASES: \"Alpha Spec\" \"Architecture Alpha\"\n\
         :FILE_LEVEL_PROP: file-val\n\
         :END:\n\
         #+TITLE: Alpha Architecture\n\
         #+FILETAGS: :arch:@laptop:\n\
         #+TODO: TODO NEXT REVIEW | DONE ARCHIVED\n\n\
         * REVIEW [#A] Alpha Heading :work:\n\
         :PROPERTIES:\n\
         :ID: 22222222-2222-4222-8222-222222222222\n\
         :CUSTOM_ID: ALPHA-01\n\
         :EFFORT: 0:30\n\
         :CUSTOM_PROP: custom-val\n\
         :END:\n\
         Body text.\n\n\
         * TODO Excluded Heading :internal:\n\
         :PROPERTIES:\n\
         :ID: 33333333-3333-4333-8333-333333333333\n\
         :CUSTOM_ID: EXCL-01\n\
         :ROAM_EXCLUDE: t\n\
         :END:\n\
         Excluded body.\n",
    );
    write_file(
        &root.join("notes/beta.org"),
        "* NEXT Beta Task :@office:ops:\n\
         :PROPERTIES:\n\
         :CUSTOM_ID: BETA-02\n\
         :OWNER: alice\n\
         :END:\n\
         Task body.\n",
    );
}

// endregion

// region Slice 1: SQLite v4 Completion Indexing & ValueHint / Schema Metadata

/// Verifies `WorkspaceCache` schema v4 indexes IDs, custom IDs, headings, tags, properties, and TODO keywords with incremental sync.
#[test]
fn slice1_sqlite_cache_v4_indexes_ids_custom_ids_headings_tags_props_and_todos_with_incremental_sync(
) {
    let dir = TempDir::new().unwrap();
    seed_completion_workspace(dir.path());

    let (cache, tel1) = WorkspaceCache::open_and_sync(dir.path()).unwrap();
    assert_eq!(
        (tel1.files_scanned, tel1.cache_hits, tel1.cache_misses),
        (2, 0, 2)
    );

    let ids = cache.complete_ids("").unwrap();
    assert!(ids
        .iter()
        .any(|c| c.id == "11111111-1111-4111-8111-111111111111"));
    assert!(ids
        .iter()
        .any(|c| c.id == "22222222-2222-4222-8222-222222222222"));
    assert!(
        ids.iter()
            .any(|c| c.id == "33333333-3333-4333-8333-333333333333"),
        "ROAM_EXCLUDE=t entry IDs must still be indexed for id/locator completion"
    );

    let outlines = cache.complete_headings("").unwrap();
    assert!(outlines
        .iter()
        .any(|h| h.custom_id.as_deref() == Some("ALPHA-01")));
    assert!(outlines
        .iter()
        .any(|h| h.custom_id.as_deref() == Some("EXCL-01")));
    assert!(outlines
        .iter()
        .any(|h| h.custom_id.as_deref() == Some("BETA-02")));
    assert!(outlines
        .iter()
        .any(|h| h.title == "Beta Task" && h.rel_path == "notes/beta.org"));

    let tags = cache.complete_tags("").unwrap();
    for expected_tag in ["arch", "@laptop", "work", "internal", "@office", "ops"] {
        assert!(
            tags.iter().any(|t| t == expected_tag),
            "missing tag {expected_tag}"
        );
    }

    let props = cache.complete_property_keys("").unwrap();
    for expected_prop in [
        "CUSTOM_PROP",
        "OWNER",
        "EFFORT",
        "CUSTOM_ID",
        "FILE_LEVEL_PROP",
    ] {
        assert!(
            props.iter().any(|p| p == expected_prop),
            "missing property key {expected_prop}"
        );
    }

    let (active_todos, done_todos) = cache.complete_todo_keywords().unwrap();
    assert!(active_todos.iter().any(|k| k == "REVIEW"));
    assert!(done_todos.iter().any(|k| k == "ARCHIVED"));
    drop(cache);

    let beta_path = dir.path().join("notes/beta.org");
    let bumped =
        fs::metadata(&beta_path).unwrap().modified().unwrap() + std::time::Duration::from_secs(2);
    write_file(
        &beta_path,
        "* NEXT Beta Task :@office:ops:\n\
         :PROPERTIES:\n\
         :CUSTOM_ID: BETA-02\n\
         :OWNER: alice\n\
         :END:\n\n\
         * WAIT Gamma Followup :@home:\n\
         :PROPERTIES:\n\
         :ID: 44444444-4444-4444-8444-444444444444\n\
         :CUSTOM_ID: GAMMA-03\n\
         :STAGE_KEY: triage\n\
         :END:\n",
    );
    let f = fs::OpenOptions::new().write(true).open(&beta_path).unwrap();
    f.set_modified(bumped).unwrap();

    let (cache2, tel2) = WorkspaceCache::open_and_sync(dir.path()).unwrap();
    assert_eq!(
        (tel2.files_scanned, tel2.cache_hits, tel2.cache_misses),
        (2, 1, 1),
        "only modified beta.org should be re-parsed"
    );
    assert!(cache2
        .complete_ids("4444")
        .unwrap()
        .iter()
        .any(|c| c.id == "44444444-4444-4444-8444-444444444444"));
    assert!(cache2
        .complete_headings("GAMMA")
        .unwrap()
        .iter()
        .any(|h| h.custom_id.as_deref() == Some("GAMMA-03")));
    assert!(cache2
        .complete_tags("@h")
        .unwrap()
        .iter()
        .any(|t| t == "@home"));
    assert!(cache2
        .complete_property_keys("STAGE")
        .unwrap()
        .iter()
        .any(|p| p == "STAGE_KEY"));
}

/// Verifies `PossibleValuesParser` on `org schema` and `ValueHint` annotations in generated completion scripts.
#[test]
fn slice1_value_hints_and_schema_possible_values_are_registered_across_cli() {
    let zsh = run_ok(&["--completions", "zsh"]);
    let fish = run_ok(&["--completions", "fish"]);

    for schema_name in [
        "fmt",
        "query",
        "agenda",
        "columns",
        "show",
        "update",
        "clock-status",
        "clock-report",
        "jscalendar",
        "roam",
        "fleeting",
        "blocked",
        "stuck",
        "deps",
    ] {
        assert!(
            zsh.contains(schema_name) && fish.contains(schema_name),
            "schema completion missing '{schema_name}'"
        );
    }

    assert!(
        zsh.contains("_files -/"),
        "zsh completions must use '_files -/' directory hints for DirPath options"
    );
    assert!(
        fish.contains("__fish_complete_directories"),
        "fish completions must use '__fish_complete_directories' for DirPath options"
    );
}

// endregion

// region Slice 2: Dynamic `org complete` Across All 9 Domains & Warm Latency

/// Verifies `org complete <kind>` output formatting across `zsh`, `fish`, and `bash` for all domains.
#[test]
fn slice2_org_complete_emits_candidates_and_descriptions_across_all_domains() {
    let dir = TempDir::new().unwrap();
    seed_completion_workspace(dir.path());
    let ws = dir.path().to_str().unwrap();

    let id_zsh = run_ok(&["complete", "id", "", "--workspace", ws, "--shell", "zsh"]);
    assert!(id_zsh.contains(
        "id:22222222-2222-4222-8222-222222222222\tREVIEW Alpha Heading (specs/alpha.org:11)"
    ));
    assert!(id_zsh.contains(
        "22222222-2222-4222-8222-222222222222\tREVIEW Alpha Heading (specs/alpha.org:11)"
    ));
    assert!(id_zsh.contains(
        "id:33333333-3333-4333-8333-333333333333\tTODO Excluded Heading (specs/alpha.org:20)"
    ));

    let id_filtered = run_ok(&[
        "complete",
        "id",
        "alpha",
        "--workspace",
        ws,
        "--shell",
        "zsh",
    ]);
    assert!(id_filtered.contains("22222222-2222-4222-8222-222222222222"));
    assert!(!id_filtered.contains("33333333-3333-4333-8333-333333333333"));

    let id_bash = run_ok(&[
        "complete",
        "id",
        "id:2222",
        "--workspace",
        ws,
        "--shell",
        "bash",
    ]);
    for line in id_bash.lines() {
        assert!(
            !line.contains('\t'),
            "bash mode must omit tab descriptions: {line}"
        );
    }
    assert!(id_bash
        .lines()
        .any(|l| l == "id:22222222-2222-4222-8222-222222222222"));

    let loc_fish = run_ok(&[
        "complete",
        "locator",
        "",
        "--workspace",
        ws,
        "--shell",
        "fish",
    ]);
    assert!(loc_fish.contains("id:22222222-2222-4222-8222-222222222222\t"));
    assert!(loc_fish.contains("#ALPHA-01\t"));
    assert!(loc_fish.contains("specs/alpha.org::#ALPHA-01\t"));
    assert!(loc_fish.contains("notes/beta.org::#BETA-02\t"));
    assert!(loc_fish.contains("notes/beta.org::*Beta Task\t"));

    let roam_zsh = run_ok(&[
        "complete",
        "roam-node",
        "",
        "--workspace",
        ws,
        "--shell",
        "zsh",
    ]);
    assert!(roam_zsh.contains("Alpha Architecture\t"));
    assert!(roam_zsh.contains("Alpha Spec\t"));
    assert!(roam_zsh.contains("id:11111111-1111-4111-8111-111111111111\t"));
    assert!(!roam_zsh.contains("33333333-3333-4333-8333-333333333333"));

    let todo_zsh = run_ok(&[
        "complete",
        "todo-state",
        "",
        "--workspace",
        ws,
        "--shell",
        "zsh",
    ]);
    assert!(todo_zsh.contains("TODO\tactive state"));
    assert!(todo_zsh.contains("NEXT\tactive state"));
    assert!(todo_zsh.contains("REVIEW\tactive state"));
    assert!(todo_zsh.contains("CONFIG_STATE\tactive state"));
    assert!(todo_zsh.contains("WAIT\tactive state"));
    assert!(todo_zsh.contains("BLOCKED\tactive state"));
    assert!(todo_zsh.contains("DONE\tdone state"));
    assert!(todo_zsh.contains("ARCHIVED\tdone state"));

    let tag_zsh = run_ok(&["complete", "tag", "", "--workspace", ws, "--shell", "zsh"]);
    assert!(tag_zsh.contains("work\t"));
    assert!(tag_zsh.contains("@laptop\t"));
    assert!(tag_zsh.contains("@office\t"));

    let ctx_zsh = run_ok(&[
        "complete",
        "context",
        "",
        "--workspace",
        ws,
        "--shell",
        "zsh",
    ]);
    assert!(ctx_zsh.contains("@laptop\t"));
    assert!(ctx_zsh.contains("@office\t"));
    assert!(!ctx_zsh.lines().any(|l| l.starts_with("work\t")));

    let prop_zsh = run_ok(&[
        "complete",
        "property",
        "",
        "--workspace",
        ws,
        "--shell",
        "zsh",
    ]);
    assert!(prop_zsh.contains("EFFORT\t"));
    assert!(prop_zsh.contains("BLOCKER\t"));
    assert!(prop_zsh.contains("TRIGGER\t"));
    assert!(prop_zsh.contains("CUSTOM_PROP\t"));
    assert!(prop_zsh.contains("OWNER\t"));

    let date_zsh = run_ok(&[
        "complete",
        "date-spec",
        "",
        "--workspace",
        ws,
        "--shell",
        "zsh",
    ]);
    for tok in [
        "today\t", ".\t", "+1d\t", "+3d\t", "+1w\t", "+2w\t", "+1m\t", "-1d\t",
    ] {
        assert!(date_zsh.contains(tok), "missing date-spec candidate {tok}");
    }

    let q_root = run_ok(&[
        "complete",
        "query",
        "",
        "--workspace",
        ws,
        "--shell",
        "fish",
    ]);
    for pfx in [
        "todo:",
        "done:",
        "tags:",
        "heading:",
        "body:",
        "property:",
        "priority:",
        "scheduled:",
        "deadline:",
    ] {
        assert!(q_root.contains(pfx), "missing query predicate prefix {pfx}");
    }
    let q_tags = run_ok(&[
        "complete",
        "query",
        "tags:",
        "--workspace",
        ws,
        "--shell",
        "fish",
    ]);
    assert!(q_tags.contains("tags:work\t"));
    assert!(q_tags.contains("tags:@laptop\t"));
    let q_todo = run_ok(&[
        "complete",
        "query",
        "todo:",
        "--workspace",
        ws,
        "--shell",
        "fish",
    ]);
    assert!(q_todo.contains("todo:REVIEW\t"));
    let q_prop = run_ok(&[
        "complete",
        "query",
        "property:",
        "--workspace",
        ws,
        "--shell",
        "fish",
    ]);
    assert!(q_prop.contains("property:CUSTOM_PROP\t"));

    let rules_zsh = run_ok(&["complete", "rule", "", "--workspace", ws, "--shell", "zsh"]);
    for rule_tok in [
        "F001\t",
        "trailing-whitespace\t",
        "F007\t",
        "E001\t",
        "W001\t",
        "W035\t",
        "I001\t",
        "I004\t",
    ] {
        assert!(
            rules_zsh.contains(rule_tok),
            "missing rule completion {rule_tok}"
        );
    }
}

/// Verifies warm SQLite completion queries stay within the 5ms release / 50ms debug latency budget.
#[test]
fn slice2_warm_cache_completion_meets_latency_budget() {
    let dir = TempDir::new().unwrap();
    seed_completion_workspace(dir.path());
    let _ = WorkspaceCache::open_and_sync(dir.path()).unwrap();

    let budget_us = if cfg!(not(debug_assertions)) {
        RELEASE_WARM_BUDGET_US
    } else {
        DEBUG_WARM_BUDGET_US
    };
    let (warmup, runs, p95_idx) = BENCH_WARMUP_MEASURED_P95;
    let mut samples = Vec::with_capacity(runs);
    for iter in 0..(warmup + runs) {
        let t0 = Instant::now();
        let (cache, _) = WorkspaceCache::open_and_sync(dir.path()).unwrap();
        let _ = cache.complete_ids("").unwrap();
        let _ = cache.complete_headings("").unwrap();
        let _ = cache.complete_tags("").unwrap();
        let _ = cache.complete_property_keys("").unwrap();
        let _ = cache.complete_todo_keywords().unwrap();
        let elapsed_us = t0.elapsed().as_micros();
        if iter >= warmup {
            samples.push(elapsed_us);
        }
    }
    samples.sort_unstable();
    assert!(
        samples[p95_idx] <= budget_us,
        "warm completion p95 {}us exceeded budget {}us ({samples:?})",
        samples[p95_idx],
        budget_us
    );
}

// endregion

// region Slice 3: Dynamic Shell Completion Adapters (`bash`, `zsh`, `fish`)

/// Verifies `org --completions bash|zsh|fish` embed dynamic `org complete` dispatchers and pass shell syntax checks.
#[test]
fn slice3_shell_completion_scripts_embed_dynamic_hooks_for_bash_zsh_and_fish() {
    let bash_script = run_ok(&["--completions", "bash"]);
    let zsh_script = run_ok(&["--completions", "zsh"]);
    let fish_script = run_ok(&["--completions", "fish"]);

    assert!(bash_script.contains("org complete"));
    assert!(bash_script.contains("compgen -W"));
    assert!(zsh_script.contains("org complete"));
    assert!(zsh_script.contains("_describe"));
    assert!(fish_script.contains("org complete"));
    assert!(fish_script.contains("complete -c org"));

    for kind in [
        "locator",
        "roam-node",
        "todo-state",
        "tag",
        "context",
        "property",
        "date-spec",
        "query",
    ] {
        assert!(
            bash_script.contains(kind) && zsh_script.contains(kind) && fish_script.contains(kind),
            "dynamic completion kind '{kind}' missing from shell scripts"
        );
    }

    let tmp = TempDir::new().unwrap();
    let bash_file = tmp.path().join("org.bash");
    let zsh_file = tmp.path().join("_org");
    let fish_file = tmp.path().join("org.fish");
    write_file(&bash_file, &bash_script);
    write_file(&zsh_file, &zsh_script);
    write_file(&fish_file, &fish_script);

    if ProcessCommand::new("bash")
        .arg("--version")
        .output()
        .is_ok()
    {
        let status = ProcessCommand::new("bash")
            .arg("-n")
            .arg(&bash_file)
            .status()
            .unwrap();
        assert!(status.success(), "bash -n syntax check failed on org.bash");
    }
    if ProcessCommand::new("zsh").arg("--version").output().is_ok() {
        let status = ProcessCommand::new("zsh")
            .arg("-n")
            .arg(&zsh_file)
            .status()
            .unwrap();
        assert!(status.success(), "zsh -n syntax check failed on _org");
    }
    if ProcessCommand::new("fish")
        .arg("--version")
        .output()
        .is_ok()
    {
        let status = ProcessCommand::new("fish")
            .arg("--no-execute")
            .arg(&fish_file)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "fish --no-execute syntax check failed on org.fish"
        );
    }
}

// endregion
