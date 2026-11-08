// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Unit and workspace parity tests for `WorkspaceCache`, walker rules, `unicode61`, and `org-edna`.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

#[cfg(unix)]
use super::RoamWorkspace;
use super::{
    build_fts5_match_expr, load_cached_fts_candidate_docs, load_docs_with_bodies,
    matches_body_phrase, rank_matched_indices_bm25, tokenize_unicode61, WorkspaceCache,
    DEFAULT_CACHE_REL_PATH,
};
use crate::edna::{self, EdnaContext};
use crate::files::{collect_org_files, join_slash_rel};

/// Writes UTF-8 Org content to `path`, creating parent directories when needed.
fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, content).expect("write fixture file");
}

/// Verifies `:ROAM_EXCLUDE: t` `:ID:` targets, `olp(...)`, and `#+PROPERTY: BLOCKER` match `--no-cache`.
#[test]
fn edna_roam_excluded_id_olp_and_default_property_blockers_match_uncached() {
    let dir = TempDir::new().unwrap();
    write_file(
        &dir.path().join("excluded_prereq.org"),
        "* TODO Hidden Prereq\n:PROPERTIES:\n:ID: excl-0001\n:ROAM_EXCLUDE: t\n:END:\n",
    );
    write_file(&dir.path().join("olp_prereq.org"), "* TODO Olp Prereq\n");
    write_file(
        &dir.path().join("hit_excluded_id.org"),
        "* TODO Blocked By Excluded ID\n:PROPERTIES:\n:BLOCKER: ids(\"excl-0001\")\n:END:\nSignalToken alpha.\n",
    );
    write_file(
        &dir.path().join("hit_default_olp.org"),
        "#+PROPERTY: BLOCKER olp(\"olp_prereq.org\" \"Olp Prereq\")\n* TODO Blocked By Default Olp\nSignalToken beta.\n",
    );

    let (cache, _) = WorkspaceCache::open_and_sync(dir.path()).unwrap();
    let graph = cache.load_roam_graph(dir.path(), 4).unwrap();
    assert!(
        graph.nodes.is_empty(),
        "ROAM_EXCLUDE node must stay hidden from roam graph"
    );

    let id_plan =
        load_cached_fts_candidate_docs(&cache, dir.path(), &["alpha".to_string()]).unwrap();
    let (_, loaded) = id_plan.expect("ids() blocker plan must stay on fast candidate path");
    let refs: Vec<_> = loaded.iter().map(|d| &d.doc).collect();
    let hit_doc = loaded
        .iter()
        .find(|d| d.doc.file.ends_with("hit_excluded_id.org"))
        .unwrap();
    assert!(edna::is_blocked(&EdnaContext {
        all_docs: &refs,
        doc: &hit_doc.doc,
        entry_idx: 0,
    }));

    let olp_plan =
        load_cached_fts_candidate_docs(&cache, dir.path(), &["beta".to_string()]).unwrap();
    assert!(
        olp_plan.is_none(),
        "default #+PROPERTY: BLOCKER olp(...) must trigger full-doc fallback"
    );
}

/// Verifies walker parity for standalone `.gitignore` (no `.git`) and `<repo>/specs` (ancestor `.git` + `.git/info/exclude`).
#[test]
fn walker_matches_collect_org_files_across_git_subdir_and_standalone_dir() {
    let standalone = TempDir::new().unwrap();
    write_file(&standalone.path().join(".gitignore"), "ignored/\n");
    write_file(&standalone.path().join("visible.org"), "* TODO Visible\n");
    write_file(
        &join_slash_rel(standalone.path(), "ignored/included_without_git.org"),
        "* TODO Included Outside Git\n",
    );
    let direct_standalone = collect_org_files(&[standalone.path().to_path_buf()]);
    let (_, st_tel) = WorkspaceCache::open_and_sync(standalone.path()).unwrap();
    assert_eq!(st_tel.files_scanned, direct_standalone.len());

    let repo = TempDir::new().unwrap();
    write_file(
        &join_slash_rel(repo.path(), ".git/HEAD"),
        "ref: refs/heads/main\n",
    );
    write_file(
        &join_slash_rel(repo.path(), ".git/info/exclude"),
        "specs/info_excluded.org\n",
    );
    write_file(&repo.path().join(".gitignore"), "build/\n");
    let specs = repo.path().join("specs");
    write_file(&specs.join("kept.org"), "* TODO Kept\n");
    write_file(&specs.join("info_excluded.org"), "* TODO Excluded\n");
    write_file(
        &join_slash_rel(&specs, "build/generated.org"),
        "* TODO Generated\n",
    );
    write_file(
        &join_slash_rel(&specs, ".cache/stale.org"),
        "* TODO Cache Internal\n",
    );

    let direct_specs: Vec<PathBuf> = collect_org_files(std::slice::from_ref(&specs));
    let (_, repo_tel) = WorkspaceCache::open_and_sync(&specs).unwrap();
    assert_eq!(direct_specs.len(), 1);
    assert_eq!(repo_tel.files_scanned, direct_specs.len());
}

