// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=680 reason="Cohesive query predicate evaluator, BM25 body-ranking bridge, and 23-case behavioural unit test suite"

//! Predicate evaluation and BM25 body-search helpers against [`org_tools_core::document::OrgEntry`] values.

use std::path::Path;

use org_tools_core::cache::{
    extract_entry_bodies, matches_body_phrase, rank_matched_indices_bm25, BodyLoadedDoc, FtsHit,
};
use org_tools_core::document::{OrgDocument, OrgEntry};
use org_tools_core::edna::{self, EdnaContext};
use org_tools_core::files::join_slash_rel;
use org_tools_core::rules::timestamp::OrgTimestamp;
use org_tools_core::source::SourceFile;

use super::parser::{CmpOp, Comparison, DateMatch, DateRef, DateUnit, Predicate, PriorityMatch};

// region Public API

/// Evaluates `pred` against `entry` in `doc` with optional cross-file `all_docs` context.
pub fn matches(
    pred: &Predicate,
    entry: &OrgEntry,
    doc: &OrgDocument,
    all_docs: &[&OrgDocument],
    today: (u16, u8, u8),
) -> bool {
    let entry_idx = doc
        .entries
        .iter()
        .position(|e| std::ptr::eq(e, entry))
        .unwrap_or(0);
    matches_with_body(pred, entry, entry_idx, None, doc, all_docs, today)
}

/// Evaluates `pred` against `entry` using pre-extracted `entry_body` when supplied.
pub fn matches_with_body(
    pred: &Predicate,
    entry: &OrgEntry,
    entry_idx: usize,
    entry_body: Option<&str>,
    doc: &OrgDocument,
    all_docs: &[&OrgDocument],
    today: (u16, u8, u8),
) -> bool {
    match pred {
        Predicate::True => true,
        Predicate::Body(needle) => eval_body(entry, entry_idx, entry_body, doc, needle),
        Predicate::Blocked => is_entry_blocked(doc, all_docs, entry_idx),
        Predicate::Actionable => is_entry_actionable(entry, doc, all_docs, entry_idx),
        Predicate::Waiting => is_entry_waiting(entry),
        Predicate::Meta { key, value } => {
            let meta_vals = entry
                .get_vulpea_meta(key)
                .or_else(|| doc.get_vulpea_meta(key));
            match (meta_vals, value) {
                (Some(vals), Some(v)) => vals.iter().any(|item| item.eq_ignore_ascii_case(v)),
                (Some(vals), None) => !vals.is_empty(),
                (None, _) => false,
            }
        }
        Predicate::And(preds) => preds
            .iter()
            .all(|p| matches_with_body(p, entry, entry_idx, entry_body, doc, all_docs, today)),
        Predicate::Or(preds) => preds
            .iter()
            .any(|p| matches_with_body(p, entry, entry_idx, entry_body, doc, all_docs, today)),
        Predicate::Not(inner) => {
            !matches_with_body(inner, entry, entry_idx, entry_body, doc, all_docs, today)
        }
        other => match_metadata_predicate(other, entry, entry_idx, doc, today),
    }
}

/// Filters BM25-ordered [`FtsHit`] candidates against full `pred` semantics, honoring `limit`.
pub fn filter_fts_hits(
    pred: &Predicate,
    scan_dir: &Path,
    hits: Vec<FtsHit>,
    loaded: &[BodyLoadedDoc],
    doc_refs: &[&OrgDocument],
    today: (u16, u8, u8),
    limit: Option<usize>,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for hit in hits {
        let target_file = join_slash_rel(scan_dir, &hit.rel_path);
        let Ok(di) = loaded.binary_search_by(|d| d.doc.file.cmp(&target_file)) else {
            continue;
        };
        let Some(entry) = loaded[di].doc.entries.get(hit.entry_idx) else {
            continue;
        };
        if matches_with_body(
            pred,
            entry,
            hit.entry_idx,
            Some(&loaded[di].bodies[hit.entry_idx]),
            &loaded[di].doc,
            doc_refs,
            today,
        ) {
            out.push((di, hit.entry_idx));
            if limit.is_some_and(|n| out.len() >= n) {
                break;
            }
        }
    }
    out
}

