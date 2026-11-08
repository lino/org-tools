// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Red-to-Green behavioural, parity, and `corpus-1k` benchmark tests for `WorkspaceCache` (`ORGFMT-WP-06`).

use std::fs;
use std::path::Path;
use std::time::Instant;

use assert_cmd::Command;
use org_tools_core::cache::{WorkspaceCache, CACHE_PATH_ENV_VAR, DEFAULT_CACHE_REL_PATH};
use org_tools_core::files::join_slash_rel;
use serde_json::Value;
use tempfile::TempDir;

// region Constants & Path Helpers

/// Maximum cold `rebuild` duration budget in microseconds (`500 ms`) for release builds.
const COLD_REBUILD_BUDGET_US: u128 = 500_000;
/// Maximum warm query duration budget in microseconds (`5 ms`) for release builds.
const WARM_QUERY_BUDGET_US: u128 = 5_000;
/// Relaxed cold `rebuild` ceiling in microseconds (`5 s`) for unoptimized debug test runs.
const DEBUG_COLD_REBUILD_BUDGET_US: u128 = 5_000_000;
/// Relaxed warm query ceiling in microseconds (`50 ms`) for unoptimized debug test runs.
const DEBUG_WARM_QUERY_BUDGET_US: u128 = 50_000;
/// Environment variable forcing strict release performance budgets (`"1"`) under debug builds.
const STRICT_PERF_BUDGET_ENV_VAR: &str = "ORGFMT_STRICT_PERF_BUDGET";
/// Warmup and measured sample counts (`p95` index `7` of `9`) per `BENCH-SPEC.org`.
const BENCH_WARMUP_MEASURED_P95: (usize, usize, usize) = (3, 9, 7);
/// File count in the canonical `corpus-1k` synthetic benchmark corpus.
const CORPUS_1K_FILES: usize = 1_000;
/// Expected `(files, nodes, links, edna_edges, fts_entries)` row counts in `corpus-1k`.
const CORPUS_1K_ROW_COUNTS: (usize, usize, usize, usize, usize) = (1_000, 4_000, 5_000, 500, 3_000);
/// Hub target UUID receiving deterministic incoming links in `corpus-1k`.
const HUB_FILE_UUID: &str = "f0000000-0000-4000-8000-000000000000";

/// Resolves `(cold_rebuild_us, warm_query_us)` budgets from build profile and `ORGFMT_STRICT_PERF_BUDGET`.
fn resolve_perf_budgets(is_release: bool, strict_env: Option<&str>) -> (u128, u128) {
    if is_release || strict_env == Some("1") {
        (COLD_REBUILD_BUDGET_US, WARM_QUERY_BUDGET_US)
    } else {
        (DEBUG_COLD_REBUILD_BUDGET_US, DEBUG_WARM_QUERY_BUDGET_US)
    }
}

/// Returns active `(cold_rebuild_us, warm_query_us)` budgets for the current test execution.
fn active_perf_budgets() -> (u128, u128) {
    let strict_env = std::env::var(STRICT_PERF_BUDGET_ENV_VAR).ok();
    resolve_perf_budgets(cfg!(not(debug_assertions)), strict_env.as_deref())
}

/// Constructs an `assert_cmd` runner for the `org` binary.
fn org() -> Command {
    Command::cargo_bin("org").expect("org binary must compile")
}

/// Writes UTF-8 Org content to `path`, creating parent directories when needed.
fn write_org(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent dirs");
    }
    fs::write(path, content).expect("write org fixture");
}

/// Executes `org` with `args` and asserts the given exit status `code`.
fn run_code(args: &[&str], code: i32) {
    org().args(args).assert().code(code);
}

/// Executes `org` with `args`, asserts exit code `0`, and returns UTF-8 stdout.
fn run_ok(args: &[&str]) -> String {
    let out = org().args(args).assert().success();
    String::from_utf8(out.get_output().stdout.clone()).expect("stdout must be UTF-8")
}

/// Executes `org` with `args` and parses stdout as JSON [`Value`].
fn run_json(args: &[&str]) -> Value {
    serde_json::from_str(&run_ok(args)).expect("stdout must be valid JSON")
}

/// Extracts `(cache_hits, cache_misses)` from a telemetry JSON object.
fn hit_miss(tel: &Value) -> (u64, u64) {
    let hits = tel["cache_hits"].as_u64().unwrap();
    (hits, tel["cache_misses"].as_u64().unwrap())
}