/// Verifies `unicode61 remove_diacritics 0` preserves NFD combining marks and diacritic IDF distinction.
#[test]
fn unicode61_preserves_nfd_combining_marks_and_diacritic_bm25_idf() {
    let nfd = "e\u{0301}cole";
    assert_eq!(tokenize_unicode61(nfd), vec![nfd.to_string()]);
    assert_eq!(
        build_fts5_match_expr(&[nfd.to_string()]),
        Some(format!("\"{nfd}\""))
    );

    let dir = TempDir::new().unwrap();
    write_file(
        &dir.path().join("school.org"),
        &format!("* TODO NFD Match\nVisits {nfd} daily.\n* TODO Separate Letters\nLetter e and cole apart.\n"),
    );
    write_file(
        &dir.path().join("diacritics.org"),
        "* TODO Exact Diaeresis\nÜmlaut rare.\n* TODO Plain Ascii\nUmlaut filler.\n",
    );
    let (cache, _) = WorkspaceCache::open_and_sync(dir.path()).unwrap();
    let nfd_hits = cache.search_fts_bm25(&[nfd.to_string()]).unwrap();
    assert_eq!(nfd_hits.len(), 1);
    assert_eq!(nfd_hits[0].entry_idx, 0);

    let umlaut_hits = cache.search_fts_bm25(&["Ümlaut".to_string()]).unwrap();
    assert_eq!(
        umlaut_hits.len(),
        1,
        "remove_diacritics 0 must not match plain Umlaut"
    );
}

/// Verifies `open_and_sync` on an empty directory does not create `.cache/` or `org-cache.db`.
#[test]
fn empty_directory_open_and_sync_does_not_create_cache_files() {
    let empty = TempDir::new().unwrap();
    let (_, tel) = WorkspaceCache::open_and_sync(empty.path()).unwrap();
    assert_eq!(tel.files_scanned, 0);
    assert!(!empty.path().join(".cache").exists());
    assert!(!join_slash_rel(empty.path(), DEFAULT_CACHE_REL_PATH).exists());
}

/// Verifies Git-repo scanning follows symlinked `.org` files to external targets while skipping broken symlinks.
#[cfg(unix)]
#[test]
fn git_repo_follows_external_org_symlink_and_skips_broken_symlink() {
    use std::os::unix::fs::symlink;

    let outer = TempDir::new().unwrap();
    let ext = outer.path().join("ext/shared.org");
    write_file(
        &ext,
        ":PROPERTIES:\n:ID: ext-shared-01\n:END:\n#+TITLE: Shared Note\n* TODO External Task\nShared body.\n",
    );
    let repo = outer.path().join("repo");
    write_file(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
    write_file(&repo.join("local.org"), "* TODO Local Task\nLocal body.\n");
    symlink(&ext, repo.join("linked.org")).unwrap();
    symlink(
        outer.path().join("ext/missing.org"),
        repo.join("broken.org"),
    )
    .unwrap();

    let direct = collect_org_files(std::slice::from_ref(&repo));
    assert_eq!(direct.len(), 2);
    let (cache, tel) = WorkspaceCache::open_and_sync(&repo).unwrap();
    assert_eq!(tel.files_scanned, 2);
    let hits = cache.search_fts_bm25(&["Shared".to_string()]).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].rel_path, "linked.org");
}

