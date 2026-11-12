// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Unit tests for the `edna` module across parser, multi-finder scoping,
//! conditions, trigger cascades, and `:BLOCKER:` cycle detection.

use std::collections::HashMap;
use std::path::PathBuf;

use tempfile::TempDir;

use crate::document::{OrgDocument, OrgEntry, Planning};

use super::*;

// region Test Fixtures

/// Builds a synthetic [`OrgDocument`] from `(title, keyword, parent)` tuples.
fn make_doc_with_entries(entries_data: &[(&str, Option<&str>, Option<usize>)]) -> OrgDocument {
    let mut entries = Vec::new();
    for (i, (title, keyword, parent)) in entries_data.iter().enumerate() {
        entries.push(OrgEntry {
            level: if parent.is_some() { 2 } else { 1 },
            keyword: keyword.map(str::to_string),
            priority: None,
            title: (*title).to_string(),
            tags: Vec::new(),
            properties: HashMap::new(),
            planning: Planning::default(),
            clocks: Vec::new(),
            checklist: Vec::new(),
            vulpea_meta: HashMap::new(),
            heading_line: i + 1,
            heading_offset: 0,
            content_end_line: i + 2,
            subtree_end_line: i + 2,
            parent: *parent,
            children: Vec::new(),
            raw_heading: format!("* {title}"),
        });
    }
    for i in 0..entries.len() {
        if let Some(parent_idx) = entries[i].parent {
            entries[parent_idx].children.push(i);
        }
    }

    OrgDocument {
        file: PathBuf::from("test.org"),
        entries,
        file_properties: HashMap::new(),
        file_properties_line: None,
        file_keywords: HashMap::new(),
        todo_keywords: Default::default(),
        priority_range: Default::default(),
        filetags: Vec::new(),
        default_properties: HashMap::new(),
        tag_spec: Default::default(),
        link_abbreviations: HashMap::new(),
        table_constants: HashMap::new(),
        id_index: HashMap::new(),
        custom_id_index: HashMap::new(),
        vulpea_meta: HashMap::new(),
    }
}

// endregion

// region Parser & Formatter Unit Tests

/// Verifies parsing a zero-argument structural finder.
#[test]
fn parse_simple_finder() {
    let (exprs, errors) = parse_edna("next-sibling");
    assert!(errors.is_empty());
    assert_eq!(exprs, vec![EdnaExpr::Finder(Finder::NextSibling)]);
}

/// Verifies parsing both quoted and bare-word `ids(...)` arguments.
#[test]
fn parse_ids_finder() {
    let expected = vec![EdnaExpr::Finder(Finder::Ids(vec![
        "abc-123".to_string(),
        "def-456".to_string(),
    ]))];
    for raw in ["ids(\"abc-123\" \"def-456\")", "ids(abc-123 def-456)"] {
        let (exprs, errors) = parse_edna(raw);
        assert!(errors.is_empty());
        assert_eq!(exprs, expected);
    }
}

/// Verifies parsing `todo!("DONE")` and two-argument `set-property!("KEY" "VALUE")` actions.
#[test]
fn parse_action() {
    let (exprs, errors) = parse_edna("todo!(\"DONE\") set-property!(\"KEY\" \"VALUE\")");
    assert!(errors.is_empty());
    assert_eq!(
        exprs,
        vec![
            EdnaExpr::Action(Action::Todo("DONE".into())),
            EdnaExpr::Action(Action::SetProperty("KEY".into(), "VALUE".into())),
        ]
    );
}

/// Verifies parsing `todo-state?("DONE")` and negated `!done?` conditions.
#[test]
fn parse_condition_and_negation() {
    let (exprs, errors) = parse_edna("todo-state?(\"DONE\") !done? !has-property?(\"K\" \"V\")");
    assert!(errors.is_empty(), "errors: {errors:?}");
    assert_eq!(
        exprs,
        vec![
            EdnaExpr::Condition(Condition::TodoState("DONE".to_string())),
            EdnaExpr::Condition(Condition::Negated(Box::new(Condition::Done))),
            EdnaExpr::Condition(Condition::Negated(Box::new(Condition::HasProperty(
                "K".to_string(),
                "V".to_string()
            )))),
        ]
    );
}