/// Runs `org roam <sub_args...> <ws> --format json` (with optional `--no-cache`) and splits `(body, telemetry)`.
fn roam_call(ws: &str, sub_args: &[&str], no_cache: bool) -> (Value, Value) {
    let mut args = Vec::with_capacity(sub_args.len() + 5);
    if no_cache {
        args.push("--no-cache");
    }
    args.push("roam");
    args.extend_from_slice(sub_args);
    args.extend_from_slice(&[ws, "--format", "json"]);
    let mut parsed = run_json(&args);
    let tel = parsed.as_object_mut().and_then(|m| m.remove("telemetry"));
    (parsed, tel.unwrap_or(Value::Null))
}

/// Runs `org query search <q> <ws> --format json` with or without `--no-cache`.
fn search_json(ws: &str, q: &str, no_cache: bool) -> Value {
    if no_cache {
        run_json(&["--no-cache", "query", "search", q, ws, "--format", "json"])
    } else {
        run_json(&["query", "search", q, ws, "--format", "json"])
    }
}

/// Seeds a 4-file multi-directory workspace with roam nodes, aliases, links, and edna edges.
fn seed_four_file_workspace(root: &Path) {
    let files = [
        ("specs/alpha.org", ":PROPERTIES:\n:ID: 10000000-0000-4000-8000-000000000001\n:ROAM_ALIASES: \"Alpha Spec\"\n:END:\n#+TITLE: Alpha Architecture\n\n* TODO Alpha Engine\n:PROPERTIES:\n:ID: 10000000-0000-4000-8000-000000000011\n:BLOCKER: ids(\"10000000-0000-4000-8000-000000000021\")\n:END:\nConnects [[id:10000000-0000-4000-8000-000000000002][Beta Doc]] to SearchEngine.\n"),
        ("specs/beta.org", ":PROPERTIES:\n:ID: 10000000-0000-4000-8000-000000000002\n:END:\n#+TITLE: Beta Subsystem\n\n* TODO Beta Prereq\n:PROPERTIES:\n:ID: 10000000-0000-4000-8000-000000000021\n:TRIGGER: ids(\"10000000-0000-4000-8000-000000000011\") todo!(\"NEXT\")\n:END:\nBack-references [[id:10000000-0000-4000-8000-000000000001][Alpha]].\n"),
        ("notes/gamma.org", ":PROPERTIES:\n:ID: 10000000-0000-4000-8000-000000000003\n:END:\n#+TITLE: Gamma Notes\n\n* DONE Gamma Milestone\nBody mentions multi word token.\n"),
        ("notes/delta.org", "* TODO Delta Task :ops:\nStandalone entry linking [[id:10000000-0000-4000-8000-000000000002]].\n"),
    ];
    for (rel, body) in files {
        write_org(&join_slash_rel(root, rel), body);
    }
}

// endregion

// region Corpus-1k Deterministic Generator

/// Formats the body prose for heading `(file_idx, heading_idx)` with seeded `"SearchEngine"` frequencies.
fn corpus_heading_body(file_idx: usize, heading_idx: usize, links: &[String]) -> String {
    let ll = links.join(" and ");
    let prefix = match (file_idx, heading_idx) {
        (42, 1) => "SearchEngine SearchEngine SearchEngine SearchEngine SearchEngine drives telemetry.",
        (314, 2) => "The SearchEngine pipeline validates SearchEngine metrics via SearchEngine gates across modules.",
        (512, 0) => "Calibrates SearchEngine latency budgets across synthetic workspace runs and reports.",
        (777, 2) => "Background operational notes covering archival export workflows, scheduling rules, and one SearchEngine mention in prose.",
        _ => return format!("Synthetic prose body for entry {file_idx}:{heading_idx}. {ll}\n"),
    };
    format!("{prefix} {ll}\n")
}

/// Appends deterministic `:BLOCKER:` and `:TRIGGER:` lines for `(file_idx, h)` in `corpus-1k`.
fn append_corpus_edna(buf: &mut String, file_idx: usize, h: usize) {
    if file_idx < 250 && h == 0 {
        let dep = (file_idx + 1) % CORPUS_1K_FILES;
        let id = format!("10000000-0000-4000-8000-{dep:04}00000000");
        buf.push_str(&format!(":BLOCKER: ids(\"{id}\")\n"));
    } else if (file_idx, h) == (42, 1) {
        buf.push_str(":BLOCKER: ids(\"10000000-0000-4000-8000-001000000000\")\n");
    } else if (file_idx, h) == (314, 2) {
        buf.push_str(":BLOCKER: ids(\"10000000-0000-4000-8000-001100000000\")\n");
    }
    if file_idx < 248 && h == 1 {
        let trig = (file_idx + 2) % CORPUS_1K_FILES;
        let id = format!("10000000-0000-4000-8000-{trig:04}00000000");
        buf.push_str(&format!(":TRIGGER: ids(\"{id}\") todo!(\"NEXT\")\n"));
    }
}