/// Verifies intra-workspace symlinks deduplicate `RoamWorkspace` (`1` node, `files_scanned: 1`) while keeping both files in FTS5.
#[cfg(unix)]
#[test]
fn intra_workspace_symlink_deduplicates_roam_while_preserving_query_files() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_path_buf();
    write_file(
        &ws.join("real.org"),
        ":PROPERTIES:\n:ID: sym-node-01\n:END:\n#+TITLE: Real Node\n* TODO Symlinked Heading\nLinks [[id:sym-node-02]] with SignalWord.\n",
    );
    write_file(
        &ws.join("peer.org"),
        ":PROPERTIES:\n:ID: sym-node-02\n:END:\n#+TITLE: Peer Node\n* TODO Peer\nLinks [[id:sym-node-01]].\n",
    );
    symlink(ws.join("real.org"), ws.join("alias.org")).unwrap();

    let uncached = RoamWorkspace::open(std::slice::from_ref(&ws), true).unwrap();
    let cached_cold = RoamWorkspace::open(std::slice::from_ref(&ws), false).unwrap();
    assert_eq!(cached_cold.counts(), (2, 0, 2));
    assert_eq!(uncached.counts(), (2, 0, 2));
    assert_eq!(
        cached_cold.resolve("Real Node").unwrap(),
        uncached.resolve("Real Node").unwrap()
    );
    let c_nodes = cached_cold.resolve("id:sym-node-01").unwrap();
    assert_eq!(c_nodes.len(), 1);
    assert_eq!(
        cached_cold.backlinks(&c_nodes).unwrap(),
        uncached.backlinks(&c_nodes).unwrap()
    );
    assert_eq!(
        cached_cold.outgoing(&c_nodes).unwrap(),
        uncached.outgoing(&c_nodes).unwrap()
    );
    let (cg, cc) = cached_cold.into_graph().unwrap();
    let (ug, uc) = uncached.into_graph().unwrap();
    assert_eq!((cc.0, cg.nodes, cg.links), (uc.0, ug.nodes, ug.links));

    let cached_warm = RoamWorkspace::open(std::slice::from_ref(&ws), false).unwrap();
    assert_eq!(cached_warm.counts(), (2, 2, 0));

    let (mcache, _) = WorkspaceCache::open_and_sync(&ws).unwrap();
    assert_eq!(
        mcache
            .search_fts_bm25(&["SignalWord".into()])
            .unwrap()
            .len(),
        2
    );
}

/// Verifies SQLite `fts5vocab` tokenization and BM25 parity on standalone combining marks and Devanagari (`हिन्दी`).
#[test]
fn fts5vocab_matches_standalone_combining_mark_and_devanagari_bm25() {
    assert!(tokenize_unicode61("\u{0301}").is_empty());
    assert_eq!(build_fts5_match_expr(&["\u{0301}".to_string()]), None);
    assert!(!matches_body_phrase(
        "Title",
        "Hello \u{0301} world",
        "\u{0301}"
    ));
    assert_eq!(
        tokenize_unicode61("Hello \u{0301} world"),
        vec!["hello".to_string(), "world".to_string()]
    );

    let dir = TempDir::new().unwrap();
    let hindi = "हिन्दी";
    write_file(
        &dir.path().join("a.org"),
        &format!("* TODO Short Doc\n{hindi} alpha beta gamma delta.\n"),
    );
    write_file(
        &dir.path().join("b.org"),
        &format!(
            "* TODO Devanagari Heavy\n{hindi} {hindi} किताबें विज्ञान प्रणाली भाषाएँ विकास संरचना अनुसंधान मूल्यांकन अभियांत्रिकी।\n"
        ),
    );
    let (cache, _) = WorkspaceCache::open_and_sync(dir.path()).unwrap();
    assert!(cache
        .search_fts_bm25(&["\u{0301}".into()])
        .unwrap()
        .is_empty());

    let fts_hits = cache.search_fts_bm25(&[hindi.to_string()]).unwrap();
    let files = collect_org_files(&[dir.path().to_path_buf()]);
    let loaded = load_docs_with_bodies(&files);
    let pairs = vec![(0, 0), (1, 0)];
    let ranked = rank_matched_indices_bm25(&loaded, &pairs, &[hindi.to_string()]);
    let cached_order: Vec<_> = fts_hits
        .iter()
        .map(|h| join_slash_rel(dir.path(), &h.rel_path))
        .collect();
    let uncached_order: Vec<_> = ranked
        .iter()
        .map(|&(d, _)| loaded[d].doc.file.clone())
        .collect();
    assert_eq!(cached_order, uncached_order);
}