/// Evaluates `pred` across `loaded` documents and orders matches by Okapi BM25 relevance.
pub fn eval_and_rank_body_docs(
    pred: &Predicate,
    loaded: &[BodyLoadedDoc],
    doc_refs: &[&OrgDocument],
    today: (u16, u8, u8),
    limit: Option<usize>,
) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for (di, item) in loaded.iter().enumerate() {
        for (ei, entry) in item.doc.entries.iter().enumerate() {
            if matches_with_body(
                pred,
                entry,
                ei,
                Some(&item.bodies[ei]),
                &item.doc,
                doc_refs,
                today,
            ) {
                pairs.push((di, ei));
            }
        }
    }
    let mut ranked = rank_matched_indices_bm25(loaded, &pairs, &pred.positive_body_phrases());
    if let Some(n) = limit {
        ranked.truncate(n);
    }
    ranked
}

// endregion

// region Predicate Evaluation

/// Evaluates `Predicate::Body(needle)` against `entry.title` and `entry_body` (loading from disk when `None`).
fn eval_body(
    entry: &OrgEntry,
    entry_idx: usize,
    entry_body: Option<&str>,
    doc: &OrgDocument,
    needle: &str,
) -> bool {
    if let Some(body) = entry_body {
        return matches_body_phrase(&entry.title, body, needle);
    }
    let loaded = SourceFile::from_path(&doc.file)
        .ok()
        .and_then(|src| {
            extract_entry_bodies(&src.content, doc)
                .into_iter()
                .nth(entry_idx)
        })
        .unwrap_or_default();
    matches_body_phrase(&entry.title, &loaded, needle)
}

/// Returns `true` when `entry_idx` is blocked by an `org-edna` `:BLOCKER:` expression.
fn is_entry_blocked(doc: &OrgDocument, all_docs: &[&OrgDocument], entry_idx: usize) -> bool {
    edna::is_blocked(&EdnaContext {
        all_docs,
        doc,
        entry_idx,
    })
}

/// Returns `true` when `entry` is an active, non-waiting, unblocked TODO item.
fn is_entry_actionable(
    entry: &OrgEntry,
    doc: &OrgDocument,
    all_docs: &[&OrgDocument],
    entry_idx: usize,
) -> bool {
    let has_todo = entry.keyword.is_some()
        && !entry
            .keyword
            .as_deref()
            .is_some_and(|k| doc.todo_keywords.is_done(k));
    has_todo && !is_entry_waiting(entry) && !is_entry_blocked(doc, all_docs, entry_idx)
}

/// Returns `true` when `entry` carries a `WAIT` keyword or `:WAITING_FOR:` property.
fn is_entry_waiting(entry: &OrgEntry) -> bool {
    entry
        .keyword
        .as_deref()
        .is_some_and(|kw| kw.to_uppercase().contains("WAIT"))
        || entry.properties.contains_key("WAITING_FOR")
}

/// Evaluates non-recursive metadata and planning predicates on `entry`.
fn match_metadata_predicate(
    pred: &Predicate,
    entry: &OrgEntry,
    entry_idx: usize,
    doc: &OrgDocument,
    today: (u16, u8, u8),
) -> bool {
    match pred {
        Predicate::Todo(kw) => match kw {
            None => entry.keyword.is_some(),
            Some(k) => entry.keyword.as_deref() == Some(k.as_str()),
        },
        Predicate::Done => entry
            .keyword
            .as_deref()
            .is_some_and(|k| doc.todo_keywords.is_done(k)),
        Predicate::Tags(tags) => {
            let eff = doc.effective_tags(entry_idx);
            tags.iter()
                .all(|t| eff.iter().any(|it| it.eq_ignore_ascii_case(t)))
        }
        Predicate::Heading(substr) => entry.title.to_lowercase().contains(&substr.to_lowercase()),
        other => match_structural_predicate(other, entry, entry_idx, doc, today),
    }
}