/// Builds a single deterministic `.org` document in `corpus-1k` for `file_idx` (`0..1000`).
fn render_corpus_1k_file(file_idx: usize) -> String {
    let file_id = format!("f0000000-0000-4000-8000-{file_idx:012}");
    let mut buf = format!(":PROPERTIES:\n:ID: {file_id}\n:END:\n#+TITLE: Note {file_idx:04}\n\n");
    for h in 0..3 {
        let hid = format!("10000000-0000-4000-8000-{file_idx:04}{h:08}");
        let kw = ["TODO", "DONE"][usize::from(file_idx % 2 == 1 && h == 0)];
        let kind = ["Heading", "SearchEngine Node"][usize::from((file_idx, h) == (512, 0))];
        let hdr = format!("* {kw} {kind} {file_idx:04}-{h}\n");
        buf.push_str(&format!("{hdr}:PROPERTIES:\n:ID: {hid}\n"));
        append_corpus_edna(&mut buf, file_idx, h);
        buf.push_str(":END:\n");
        let links: Vec<String> = (0..if h < 2 { 2 } else { 1 })
            .map(|l| {
                let ti = (file_idx * 7 + h * 3 + l + 1) % CORPUS_1K_FILES;
                let tgt = if file_idx < 10 && h == 0 && l == 0 {
                    HUB_FILE_UUID.into()
                } else {
                    format!("f0000000-0000-4000-8000-{ti:012}")
                };
                format!("[[id:{tgt}][Ref {l}]]")
            })
            .collect();
        buf.push_str(&corpus_heading_body(file_idx, h, &links));
    }
    buf
}

/// Generates the 1,000-file canonical `corpus-1k` workspace inside `root` and warms OS file caches.
fn generate_corpus_1k(root: &Path) {
    for idx in 0..CORPUS_1K_FILES {
        let rel = format!("dir_{:02}/note_{idx:04}.org", idx / 100);
        write_org(&join_slash_rel(root, &rel), &render_corpus_1k_file(idx));
    }
    let _ = WorkspaceCache::rebuild(root).and_then(|_| WorkspaceCache::clear(root));
}

// endregion

// region Slice 1: SQLite WAL + FTS5 Schema & Cold Index Creation

/// Verifies cold rebuild populates all 5 required SQLite tables and `org cache status|rebuild|clear` + env override work.
#[test]
fn slice1_cold_rebuild_populates_all_five_tables_and_cache_commands() {
    let dir = TempDir::new().unwrap();
    seed_four_file_workspace(dir.path());
    let ws = dir.path().to_str().unwrap();
    let db_path = join_slash_rel(dir.path(), DEFAULT_CACHE_REL_PATH);
    let pre = run_json(&["cache", "status", ws, "--format", "json"]);
    assert!(!pre["exists"].as_bool().unwrap() && !db_path.exists());

    let reb = run_json(&["cache", "rebuild", ws, "--format", "json"]);
    assert!(db_path.is_file());
    assert!(!dir.path().join("org-roam.db").exists());
    assert_eq!(reb["telemetry"]["files_scanned"], 4);
    assert_eq!(hit_miss(&reb["telemetry"]), (0, 4));
    let st = run_json(&["cache", "status", ws, "--format", "json"]);
    assert!(st["exists"] == true && st["size_bytes"].as_u64().unwrap() > 0);
    assert_eq!(st["files_indexed"], 4);
    assert_eq!(st["nodes_count"], 5);
    assert_eq!(st["links_count"], 3);
    assert_eq!(st["edna_edges_count"], 2);
    assert_eq!(st["fts_entries_count"], 4);
    let (cache, _) = WorkspaceCache::open_and_sync(dir.path()).unwrap();
    let tables = cache.table_names().unwrap();
    for t in ["files", "nodes", "links", "edna_edges", "entries_fts"] {
        assert!(tables.iter().any(|x| x == t), "missing table {t}");
    }
    drop(cache);
    run_ok(&["cache", "clear", ws, "--format", "json"]);
    assert!(!db_path.exists());
    let cdir = TempDir::new().unwrap();
    let cdb = cdir.path().join("override-org-cache.db");
    let mut cmd = org();
    cmd.env(CACHE_PATH_ENV_VAR, &cdb);
    cmd.args(["cache", "rebuild", ws, "--format", "json"]);
    cmd.assert().success();
    assert!(cdb.is_file() && !db_path.exists());
}

