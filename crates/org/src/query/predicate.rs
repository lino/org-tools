// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Predicate evaluation against [`OrgEntry`] values.

use org_tools_core::document::{OrgDocument, OrgEntry};
use org_tools_core::edna::{self, EdnaContext};
use org_tools_core::rules::timestamp::OrgTimestamp;

use super::parser::{CmpOp, Comparison, DateMatch, DateRef, DateUnit, Predicate, PriorityMatch};

/// Evaluate a predicate against an entry in a document.
///
/// The `doc` parameter is needed for tag inheritance and TODO keyword config.
/// The `all_docs` parameter enables cross-file resolution for edna blockers.
pub fn matches(
    pred: &Predicate,
    entry: &OrgEntry,
    doc: &OrgDocument,
    all_docs: &[&OrgDocument],
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
            let entry_idx = doc
                .entries
                .iter()
                .position(|e| std::ptr::eq(e, entry))
                .unwrap_or(0);
            let inherited = doc.inherited_tags(entry_idx);
            tags.iter()
                .all(|t| inherited.iter().any(|it| it.eq_ignore_ascii_case(t)))
        }
        Predicate::Heading(substr) => {
            let lower = substr.to_lowercase();
            entry.title.to_lowercase().contains(&lower)
        }
        Predicate::Property { key, value } => {
            if value.is_empty() {
                entry.properties.contains_key(key)
            } else {
                entry.properties.get(key).is_some_and(|v| v == value)
            }
        }
        Predicate::Priority(pm) => match pm {
            PriorityMatch::Exact(ch) => entry.priority == Some(*ch),
            PriorityMatch::Cmp(op, ch) => entry.priority.is_some_and(|p| cmp_priority(*op, p, *ch)),
        },
        Predicate::Level(cmp) => match_comparison(cmp, entry.level),
        Predicate::Scheduled(dm) => match_date_opt(&entry.planning.scheduled, dm, today),
        Predicate::Deadline(dm) => match_date_opt(&entry.planning.deadline, dm, today),
        Predicate::Closed(dm) => match_date_opt(&entry.planning.closed, dm, today),
        Predicate::Clocked => !entry.clocks.is_empty(),
        Predicate::Blocked => {
            let entry_idx = doc
                .entries
                .iter()
                .position(|e| std::ptr::eq(e, entry))
                .unwrap_or(0);
            let ctx = EdnaContext {
                all_docs,
                doc,
                entry_idx,
            };
            edna::is_blocked(&ctx)
        }
        Predicate::Actionable => {
            // Actionable = has a TODO keyword AND is not blocked.
            let has_todo = entry.keyword.is_some()
                && !entry
                    .keyword
                    .as_deref()
                    .is_some_and(|k| doc.todo_keywords.is_done(k));
            if !has_todo {
                return false;
            }
            let entry_idx = doc
                .entries
                .iter()
                .position(|e| std::ptr::eq(e, entry))
                .unwrap_or(0);
            let ctx = EdnaContext {
                all_docs,
                doc,
                entry_idx,
            };
            !edna::is_blocked(&ctx)
        }
        Predicate::And(preds) => preds
            .iter()
            .all(|p| matches(p, entry, doc, all_docs, today)),
        Predicate::Or(preds) => preds
            .iter()
            .any(|p| matches(p, entry, doc, all_docs, today)),
        Predicate::Not(inner) => !matches(inner, entry, doc, all_docs, today),
    }
}

fn match_comparison(cmp: &Comparison, value: usize) -> bool {
    match cmp {
        Comparison::Eq(n) => value == *n,
        Comparison::Lt(n) => value < *n,
        Comparison::Lte(n) => value <= *n,
        Comparison::Gt(n) => value > *n,
        Comparison::Gte(n) => value >= *n,
    }
}