/// Evaluates property, priority, level, clock, and planning predicates on `entry`.
fn match_structural_predicate(
    pred: &Predicate,
    entry: &OrgEntry,
    entry_idx: usize,
    doc: &OrgDocument,
    today: (u16, u8, u8),
) -> bool {
    match pred {
        Predicate::Property { key, value } => {
            let actual = doc.property(entry_idx, key).or_else(|| {
                entry
                    .properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v.as_str())
            });
            match actual {
                Some(act) => org_tools_core::property::matches_property_token(act, value),
                None => false,
            }
        }
        Predicate::Priority(pm) => match pm {
            PriorityMatch::Exact(ch) => entry.priority == Some(*ch),
            PriorityMatch::Cmp(op, ch) => entry.priority.is_some_and(|p| cmp_priority(*op, p, *ch)),
        },
        Predicate::Level(cmp) => match_comparison(cmp, entry.level),
        Predicate::Clocked => !entry.clocks.is_empty(),
        other => match_planning_predicate(other, entry, today),
    }
}

/// Evaluates `pred` against a level-0 file `:PROPERTIES:` node.
pub fn matches_file_node(
    pred: &Predicate,
    doc: &OrgDocument,
    _all_docs: &[&OrgDocument],
    _today: (u16, u8, u8),
) -> bool {
    match pred {
        Predicate::True => true,
        Predicate::Level(cmp) => match_comparison(cmp, 0),
        Predicate::Heading(substr) => doc
            .file_title_or_stem()
            .to_lowercase()
            .contains(&substr.to_lowercase()),
        Predicate::Tags(tags) => {
            let eff = doc.effective_tags(org_tools_core::locator::FILE_NODE_ENTRY_INDEX);
            tags.iter()
                .all(|t| eff.iter().any(|it| it.eq_ignore_ascii_case(t)))
        }
        Predicate::Property { key, value } => {
            let actual = doc
                .property(org_tools_core::locator::FILE_NODE_ENTRY_INDEX, key)
                .or_else(|| {
                    doc.file_properties
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(key))
                        .map(|(_, v)| v.as_str())
                });
            match actual {
                Some(act) => org_tools_core::property::matches_property_token(act, value),
                None => false,
            }
        }
        Predicate::Meta { key, value } => {
            let meta_vals = doc.get_vulpea_meta(key);
            match (meta_vals, value) {
                (Some(vals), Some(v)) => vals.iter().any(|item| item.eq_ignore_ascii_case(v)),
                (Some(vals), None) => !vals.is_empty(),
                (None, _) => false,
            }
        }
        Predicate::And(preds) => preds
            .iter()
            .all(|p| matches_file_node(p, doc, _all_docs, _today)),
        Predicate::Or(preds) => preds
            .iter()
            .any(|p| matches_file_node(p, doc, _all_docs, _today)),
        Predicate::Not(inner) => !matches_file_node(inner, doc, _all_docs, _today),
        _ => false,
    }
}

/// Evaluates `Scheduled`, `Deadline`, or `Closed` planning predicates on `entry`.
fn match_planning_predicate(pred: &Predicate, entry: &OrgEntry, today: (u16, u8, u8)) -> bool {
    match pred {
        Predicate::Scheduled(dm) => match_date_opt(&entry.planning.scheduled, dm, today),
        Predicate::Deadline(dm) => match_date_opt(&entry.planning.deadline, dm, today),
        Predicate::Closed(dm) => match_date_opt(&entry.planning.closed, dm, today),
        _ => false,
    }
}

// endregion

// region Comparison & Date Helpers

/// Evaluates a numeric [`Comparison`] against `value`.
fn match_comparison(cmp: &Comparison, value: usize) -> bool {
    match cmp {
        Comparison::Eq(n) => value == *n,
        Comparison::Lt(n) => value < *n,
        Comparison::Lte(n) => value <= *n,
        Comparison::Gt(n) => value > *n,
        Comparison::Gte(n) => value >= *n,
    }
}

/// Compares Org priorities where `A < B < C` (`A` is highest priority).
fn cmp_priority(op: CmpOp, entry_pri: char, target: char) -> bool {
    match op {
        CmpOp::Eq => entry_pri == target,
        CmpOp::Lt => entry_pri > target,
        CmpOp::Lte => entry_pri >= target,
        CmpOp::Gt => entry_pri < target,
        CmpOp::Gte => entry_pri <= target,
    }
}