/// Verifies parsing sequential finder and action expressions.
#[test]
fn parse_multiple_exprs() {
    let (exprs, errors) = parse_edna("ids(\"uuid-1\") todo!(\"DONE\")");
    assert!(errors.is_empty());
    assert_eq!(exprs.len(), 2);
    assert!(matches!(exprs[0], EdnaExpr::Finder(Finder::Ids(_))));
    assert!(matches!(exprs[1], EdnaExpr::Action(Action::Todo(_))));
}

/// Verifies parsing `match("work")` tag finders.
#[test]
fn parse_match_finder() {
    let (exprs, errors) = parse_edna("match(\"work\")");
    assert!(errors.is_empty());
    assert_eq!(
        exprs,
        vec![EdnaExpr::Finder(Finder::Match("work".to_string()))]
    );
}

/// Verifies parsing `if ... then ... endif` conditional blocks.
#[test]
fn parse_if_block() {
    let (exprs, errors) =
        parse_edna("if todo-state?(\"DONE\") then next-sibling todo!(\"TODO\") endif");
    assert!(errors.is_empty(), "errors: {errors:?}");
    assert_eq!(exprs.len(), 1);
    match &exprs[0] {
        EdnaExpr::If { cond, then, else_ } => {
            assert!(matches!(
                cond.as_ref(),
                EdnaExpr::Condition(Condition::TodoState(_))
            ));
            assert_eq!(then.len(), 2);
            assert!(else_.is_empty());
        }
        _ => panic!("expected If expression"),
    }
}

/// Verifies parsing `if ... then ... else ... endif` blocks.
#[test]
fn parse_if_else_block() {
    let (exprs, errors) =
        parse_edna("if todo-state?(\"DONE\") then todo!(\"TODO\") else todo!(\"WAITING\") endif");
    assert!(errors.is_empty(), "errors: {errors:?}");
    assert_eq!(exprs.len(), 1);
    match &exprs[0] {
        EdnaExpr::If { then, else_, .. } => {
            assert_eq!(then.len(), 1);
            assert_eq!(else_.len(), 1);
        }
        _ => panic!("expected If expression"),
    }
}

/// Verifies unknown finders, actions, conditions, and unclosed parens emit [`EdnaParseError`]s.
#[test]
fn parse_invalid_syntax_reports_errors() {
    assert_eq!(parse_edna("nonexistent-finder").1.len(), 1);
    assert_eq!(parse_edna("nonexistent!(\"arg\")").1.len(), 1);
    assert_eq!(parse_edna("nonexistent?(\"arg\")").1.len(), 1);
    assert_eq!(parse_edna("ids(\"abc\"").1.len(), 1);
    assert_eq!(parse_edna("ids()").1.len(), 1);
}

/// Verifies zero-argument `archive!`, `self`, and structural finders parse cleanly.
#[test]
fn parse_zero_arg_actions_and_structural_finders() {
    let (arch, arch_err) = parse_edna("archive!");
    assert!(arch_err.is_empty());
    assert_eq!(arch, vec![EdnaExpr::Action(Action::Archive)]);

    for keyword in &[
        "self",
        "parent",
        "ancestors",
        "children",
        "descendants",
        "siblings",
        "first-child",
    ] {
        let (exprs, errors) = parse_edna(keyword);
        assert!(errors.is_empty(), "error parsing {keyword}: {errors:?}");
        assert_eq!(exprs.len(), 1);
    }
}

/// Verifies `extract_dependency_ids` collects IDs from top-level and `if`-branch finders.
#[test]
fn extract_ids_from_blocker_and_if_branches() {
    let ids = extract_dependency_ids("ids(\"abc\" \"def\") if done? then ids(\"ghi\") endif");
    assert_eq!(ids, vec!["abc", "def", "ghi"]);
    assert!(extract_dependency_ids("next-sibling").is_empty());
}

// endregion

// region Blocker Scoping & Condition Evaluation Tests

/// Verifies entries without `:BLOCKER:` report unblocked and empty details.
#[test]
fn not_blocked_without_blocker_property() {
    let doc = make_doc_with_entries(&[("Task A", Some("TODO"), None)]);
    let docs: Vec<&OrgDocument> = vec![&doc];
    let ctx = EdnaContext {
        all_docs: &docs,
        doc: &doc,
        entry_idx: 0,
    };
    assert!(!is_blocked(&ctx));
    assert!(blocking_details(&ctx).is_empty());
}