/// Compare priorities. Note: A < B < C in org-mode (A is highest).
fn cmp_priority(op: CmpOp, entry_pri: char, target: char) -> bool {
    match op {
        CmpOp::Eq => entry_pri == target,
        CmpOp::Lt => entry_pri > target, // Higher letter = lower priority.
        CmpOp::Lte => entry_pri >= target,
        CmpOp::Gt => entry_pri < target,
        CmpOp::Gte => entry_pri <= target,
    }
}

fn match_date_opt(ts: &Option<OrgTimestamp>, dm: &DateMatch, today: (u16, u8, u8)) -> bool {
    match dm {
        DateMatch::Any => ts.is_some(),
        _ => ts.as_ref().is_some_and(|t| match_date(t, dm, today)),
    }
}

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

/// Convert a date to a day count for comparison.
/// Uses Howard Hinnant's days_from_civil algorithm.
fn date_to_days(year: u16, month: u8, day: u8) -> i64 {
    super::agenda::date_to_days_pub(year, month, day)
}

fn date_ref_to_days(date_ref: &DateRef, today_days: i64) -> i64 {
    match date_ref {
        DateRef::Today => today_days,
        DateRef::Relative(n, unit) => {
            let delta = match unit {
                DateUnit::Day => *n,
                DateUnit::Week => n * 7,
                DateUnit::Month => n * 30, // Approximate.
            };
            today_days + delta
        }
        DateRef::Absolute(y, m, d) => date_to_days(*y, *m, *d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use org_tools_core::source::SourceFile;

    fn today() -> (u16, u8, u8) {
        (2024, 6, 15)
    }

    fn make_doc_and_entry(org: &str) -> (OrgDocument, usize) {
        let source = SourceFile::new("test.org", org.to_string());
        let doc = OrgDocument::from_source(&source);
        (doc, 0)
    }

    /// Helper: evaluate predicate with a single-doc context.
    fn eval(pred: &Predicate, doc: &OrgDocument, idx: usize) -> bool {
        let all: Vec<&OrgDocument> = vec![doc];
        matches(pred, &doc.entries[idx], doc, &all, today())
    }

    #[test]
    fn match_todo_any() {
        let (doc, idx) = make_doc_and_entry("* TODO Task\n");
        assert!(eval(&Predicate::Todo(None), &doc, idx));
    }

    #[test]
    fn match_todo_specific() {
        let (doc, idx) = make_doc_and_entry("* TODO Task\n");
        assert!(eval(&Predicate::Todo(Some("TODO".to_string())), &doc, idx));
        assert!(!eval(&Predicate::Todo(Some("NEXT".to_string())), &doc, idx));
    }

    #[test]
    fn match_done() {
        let (doc, idx) = make_doc_and_entry("* DONE Task\n");
        assert!(eval(&Predicate::Done, &doc, idx));
    }

    #[test]
    fn match_tags() {
        let (doc, idx) = make_doc_and_entry("* Task :work:urgent:\n");
        assert!(eval(&Predicate::Tags(vec!["work".to_string()]), &doc, idx));
        assert!(eval(
            &Predicate::Tags(vec!["work".to_string(), "urgent".to_string()]),
            &doc,
            idx
        ));
        assert!(!eval(&Predicate::Tags(vec!["home".to_string()]), &doc, idx));
    }

    #[test]
    fn match_heading_substring() {
        let (doc, idx) = make_doc_and_entry("* Team Meeting Notes\n");
        assert!(eval(&Predicate::Heading("meeting".to_string()), &doc, idx));
        assert!(!eval(&Predicate::Heading("standup".to_string()), &doc, idx));
    }

    #[test]
    fn match_property() {
        let (doc, idx) = make_doc_and_entry("* Task\n:PROPERTIES:\n:CATEGORY: project\n:END:\n");
        assert!(eval(
            &Predicate::Property {
                key: "CATEGORY".to_string(),
                value: "project".to_string()
            },
            &doc,
            idx
        ));
        assert!(eval(
            &Predicate::Property {
                key: "CATEGORY".to_string(),
                value: String::new()
            },
            &doc,
            idx
        ));
    }

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

    #[test]
    fn match_level() {
        let (doc, _) = make_doc_and_entry("* A\n** B\n*** C\n");
        assert!(eval(&Predicate::Level(Comparison::Eq(2)), &doc, 1));
        assert!(eval(&Predicate::Level(Comparison::Lte(2)), &doc, 1));
        assert!(!eval(&Predicate::Level(Comparison::Eq(1)), &doc, 1));
    }

    #[test]
    fn match_clocked() {
        let (doc, idx) = make_doc_and_entry(
            "* Task\n:LOGBOOK:\nCLOCK: [2024-01-15 Mon 09:00]--[2024-01-15 Mon 10:00] =>  1:00\n:END:\n",
        );
        assert!(eval(&Predicate::Clocked, &doc, idx));
    }

    #[test]
    fn match_scheduled_today() {
        let (doc, idx) = make_doc_and_entry("* Task\nSCHEDULED: <2024-06-15 Sat>\n");
        assert!(eval(&Predicate::Scheduled(DateMatch::Today), &doc, idx));
    }

    #[test]
    fn match_deadline_past() {
        let (doc, idx) = make_doc_and_entry("* Task\nDEADLINE: <2024-06-10 Mon>\n");
        assert!(eval(&Predicate::Deadline(DateMatch::Past), &doc, idx));
    }

    #[test]
    fn match_and() {
        let (doc, idx) = make_doc_and_entry("* TODO Task :work:\n");
        let pred = Predicate::And(vec![
            Predicate::Todo(Some("TODO".to_string())),
            Predicate::Tags(vec!["work".to_string()]),
        ]);
        assert!(eval(&pred, &doc, idx));
    }

    #[test]
    fn match_or() {
        let (doc, idx) = make_doc_and_entry("* NEXT Task\n");
        let pred = Predicate::Or(vec![
            Predicate::Todo(Some("TODO".to_string())),
            Predicate::Todo(Some("NEXT".to_string())),
        ]);
        assert!(eval(&pred, &doc, idx));
    }

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

    #[test]
    fn match_inherited_tags() {
        let (doc, _) = make_doc_and_entry("* Parent :parent_tag:\n** Child :child_tag:\n");
        assert!(eval(
            &Predicate::Tags(vec!["parent_tag".to_string()]),
            &doc,
            1
        ));
    }

    #[test]
    fn match_blocked() {
        let org = "* TODO Dep\n:PROPERTIES:\n:ID: dep-1\n:END:\n\
                   * TODO Blocked\n:PROPERTIES:\n:BLOCKER: ids(\"dep-1\")\n:END:\n";
        let (doc, _) = make_doc_and_entry(org);
        // Entry 1 is blocked because dep-1 (entry 0) is TODO.
        assert!(eval(&Predicate::Blocked, &doc, 1));
        // Entry 0 has no blocker.
        assert!(!eval(&Predicate::Blocked, &doc, 0));
    }

    #[test]
    fn match_actionable() {
        let org = "* TODO Dep\n:PROPERTIES:\n:ID: dep-1\n:END:\n\
                   * TODO Blocked\n:PROPERTIES:\n:BLOCKER: ids(\"dep-1\")\n:END:\n";
        let (doc, _) = make_doc_and_entry(org);
        // Entry 0 is actionable (TODO + no blocker).
        assert!(eval(&Predicate::Actionable, &doc, 0));
        // Entry 1 is not actionable (blocked).
        assert!(!eval(&Predicate::Actionable, &doc, 1));
    }

    #[test]
    fn done_entry_not_actionable() {
        let (doc, idx) = make_doc_and_entry("* DONE Task\n");
        assert!(!eval(&Predicate::Actionable, &doc, idx));
    }
}