/// Evaluates a [`DateMatch`] against an optional planning timestamp.
fn match_date_opt(ts: &Option<OrgTimestamp>, dm: &DateMatch, today: (u16, u8, u8)) -> bool {
    match dm {
        DateMatch::Any => ts.is_some(),
        _ => ts.as_ref().is_some_and(|t| match_date(t, dm, today)),
    }
}

/// Evaluates a [`DateMatch`] against a concrete [`OrgTimestamp`].
fn match_date(ts: &OrgTimestamp, dm: &DateMatch, today: (u16, u8, u8)) -> bool {
    let ts_days = date_to_days(ts.year, ts.month, ts.day);
    let today_days = date_to_days(today.0, today.1, today.2);
    match dm {
        DateMatch::Any => true,
        DateMatch::Today => ts_days == today_days,
        DateMatch::Past => ts_days < today_days,
        DateMatch::Future => ts_days > today_days,
        DateMatch::Cmp(op, date_ref) => {
            let ref_days = date_ref_to_days(date_ref, today_days);
            match op {
                CmpOp::Eq => ts_days == ref_days,
                CmpOp::Lt => ts_days < ref_days,
                CmpOp::Lte => ts_days <= ref_days,
                CmpOp::Gt => ts_days > ref_days,
                CmpOp::Gte => ts_days >= ref_days,
            }
        }
    }
}

/// Converts a civil `(year, month, day)` triple into a day count.
fn date_to_days(year: u16, month: u8, day: u8) -> i64 {
    super::agenda::date_to_days_pub(year, month, day)
}

/// Resolves a [`DateRef`] relative to `today_days`.
fn date_ref_to_days(date_ref: &DateRef, today_days: i64) -> i64 {
    match date_ref {
        DateRef::Today => today_days,
        DateRef::Relative(n, unit) => {
            let delta = match unit {
                DateUnit::Day => *n,
                DateUnit::Week => n * 7,
                DateUnit::Month => n * 30,
            };
            today_days + delta
        }
        DateRef::Absolute(y, m, d) => date_to_days(*y, *m, *d),
    }
}

// endregion