/// Verifies `ids(...)` blockers check done state across single and multiple files.
#[test]
fn blocker_single_and_cross_file_evaluation() {
    let mut doc1 = make_doc_with_entries(&[("Dep", Some("TODO"), None)]);
    doc1.entries[0]
        .properties
        .insert("ID".to_string(), "dep-1".to_string());

    let mut doc2 = make_doc_with_entries(&[("Blocked", Some("TODO"), None)]);
    doc2.entries[0]
        .properties
        .insert("BLOCKER".to_string(), "ids(\"dep-1\")".to_string());

    let docs = vec![&doc1, &doc2];
    let ctx = EdnaContext {
        all_docs: &docs,
        doc: &doc2,
        entry_idx: 0,
    };
    assert!(is_blocked(&ctx));
    assert_eq!(blocking_details(&ctx)[0].condition_desc, "must be done");

    doc1.entries[0].keyword = Some("DONE".to_string());
    let docs_done = vec![&doc1, &doc2];
    let ctx_done = EdnaContext {
        all_docs: &docs_done,
        doc: &doc2,
        entry_idx: 0,
    };
    assert!(!is_blocked(&ctx_done));
}

/// Verifies consecutive finders union targets rather than overwriting earlier finders.
#[test]
fn consecutive_finders_union_and_reset_after_condition() {
    let mut doc = make_doc_with_entries(&[
        ("A", Some("TODO"), None),
        ("B", Some("DONE"), None),
        ("Target", Some("TODO"), None),
    ]);
    doc.entries[0]
        .properties
        .insert("ID".to_string(), "a".to_string());
    doc.entries[1]
        .properties
        .insert("ID".to_string(), "b".to_string());
    doc.entries[2]
        .properties
        .insert("BLOCKER".to_string(), "ids(\"a\") ids(\"b\")".to_string());

    let docs = vec![&doc];
    let ctx = EdnaContext {
        all_docs: &docs,
        doc: &doc,
        entry_idx: 2,
    };
    let details = blocking_details(&ctx);
    assert_eq!(details.len(), 1);
    assert_eq!(details[0].title, "A");
}

/// Verifies `previous-sibling` and explicit conditions produce accurate `BlockerDetail` items.
#[test]
fn sibling_and_explicit_condition_details() {
    let mut doc = make_doc_with_entries(&[
        ("First", Some("WAITING"), None),
        ("Second", Some("TODO"), None),
    ]);
    doc.entries[1].properties.insert(
        "BLOCKER".to_string(),
        "previous-sibling todo-state?(\"DONE\")".to_string(),
    );
    let docs = vec![&doc];
    let ctx = EdnaContext {
        all_docs: &docs,
        doc: &doc,
        entry_idx: 1,
    };
    let details = blocking_details(&ctx);
    assert_eq!(details.len(), 1);
    assert_eq!(details[0].condition_desc, "must have state DONE");
}

// endregion

// region Graph & Cascade Unit Tests

/// Verifies symmetric `merge_blocker_id`, `remove_blocker_id`, `merge_trigger_id`, and `remove_trigger_id`.
#[test]
fn symmetric_blocker_and_trigger_algebra() {
    let b1 = merge_blocker_id(None, "id-1");
    assert_eq!(b1, "ids(\"id-1\")");
    let b2 = merge_blocker_id(Some(&b1), "id-2");
    assert_eq!(b2, "ids(\"id-1\" \"id-2\")");
    assert_eq!(merge_blocker_id(Some(&b2), "id-1"), b2);
    assert_eq!(
        remove_blocker_id(Some(&b2), "id-1"),
        Some("ids(\"id-2\")".to_string())
    );
    assert_eq!(remove_blocker_id(Some("ids(\"id-2\")"), "id-2"), None);

    let t1 = merge_trigger_id(None, "w-1", "NEXT");
    assert_eq!(t1, "ids(\"w-1\") todo!(\"NEXT\")");
    let t2 = merge_trigger_id(Some(&t1), "w-2", "NEXT");
    assert_eq!(t2, "ids(\"w-1\" \"w-2\") todo!(\"NEXT\")");
    assert_eq!(
        remove_trigger_id(Some(&t2), "w-1"),
        Some("ids(\"w-2\") todo!(\"NEXT\")".to_string())
    );
    assert_eq!(
        remove_trigger_id(Some("ids(\"w-2\") todo!(\"NEXT\")"), "w-2"),
        None
    );
}