// endregion

// region Slice 2: Synchronous Incremental Refresh & 100% --no-cache Parity

/// Verifies `--no-cache`, empty-dir `EXIT_ERROR`, symlinks, `.git/info/exclude` subdir, and `.gitignore`.
#[test]
fn slice2_no_cache_and_gitignore_respect_workspace_boundaries() {
    let empty = TempDir::new().unwrap();
    let ews = empty.path().to_str().unwrap();
    run_code(&["query", "search", "body:probe", ews], 2);
    assert!(!empty.path().join(".cache").exists());

    let (ext, dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let r = dir.path();
    write_org(&ext.path().join("shared.org"), ":PROPERTIES:\n:ID: ext-01\n:END:\n#+TITLE: Shared Ext\n* TODO Ext Heading\nShared prose.\n");
    seed_four_file_workspace(r);
    #[cfg(unix)]
    for (src, dst) in [
        (ext.path().join("shared.org"), r.join("specs/ext_link.org")),
        (r.join("specs/alpha.org"), r.join("specs/alias_alpha.org")),
    ] {
        std::os::unix::fs::symlink(src, dst).unwrap();
    }
    let exp_roam = if cfg!(unix) { 5 } else { 4 };
    for (rel, body) in [
        (".git/HEAD", "ref: refs/heads/main\n"),
        (".git/ghost.org", "* TODO Git Internal\n"),
        (".git/info/exclude", "specs/excluded.org\n"),
        (".gitignore", "ignored/\n"),
        ("ignored/secret.org", "* TODO Ignored Entry\n"),
        ("specs/excluded.org", "* TODO Info Excluded\n"),
    ] {
        write_org(&join_slash_rel(r, rel), body);
    }
    let (ws, specs_ws) = (r.to_str().unwrap(), r.join("specs"));
    let sws = specs_ws.to_str().unwrap();

    let (_, tel) = roam_call(ws, &["node", "Alpha Spec"], true);
    assert!(!join_slash_rel(r, DEFAULT_CACHE_REL_PATH).exists() && hit_miss(&tel) == (0, exp_roam));
    let sg = roam_call(sws, &["graph"], false).0;
    assert_eq!(sg, roam_call(sws, &["graph"], true).0);

    let (wg_c, st) = roam_call(ws, &["graph"], false);
    let scan = st["files_scanned"].as_u64();
    assert_eq!((scan, hit_miss(&st)), (Some(exp_roam), (0, exp_roam)));
    assert_eq!(wg_c, roam_call(ws, &["graph"], true).0);
    let q = "body:\"SearchEngine\"";
    assert_eq!(search_json(ws, q, false), search_json(ws, q, true));
}

/// Asserts 100% roam & query output parity between warm cache and `--no-cache` on `ws`.
fn assert_full_parity(ws: &str) {
    let check_roam = |a: &[&str]| assert_eq!(roam_call(ws, a, false).0, roam_call(ws, a, true).0);
    check_roam(&["node", "Alpha Spec"]);
    check_roam(&["backlinks", "id:10000000-0000-4000-8000-000000000002"]);
    check_roam(&["links", "id:10000000-0000-4000-8000-000000000011"]);
    check_roam(&["graph"]);
    for fmt in ["mermaid", "dot"] {
        let g1 = run_ok(&["roam", "graph", ws, "--format", fmt]);
        assert_eq!(
            g1,
            run_ok(&["--no-cache", "roam", "graph", ws, "--format", fmt])
        );
    }
    for q in ["todo:TODO", "body:\"SearchEngine\""] {
        assert_eq!(search_json(ws, q, false), search_json(ws, q, true));
        let l1 = run_ok(&["query", "search", q, ws, "--format", "locator"]);
        let l2 = run_ok(&["--no-cache", "query", "search", q, ws, "--format=locator"]);
        assert_eq!(l1, l2);
    }
}

/// Verifies single-file modify, same-hash touch, add, and delete re-index only changed files with 100% parity.
#[test]
fn slice2_incremental_modify_add_delete_reindexes_single_file_with_full_parity() {
    let dir = TempDir::new().unwrap();
    seed_four_file_workspace(dir.path());
    let ws = dir.path().to_str().unwrap();
    assert_eq!(hit_miss(&roam_call(ws, &["graph"], false).1), (0, 4));
    assert_eq!(hit_miss(&roam_call(ws, &["graph"], false).1), (4, 0));
    assert_full_parity(ws);

    let rewrite_bump = |p: &Path, text: &str| {
        let m = fs::metadata(p).unwrap().modified().unwrap() + std::time::Duration::from_secs(2);
        write_org(p, text);
        let f = fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(m).unwrap();
    };
    let gamma = ":PROPERTIES:\n:ID: 10000000-0000-4000-8000-000000000003\n:END:\n#+TITLE: Gamma Revised\n* TODO Gamma Updated\nLinks [[id:10000000-0000-4000-8000-000000000002][Beta]] and SearchEngine SearchEngine.\n";
    rewrite_bump(&join_slash_rel(dir.path(), "notes/gamma.org"), gamma);
    assert_eq!(hit_miss(&roam_call(ws, &["graph"], false).1), (3, 1));
    assert_full_parity(ws);

    let delta_p = join_slash_rel(dir.path(), "notes/delta.org");
    let existing = fs::read_to_string(&delta_p).unwrap();
    rewrite_bump(&delta_p, &existing);
    assert_eq!(hit_miss(&roam_call(ws, &["graph"], false).1), (4, 0));

    let eps = ":PROPERTIES:\n:ID: 10000000-0000-4000-8000-000000000005\n:END:\n#+TITLE: Epsilon\n* TODO Epsilon SearchEngine\n";
    let eps_p = join_slash_rel(dir.path(), "notes/epsilon.org");
    write_org(&eps_p, eps);
    assert_eq!(hit_miss(&roam_call(ws, &["graph"], false).1), (4, 1));
    assert_full_parity(ws);

    fs::remove_file(&eps_p).unwrap();
    let (_, del_tel) = roam_call(ws, &["graph"], false);
    let dscan = del_tel["files_scanned"].as_u64();
    assert_eq!((dscan, hit_miss(&del_tel)), (Some(4), (4, 0)));
    assert_full_parity(ws);
}

// endregion

// region Slice 3: body:"..." Query Predicate, BM25 Ranking & corpus-1k Benchmark

/// Verifies `body:` sanitization, NFD combining marks, diacritics, and `:ROAM_EXCLUDE:`/`olp` blockers.
#[test]
fn slice3_body_phrase_special_chars_and_compound_query_parity() {
    let dir = TempDir::new().unwrap();
    let r = dir.path();
    let p_body = "* TODO H\n:PROPERTIES:\n:ID: excl-99\n:ROAM_EXCLUDE: t\n:END:\n* TODO OlpDep\n";
    write_org(&r.join("p.org"), p_body);
    write_org(&r.join("s.org"), "* TODO First\n:PROPERTIES:\n:BLOCKER: ids(\"excl-99\")\n:END:\nBody alpha-beta:gamma * AND OR \"quoted\" Ümlaut e\u{0301}cole Hello \u{0301} world हिन्दी.\n* DONE Second\nBody alpha-beta:gamma * AND OR Umlaut e and cole हिन्दी हिन्दी किताबें विज्ञान प्रणाली भाषाएँ विकास संरचना अनुसंधान मूल्यांकन।\n");
    write_org(
        &r.join("o.org"),
        "#+PROPERTY: BLOCKER olp(\"p.org\" \"OlpDep\")\n* TODO Third\nBody Ümlaut e\u{0301}cole.\n",
    );
    let (ws, mark) = (r.to_str().unwrap(), "body:\"\u{0301}\"");
    run_code(&["--no-cache", "query", "search", mark, ws], 1);
    assert!(!r.join(".cache").exists());
    run_ok(&["cache", "rebuild", ws]);
    run_code(&["query", "search", mark, ws], 1);
    let qs = "body:\"alpha-beta:gamma\"|body:\"AND OR\"|body:Ümlaut|body:e\u{0301}cole|body:\"hello world\"|body:हिन्दी|todo:TODO body:\"alpha-beta\"";
    for q in qs.split('|') {
        assert_eq!(search_json(ws, q, false), search_json(ws, q, true));
    }
}

/// Verifies `resolve_perf_budgets` enforces release SLOs in `--release` or `ORGFMT_STRICT_PERF_BUDGET=1` and debug ceilings otherwise.
#[test]
fn slice3_resolve_perf_budgets_enforces_release_slos_and_debug_ceilings() {
    let strict = (COLD_REBUILD_BUDGET_US, WARM_QUERY_BUDGET_US);
    let debug = (DEBUG_COLD_REBUILD_BUDGET_US, DEBUG_WARM_QUERY_BUDGET_US);
    assert_eq!(resolve_perf_budgets(true, None), strict);
    assert_eq!(resolve_perf_budgets(true, Some("0")), strict);
    assert_eq!(resolve_perf_budgets(false, Some("1")), strict);
    assert_eq!(resolve_perf_budgets(false, None), debug);
    assert_eq!(resolve_perf_budgets(false, Some("0")), debug);
}

/// Asserts warm FTS5 BM25 and warm roam backlinks stay within the active warm query budget on `corpus-1k`.
fn assert_corpus_1k_warm_latencies(root: &Path, hub_q: &str) -> (u128, u128) {
    let (_, warm_budget_us) = active_perf_budgets();
    let (warmup, runs, p95) = BENCH_WARMUP_MEASURED_P95;
    let terms = vec!["SearchEngine".to_string()];
    let exp = "dir_00/note_0042.org,dir_03/note_0314.org,dir_05/note_0512.org,dir_07/note_0777.org";
    let (cache, ft) = WorkspaceCache::open_and_sync(root).unwrap();
    assert_eq!((ft.cache_hits, ft.cache_misses), (CORPUS_1K_FILES, 0));
    let (mut fts_s, mut roam_s) = (Vec::with_capacity(runs), Vec::with_capacity(runs));
    for iter in 0..(warmup + runs) {
        let t0 = Instant::now();
        let hits = cache.search_fts_bm25(&terms).unwrap();
        let fts_us = t0.elapsed().as_micros();
        let joined: Vec<&str> = hits.iter().map(|h| h.rel_path.as_str()).collect();
        assert_eq!(joined.join(","), exp);
        let t1 = Instant::now();
        let nodes = cache.resolve_roam_nodes(root, hub_q, true).unwrap();
        let bl = cache.roam_backlinks_for(root, &nodes).unwrap();
        let roam_us = t1.elapsed().as_micros();
        assert_eq!(bl.len(), 15);
        if iter >= warmup {
            fts_s.push(fts_us);
            roam_s.push(roam_us);
        }
    }
    fts_s.sort_unstable();
    roam_s.sort_unstable();
    assert!(fts_s[p95] <= warm_budget_us, "{fts_s:?} > {warm_budget_us}");
    let rp = roam_s[p95];
    assert!(rp <= warm_budget_us, "{roam_s:?} > {warm_budget_us}");
    (fts_s[p95], rp)
}

/// Verifies `corpus-1k` cold rebuild, warm FTS5 BM25 & backlinks within active budgets, and BM25 parity.
#[test]
fn slice3_corpus_1k_cold_rebuild_warm_fts_bm25_and_roam_within_budget() {
    let (cold_budget_us, _) = active_perf_budgets();
    let dir = TempDir::new().unwrap();
    generate_corpus_1k(dir.path());
    let ws = dir.path().to_str().unwrap();
    let hub_q = format!("id:{HUB_FILE_UUID}");
    let cold_start = Instant::now();
    let (_, st) = WorkspaceCache::rebuild(dir.path()).expect("cold rebuild must succeed");
    let cold_us = cold_start.elapsed().as_micros();
    assert!(cold_us <= cold_budget_us, "{cold_us} > {cold_budget_us}");
    let (fi, nc, lc) = (st.files_indexed, st.nodes_count, st.links_count);
    let got = (fi, nc, lc, st.edna_edges_count, st.fts_entries_count);
    assert_eq!(got, CORPUS_1K_ROW_COUNTS);

    let (fts_us, roam_us) = assert_corpus_1k_warm_latencies(dir.path(), &hub_q);
    eprintln!("[corpus-1k] cold_rebuild={cold_us}us warm_fts_bm25={fts_us}us warm_roam_backlinks={roam_us}us");

    let (bl_c, _) = roam_call(ws, &["backlinks", &hub_q], false);
    assert_eq!(bl_c, roam_call(ws, &["backlinks", &hub_q], true).0);
    let cli_cached = search_json(ws, "body:\"SearchEngine\"", false);
    assert_eq!(cli_cached, search_json(ws, "body:\"SearchEngine\"", true));
    let entries = cli_cached.as_array().unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0]["blocked"].as_bool(), Some(true));
    assert_eq!(entries[1]["blocked"].as_bool(), Some(false));
}

// endregion