// region Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a fixed civil reference date `(2024, 6, 15)` for deterministic tests.
    fn today() -> (u16, u8, u8) {
        (2024, 6, 15)
    }

    /// Parses `org` text into an in-memory [`OrgDocument`] and default entry index `0`.
    fn make_doc_and_entry(org: &str) -> (OrgDocument, usize) {
        let source = SourceFile::new("test.org", org.to_string());
        (OrgDocument::from_source(&source), 0)
    }

    /// Evaluates `pred` against `doc.entries[idx]` in a single-document context.
    fn eval(pred: &Predicate, doc: &OrgDocument, idx: usize) -> bool {
        let all: Vec<&OrgDocument> = vec![doc];
        matches(pred, &doc.entries[idx], doc, &all, today())
    }

    /// Verifies `Predicate::Todo(None)` matches any TODO keyword.
    #[test]
    fn match_todo_any() {
        let (doc, idx) = make_doc_and_entry("* TODO Task\n");
        assert!(eval(&Predicate::Todo(None), &doc, idx));
    }

    /// Verifies `Predicate::Todo(Some(...))` matches only the specified TODO keyword.
    #[test]
    fn match_todo_specific() {
        let (doc, idx) = make_doc_and_entry("* TODO Task\n");
        assert!(eval(&Predicate::Todo(Some("TODO".to_string())), &doc, idx));
        assert!(!eval(&Predicate::Todo(Some("NEXT".to_string())), &doc, idx));
    }

    /// Verifies `Predicate::Done` matches DONE entries.
    #[test]
    fn match_done() {
        let (doc, idx) = make_doc_and_entry("* DONE Task\n");
        assert!(eval(&Predicate::Done, &doc, idx));
    }

    /// Verifies `Predicate::Tags` requires all listed tags case-insensitively.
    #[test]
    fn match_tags() {
        let (doc, idx) = make_doc_and_entry("* Task :work:urgent:\n");
        assert!(eval(&Predicate::Tags(vec!["work".to_string()]), &doc, idx));
        assert!(eval(
            &Predicate::Tags(vec!["work".into(), "urgent".into()]),
            &doc,
            idx
        ));
        assert!(!eval(&Predicate::Tags(vec!["home".to_string()]), &doc, idx));
    }

    /// Verifies `Predicate::Heading` performs case-insensitive substring matching.
    #[test]
    fn match_heading_substring() {
        let (doc, idx) = make_doc_and_entry("* Team Meeting Notes\n");
        assert!(eval(&Predicate::Heading("meeting".to_string()), &doc, idx));
        assert!(!eval(&Predicate::Heading("standup".to_string()), &doc, idx));
    }

    /// Verifies `Predicate::Body` matches `unicode61` phrases across title and body prose.
    #[test]
    fn match_body_phrase_in_title_and_body() {
        let raw = "* TODO Alpha Heading\n:PROPERTIES:\n:ID: x1\n:END:\nSearchEngine runs fast.\n";
        let src = SourceFile::new("test.org", raw.to_string());
        let doc = OrgDocument::from_source(&src);
        let bodies = extract_entry_bodies(&src.content, &doc);
        let all = vec![&doc];
        assert!(matches_with_body(
            &Predicate::Body("SearchEngine".into()),
            &doc.entries[0],
            0,
            Some(&bodies[0]),
            &doc,
            &all,
            today()
        ));
        assert!(!matches_with_body(
            &Predicate::Body("x1".into()),
            &doc.entries[0],
            0,
            Some(&bodies[0]),
            &doc,
            &all,
            today()
        ));
    }

    /// Verifies `Predicate::Property` checks exact values and bare key existence.
    #[test]
    fn match_property() {
        let (doc, idx) = make_doc_and_entry("* Task\n:PROPERTIES:\n:CATEGORY: project\n:END:\n");
        let exact = Predicate::Property {
            key: "CATEGORY".into(),
            value: "project".into(),
        };
        let any_val = Predicate::Property {
            key: "CATEGORY".into(),
            value: String::new(),
        };
        assert!(eval(&exact, &doc, idx));
        assert!(eval(&any_val, &doc, idx));
    }

    /// Verifies `Predicate::Priority` honors Org `A > B > C` ordering.
    #[test]
    fn match_priority() {
        let (doc, idx) = make_doc_and_entry("* TODO [#A] Urgent\n");
        assert!(eval(
            &Predicate::Priority(PriorityMatch::Exact('A')),
            &doc,
            idx
        ));
        assert!(eval(
            &Predicate::Priority(PriorityMatch::Cmp(CmpOp::Gte, 'B')),
            &doc,
            idx
        ));
        assert!(!eval(
            &Predicate::Priority(PriorityMatch::Exact('B')),
            &doc,
            idx
        ));
    }

    /// Verifies `Predicate::Level` matches exact and relational outline levels.
    #[test]
    fn match_level() {
        let (doc, _) = make_doc_and_entry("* A\n** B\n*** C\n");
        assert!(eval(&Predicate::Level(Comparison::Eq(2)), &doc, 1));
        assert!(eval(&Predicate::Level(Comparison::Lte(2)), &doc, 1));
        assert!(!eval(&Predicate::Level(Comparison::Eq(1)), &doc, 1));
    }

    /// Verifies `Predicate::Clocked` detects `:LOGBOOK:` clock lines.
    #[test]
    fn match_clocked() {
        let (doc, idx) = make_doc_and_entry(
            "* Task\n:LOGBOOK:\nCLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:00] =>  1:00\n:END:\n",
        );
        assert!(eval(&Predicate::Clocked, &doc, idx));
    }

    /// Verifies `Predicate::Scheduled(DateMatch::Today)` matches today's date.
    #[test]
    fn match_scheduled_today() {
        let (doc, idx) = make_doc_and_entry("* Task\nSCHEDULED: <2024-06-15 Sat>\n");
        assert!(eval(&Predicate::Scheduled(DateMatch::Today), &doc, idx));
    }

    /// Verifies `Predicate::Deadline(DateMatch::Past)` matches overdue deadlines.
    #[test]
    fn match_deadline_past() {
        let (doc, idx) = make_doc_and_entry("* Task\nDEADLINE: <2024-06-10 Mon>\n");
        assert!(eval(&Predicate::Deadline(DateMatch::Past), &doc, idx));
    }

    /// Verifies `Predicate::And` requires every child predicate to match.
    #[test]
    fn match_and() {
        let (doc, idx) = make_doc_and_entry("* TODO Task :work:\n");
        let pred = Predicate::And(vec![
            Predicate::Todo(Some("TODO".into())),
            Predicate::Tags(vec!["work".into()]),
        ]);
        assert!(eval(&pred, &doc, idx));
    }

    /// Verifies `Predicate::Or` succeeds when any child predicate matches.
    #[test]
    fn match_or() {
        let (doc, idx) = make_doc_and_entry("* NEXT Task\n");
        let pred = Predicate::Or(vec![
            Predicate::Todo(Some("TODO".into())),
            Predicate::Todo(Some("NEXT".into())),
        ]);
        assert!(eval(&pred, &doc, idx));
    }

    /// Verifies `Predicate::Not` inverts its inner predicate.
    #[test]
    fn match_not() {
        let (doc, idx) = make_doc_and_entry("* TODO Task\n");
        assert!(!eval(
            &Predicate::Not(Box::new(Predicate::Todo(None))),
            &doc,
            idx
        ));
        assert!(eval(&Predicate::Not(Box::new(Predicate::Done)), &doc, idx));
    }

    /// Verifies `Predicate::Tags` resolves inherited ancestor tags.
    #[test]
    fn match_inherited_tags() {
        let (doc, _) = make_doc_and_entry("* Parent :parent_tag:\n** Child :child_tag:\n");
        assert!(eval(&Predicate::Tags(vec!["parent_tag".into()]), &doc, 1));
    }

    /// Verifies `Predicate::Blocked` detects unresolved `ids(...)` blockers.
    #[test]
    fn match_blocked() {
        let org = "* TODO Dep\n:PROPERTIES:\n:ID: dep-1\n:END:\n\
                   * TODO Blocked\n:PROPERTIES:\n:BLOCKER: ids(\"dep-1\")\n:END:\n";
        let (doc, _) = make_doc_and_entry(org);
        assert!(eval(&Predicate::Blocked, &doc, 1));
        assert!(!eval(&Predicate::Blocked, &doc, 0));
    }

    /// Verifies `Predicate::Actionable` excludes blocked entries.
    #[test]
    fn match_actionable() {
        let org = "* TODO Dep\n:PROPERTIES:\n:ID: dep-1\n:END:\n\
                   * TODO Blocked\n:PROPERTIES:\n:BLOCKER: ids(\"dep-1\")\n:END:\n";
        let (doc, _) = make_doc_and_entry(org);
        assert!(eval(&Predicate::Actionable, &doc, 0));
        assert!(!eval(&Predicate::Actionable, &doc, 1));
    }

    /// Verifies `Predicate::Actionable` excludes DONE entries.
    #[test]
    fn done_entry_not_actionable() {
        let (doc, idx) = make_doc_and_entry("* DONE Task\n");
        assert!(!eval(&Predicate::Actionable, &doc, idx));
    }

    /// Verifies `Predicate::Waiting` matches `WAITING` keywords.
    #[test]
    fn match_waiting_keyword() {
        let (doc, idx) =
            make_doc_and_entry("#+TODO: TODO WAITING | DONE\n* WAITING Response from vendor\n");
        assert!(eval(&Predicate::Waiting, &doc, idx));
    }

    /// Verifies `Predicate::Waiting` matches `:WAITING_FOR:` properties.
    #[test]
    fn match_waiting_property() {
        let (doc, idx) =
            make_doc_and_entry("* TODO Follow up\n:PROPERTIES:\n:WAITING_FOR: John\n:END:\n");
        assert!(eval(&Predicate::Waiting, &doc, idx));
    }

    /// Verifies regular TODO entries do not match `Predicate::Waiting`.
    #[test]
    fn not_waiting_for_regular_todo() {
        let (doc, idx) = make_doc_and_entry("* TODO Regular task\n");
        assert!(!eval(&Predicate::Waiting, &doc, idx));
    }
}

// endregion