/// Verifies cycle detection and `WorkspaceOverlay` trigger cascade execution.
#[test]
fn cycle_detection_and_overlay_trigger_cascade() {
    let dir = TempDir::new().unwrap();
    let f_path = dir.path().join("tasks.org");
    std::fs::write(
        &f_path,
        concat!(
            "* DONE Trigger Src\n:PROPERTIES:\n:ID: t-1\n:TRIGGER: ids(\"t-2\") todo!(\"NEXT\")\n:END:\n",
            "* TODO Trigger Tgt\n:PROPERTIES:\n:ID: t-2\n:BLOCKER: ids(\"t-1\")\n:END:\n",
        ),
    )
    .unwrap();

    let mut overlay = WorkspaceOverlay::load_workspace(dir.path());
    let docs = overlay.parse_documents();
    let refs: Vec<&OrgDocument> = docs.iter().collect();
    assert!(would_create_blocker_cycle(&refs, (&docs[0], 1), (&docs[0], 0)).is_some());

    let seed = seed_for_entry(&docs[0], 0);
    let res = execute_trigger_cascade(&mut overlay, vec![seed], false);
    assert_eq!(res.entries_affected, 1);
    assert!(overlay
        .content(&f_path)
        .unwrap()
        .contains("* NEXT Trigger Tgt"));
}

/// Verifies `merge_blocker_id` preserves non-`done?` conditional clauses instead of corrupting them.
#[test]
fn merge_blocker_id_preserves_conditional_clauses() {
    let guarded = merge_blocker_id(Some("ids(\"guard\") !done?"), "dep-1");
    assert_eq!(guarded, "ids(\"guard\") !done? ids(\"dep-1\")");

    let guarded2 = merge_blocker_id(Some(&guarded), "dep-2");
    assert_eq!(guarded2, "ids(\"guard\") !done? ids(\"dep-1\" \"dep-2\")");

    let explicit_done = merge_blocker_id(Some("ids(\"d-1\") done?"), "d-2");
    assert_eq!(explicit_done, "ids(\"d-1\" \"d-2\") done?");
}

/// Verifies chained `:TRIGGER:` conditions stay suppressed when an earlier condition fails.
#[test]
fn chained_trigger_conditions_short_circuit_when_first_fails() {
    let dir = TempDir::new().unwrap();
    let f_path = dir.path().join("chained.org");
    std::fs::write(
        &f_path,
        concat!(
            "* DONE Src\n:PROPERTIES:\n:ID: c-src\n",
            ":TRIGGER: ids(\"c-tgt\") !done? has-property?(\"ROLE\" \"ops\") set-property!(\"FIRED\" \"yes\")\n:END:\n",
            "* DONE Tgt\n:PROPERTIES:\n:ID: c-tgt\n:ROLE: ops\n:END:\n",
        ),
    )
    .unwrap();

    let mut overlay = WorkspaceOverlay::load_workspace(dir.path());
    let docs = overlay.parse_documents();
    let seed = seed_for_entry(&docs[0], 0);
    let res = execute_trigger_cascade(&mut overlay, vec![seed], false);
    assert_eq!(res.entries_affected, 0);
    assert!(!overlay.content(&f_path).unwrap().contains(":FIRED:"));
}

/// Verifies unresolved and partially-unresolved `:BLOCKER:` IDs report blocked details.
#[test]
fn unresolved_and_partial_blocker_ids_are_blocked() {
    let mut doc = make_doc_with_entries(&[
        ("Dep Done", Some("DONE"), None),
        ("Blocked", Some("TODO"), None),
    ]);
    doc.entries[0]
        .properties
        .insert("ID".into(), "dep-done".into());
    doc.entries[1]
        .properties
        .insert("BLOCKER".into(), "ids(\"dep-done\" \"missing-id\")".into());
    let docs: Vec<&OrgDocument> = vec![&doc];
    let ctx = EdnaContext {
        all_docs: &docs,
        doc: &doc,
        entry_idx: 1,
    };
    assert!(is_blocked(&ctx));
    let details = blocking_details(&ctx);
    assert_eq!(details.len(), 1);
    assert_eq!(details[0].locator, "id:missing-id");
}

// endregion
