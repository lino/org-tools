// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=710 reason="Cohesive multi-finder scope evaluator, structural/file/olp finders, and condition predicates"

//! Multi-finder scope evaluation, condition predicates, and `:BLOCKER:` analysis.
//!
//! Implements Emacs `org-edna` state-machine scoping: consecutive finders merge
//! their resolved entries into the active scope, while conditions and actions
//! mark the scope for reset on the next finder.
//!
//! Spec: <https://www.nongnu.org/org-edna-el/>

use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use regex::Regex;

use crate::document::{OrgDocument, OrgEntry};
use crate::files::canonicalize_clean;
use crate::locator::{locator_for_entry, FILE_NODE_ENTRY_INDEX};
use crate::source::SourceFile;

use super::parser::{parse_edna, Condition, EdnaExpr, Finder};

// region Public Types

/// Context for resolving org-edna finders and conditions across documents.
pub struct EdnaContext<'a> {
    /// All loaded workspace documents available for cross-file resolution.
    pub all_docs: &'a [&'a OrgDocument],
    /// The document containing the entry currently being evaluated.
    pub doc: &'a OrgDocument,
    /// Zero-based index of the entry in [`Self::doc`] (or [`FILE_NODE_ENTRY_INDEX`]).
    pub entry_idx: usize,
}

/// Structured diagnostic details describing an unsatisfied `:BLOCKER:` target.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockerDetail {
    /// Heading title (or file title/stem for level-0 file nodes) of the blocker.
    pub title: String,
    /// Current TODO keyword of the blocking entry, if any.
    pub keyword: Option<String>,
    /// Display path of the file containing the blocking entry.
    pub file: String,
    /// One-based heading or drawer line number of the blocking entry.
    pub line: usize,
    /// Canonical locator string for the blocking entry.
    pub locator: String,
    /// Human-readable summary of the unsatisfied condition.
    pub condition_desc: String,
}

/// Owned file and entry index pair identifying a resolved org-edna target.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EdnaTarget {
    /// Path to the Org file containing the target.
    pub file: PathBuf,
    /// Entry index in [`OrgDocument::entries`], or [`FILE_NODE_ENTRY_INDEX`] for a file node.
    pub entry_idx: usize,
}

// endregion

// region Internal Helpers

/// Borrowed workspace document or on-demand loaded relative document.
#[derive(Clone)]
pub(crate) enum DocHandle<'a> {
    /// Document borrowed from [`EdnaContext::all_docs`] or [`EdnaContext::doc`].
    Borrowed(&'a OrgDocument),
    /// Document loaded on demand from a relative `file()` or `olp()` path.
    Loaded(Rc<OrgDocument>),
}

impl Deref for DocHandle<'_> {
    type Target = OrgDocument;

    /// Dereferences the handle to its underlying [`OrgDocument`].
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(d) => d,
            Self::Loaded(d) => d.as_ref(),
        }
    }
}

/// Resolved entry or level-0 file node paired with its [`DocHandle`].
#[derive(Clone)]
pub(crate) struct EvalTarget<'a> {
    /// Document containing this target.
    pub(crate) doc: DocHandle<'a>,
    /// Index into `doc.entries`, or [`FILE_NODE_ENTRY_INDEX`] for a level-0 file node.
    pub(crate) entry_idx: usize,
}

impl<'a> EvalTarget<'a> {
    /// Returns the heading [`OrgEntry`] when this target is not a level-0 file node.
    pub(crate) fn entry(&self) -> Option<&OrgEntry> {
        self.doc.entries.get(self.entry_idx)
    }

    /// Converts this handle into an owned [`EdnaTarget`].
    pub(crate) fn to_owned_target(&self) -> EdnaTarget {
        EdnaTarget {
            file: self.doc.file.clone(),
            entry_idx: self.entry_idx,
        }
    }
}

/// Mutable scope state for org-edna finder merging and condition flushing.
struct BlockerScope<'a> {
    /// Targets accumulated in the current finder group.
    targets: Vec<EvalTarget<'a>>,
    /// Whether at least one finder ran in the current scope group.
    has_finder: bool,
    /// Whether an explicit condition evaluated against the current scope group.
    checked_explicitly: bool,
    /// Whether the next finder should start a fresh target scope.
    reset_on_next_finder: bool,
}

impl<'a> BlockerScope<'a> {
    /// Creates an empty initial scope.
    fn new() -> Self {
        Self {
            targets: Vec::new(),
            has_finder: false,
            checked_explicitly: false,
            reset_on_next_finder: false,
        }
    }

    /// Unions `incoming` targets into `self.targets`, preserving discovery order without duplicates.
    fn merge_targets(&mut self, incoming: Vec<EvalTarget<'a>>) {
        for item in incoming {
            let exists = self
                .targets
                .iter()
                .any(|t| t.doc.file == item.doc.file && t.entry_idx == item.entry_idx);
            if !exists {
                self.targets.push(item);
            }
        }
    }

    /// Ensures `self` is populated as the fallback target when no finder preceded a condition.
    fn ensure_default_self(&mut self, ctx: &EdnaContext<'a>) {
        if !self.has_finder && self.targets.is_empty() {
            self.targets.push(EvalTarget {
                doc: DocHandle::Borrowed(ctx.doc),
                entry_idx: ctx.entry_idx,
            });
        }
    }

    /// Flushes default `done?` checks if the active finder group had no explicit condition.
    fn flush_implicit_done(&mut self, details: &mut Vec<BlockerDetail>) {
        if self.has_finder && !self.checked_explicitly {
            for t in &self.targets {
                if !is_target_done(t) {
                    details.push(make_blocker_detail(t, "must be done"));
                }
            }
        }
    }
}

// endregion

// region Finder Resolution

/// Resolves a [`Finder`] within `ctx` into matching [`EdnaTarget`] values.
pub fn resolve_finder_targets(finder: &Finder, ctx: &EdnaContext<'_>) -> Vec<EdnaTarget> {
    resolve_finder_eval(finder, ctx)
        .into_iter()
        .map(|t| t.to_owned_target())
        .collect()
}

/// Resolves a [`Finder`] into internal [`EvalTarget`] handles.
pub(crate) fn resolve_finder_eval<'a>(
    finder: &Finder,
    ctx: &EdnaContext<'a>,
) -> Vec<EvalTarget<'a>> {
    match finder {
        Finder::Self_ => vec![EvalTarget {
            doc: DocHandle::Borrowed(ctx.doc),
            entry_idx: ctx.entry_idx,
        }],
        Finder::Ids(ids) => resolve_ids_finder(ids, ctx),
        Finder::Match(tag_spec) => resolve_match_finder(tag_spec, ctx),
        Finder::File(path) | Finder::OrgFile(path) => resolve_file_finder(path, ctx),
        Finder::Olp(args) => resolve_olp_finder(args, ctx),
        Finder::Relatives(_) => Vec::new(),
        _ => resolve_structural_finder(finder, ctx),
    }
}

/// Resolves `ids(...)` across heading `:ID:` properties and level-0 file `:ID:` drawers.
fn resolve_ids_finder<'a>(ids: &[String], ctx: &EdnaContext<'a>) -> Vec<EvalTarget<'a>> {
    resolve_ids_with_missing(ids, ctx).0
}

/// Resolves `ids(...)` into matched [`EvalTarget`]s and any unresolved clean IDs.
fn resolve_ids_with_missing<'a>(
    ids: &[String],
    ctx: &EdnaContext<'a>,
) -> (Vec<EvalTarget<'a>>, Vec<String>) {
    let mut results = Vec::new();
    let mut missing = Vec::new();
    for raw_id in ids {
        let clean_id = raw_id.strip_prefix("id:").unwrap_or(raw_id.as_str());
        let mut found = false;
        for &doc in ctx.all_docs {
            if let Some(idx) = doc.find_by_id(clean_id) {
                results.push(EvalTarget {
                    doc: DocHandle::Borrowed(doc),
                    entry_idx: idx,
                });
                found = true;
                break;
            }
            if doc.has_file_id(clean_id) {
                results.push(EvalTarget {
                    doc: DocHandle::Borrowed(doc),
                    entry_idx: FILE_NODE_ENTRY_INDEX,
                });
                found = true;
                break;
            }
        }
        if !found {
            missing.push(clean_id.to_string());
        }
    }
    (results, missing)
}

/// Resolves `match("tag")` across all workspace documents.
fn resolve_match_finder<'a>(tag_spec: &str, ctx: &EdnaContext<'a>) -> Vec<EvalTarget<'a>> {
    let mut results = Vec::new();
    for &doc in ctx.all_docs {
        for idx in 0..doc.entries.len() {
            let inherited = doc.inherited_tags(idx);
            if inherited.iter().any(|t| t.eq_ignore_ascii_case(tag_spec)) {
                results.push(EvalTarget {
                    doc: DocHandle::Borrowed(doc),
                    entry_idx: idx,
                });
            }
        }
    }
    results
}

/// Finds a document in `ctx.all_docs` or loads it relative to `ctx.doc.file`.
fn find_or_load_doc<'a>(raw_path: &str, ctx: &EdnaContext<'a>) -> Option<DocHandle<'a>> {
    let req = Path::new(raw_path);
    let base_dir = ctx.doc.file.parent().unwrap_or_else(|| Path::new("."));
    let rel_candidate = base_dir.join(req);
    let rel_canon = canonicalize_clean(&rel_candidate).ok();

    for &doc in ctx.all_docs {
        if doc.file == req || doc.file == rel_candidate || doc.file.ends_with(req) {
            return Some(DocHandle::Borrowed(doc));
        }
        if let (Some(ref target_c), Ok(doc_c)) = (&rel_canon, canonicalize_clean(&doc.file)) {
            if target_c == &doc_c {
                return Some(DocHandle::Borrowed(doc));
            }
        }
    }

    let load_path = if rel_candidate.is_file() {
        rel_candidate
    } else if req.is_file() {
        req.to_path_buf()
    } else {
        return None;
    };
    let source = SourceFile::from_path(&load_path).ok()?;
    Some(DocHandle::Loaded(Rc::new(OrgDocument::from_source(
        &source,
    ))))
}

/// Resolves `file("...")` and `org-file("...")` to task headings (or all headings / file node).
fn resolve_file_finder<'a>(path: &str, ctx: &EdnaContext<'a>) -> Vec<EvalTarget<'a>> {
    let Some(doc) = find_or_load_doc(path, ctx) else {
        return Vec::new();
    };
    let task_indices: Vec<usize> = doc
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.keyword.is_some())
        .map(|(i, _)| i)
        .collect();

    if !task_indices.is_empty() {
        return task_indices
            .into_iter()
            .map(|entry_idx| EvalTarget {
                doc: doc.clone(),
                entry_idx,
            })
            .collect();
    }
    if !doc.entries.is_empty() {
        return (0..doc.entries.len())
            .map(|entry_idx| EvalTarget {
                doc: doc.clone(),
                entry_idx,
            })
            .collect();
    }
    if doc.file_id().is_some() {
        return vec![EvalTarget {
            doc,
            entry_idx: FILE_NODE_ENTRY_INDEX,
        }];
    }
    Vec::new()
}

/// Matches an outline path slice against headings in `doc`.
fn match_olp_in_doc<'a>(doc: DocHandle<'a>, segments: &[String]) -> Vec<EvalTarget<'a>> {
    if segments.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for idx in 0..doc.entries.len() {
        let path = doc.outline_path(idx);
        let matches = path.len() == segments.len()
            && path
                .iter()
                .zip(segments.iter())
                .all(|(actual, expected)| actual.trim() == expected.trim());
        if matches {
            out.push(EvalTarget {
                doc: doc.clone(),
                entry_idx: idx,
            });
        }
    }
    out
}

/// Resolves both same-file `olp("H1" "H2")` and cross-file `olp("file.org" "H1" "H2")`.
fn resolve_olp_finder<'a>(args: &[String], ctx: &EdnaContext<'a>) -> Vec<EvalTarget<'a>> {
    let local = match_olp_in_doc(DocHandle::Borrowed(ctx.doc), args);
    if !local.is_empty() {
        return local;
    }
    if args.len() < 2 {
        return Vec::new();
    }
    if args[0].is_empty() {
        return match_olp_in_doc(DocHandle::Borrowed(ctx.doc), &args[1..]);
    }
    let Some(target_doc) = find_or_load_doc(&args[0], ctx) else {
        return Vec::new();
    };
    match_olp_in_doc(target_doc, &args[1..])
}

/// Resolves sibling-oriented finders (`next-sibling`, `previous-sibling`, `siblings`, etc.).
fn resolve_sibling_indices(finder: &Finder, ctx: &EdnaContext<'_>) -> Option<Vec<usize>> {
    match finder {
        Finder::NextSibling => Some(sibling_offset(ctx, 1).into_iter().collect()),
        Finder::PreviousSibling => Some(sibling_offset(ctx, -1).into_iter().collect()),
        Finder::RestOfSiblings => Some(rest_of_siblings(ctx)),
        Finder::Siblings => Some(
            get_siblings(ctx.doc, ctx.entry_idx)
                .into_iter()
                .filter(|&i| i != ctx.entry_idx)
                .collect(),
        ),
        _ => None,
    }
}

/// Resolves hierarchy-oriented finders (`parent`, `ancestors`, `children`, `descendants`, etc.).
fn resolve_hierarchy_indices(
    finder: &Finder,
    ctx: &EdnaContext<'_>,
    entry: &OrgEntry,
) -> Vec<usize> {
    match finder {
        Finder::Parent => entry.parent.into_iter().collect(),
        Finder::Ancestors => collect_ancestors(ctx.doc, ctx.entry_idx),
        Finder::Children => entry.children.clone(),
        Finder::FirstChild => entry.children.first().copied().into_iter().collect(),
        Finder::Descendants => {
            let mut desc = Vec::new();
            collect_descendant_indices(ctx.doc, ctx.entry_idx, &mut desc);
            desc
        }
        _ => Vec::new(),
    }
}

/// Resolves relative tree finders (`next-sibling`, `parent`, `children`, etc.) safely.
fn resolve_structural_finder<'a>(finder: &Finder, ctx: &EdnaContext<'a>) -> Vec<EvalTarget<'a>> {
    let Some(entry) = ctx.doc.entries.get(ctx.entry_idx) else {
        return Vec::new();
    };
    let indices = resolve_sibling_indices(finder, ctx)
        .unwrap_or_else(|| resolve_hierarchy_indices(finder, ctx, entry));
    indices
        .into_iter()
        .map(|entry_idx| EvalTarget {
            doc: DocHandle::Borrowed(ctx.doc),
            entry_idx,
        })
        .collect()
}

/// Returns sibling entry indices sharing the same parent (or top-level root) as `entry_idx`.
fn get_siblings(doc: &OrgDocument, entry_idx: usize) -> Vec<usize> {
    let Some(entry) = doc.entries.get(entry_idx) else {
        return Vec::new();
    };
    if let Some(parent_idx) = entry.parent {
        doc.entries[parent_idx].children.clone()
    } else {
        doc.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.parent.is_none())
            .map(|(i, _)| i)
            .collect()
    }
}

/// Finds the adjacent sibling at `delta` (`-1` for previous, `+1` for next).
fn sibling_offset(ctx: &EdnaContext<'_>, delta: isize) -> Option<usize> {
    let siblings = get_siblings(ctx.doc, ctx.entry_idx);
    let pos = siblings.iter().position(|&i| i == ctx.entry_idx)? as isize;
    let target_pos = pos + delta;
    if target_pos >= 0 && (target_pos as usize) < siblings.len() {
        Some(siblings[target_pos as usize])
    } else {
        None
    }
}

/// Returns all subsequent sibling indices after `ctx.entry_idx`.
fn rest_of_siblings(ctx: &EdnaContext<'_>) -> Vec<usize> {
    let siblings = get_siblings(ctx.doc, ctx.entry_idx);
    match siblings.iter().position(|&i| i == ctx.entry_idx) {
        Some(pos) => siblings[pos + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// Collects ancestor entry indices from immediate parent up to root.
fn collect_ancestors(doc: &OrgDocument, mut current: usize) -> Vec<usize> {
    let mut out = Vec::new();
    while let Some(parent_idx) = doc.entries.get(current).and_then(|e| e.parent) {
        out.push(parent_idx);
        current = parent_idx;
    }
    out
}

/// Recursively collects all descendant indices under `entry_idx`.
fn collect_descendant_indices(doc: &OrgDocument, entry_idx: usize, out: &mut Vec<usize>) {
    if let Some(entry) = doc.entries.get(entry_idx) {
        for &child_idx in &entry.children {
            out.push(child_idx);
            collect_descendant_indices(doc, child_idx, out);
        }
    }
}

// endregion

// region Condition & Blocker Evaluation

/// Returns `true` when `target` is a heading entry in a configured done TODO state.
pub(crate) fn is_target_done(target: &EvalTarget<'_>) -> bool {
    target
        .entry()
        .and_then(|e| e.keyword.as_deref())
        .is_some_and(|kw| target.doc.todo_keywords.is_done(kw))
}

/// Evaluates `condition` against a single `target`, optionally reading in-memory `sources`.
pub(crate) fn evaluate_condition_on_target(
    condition: &Condition,
    target: &EvalTarget<'_>,
    sources: Option<&HashMap<PathBuf, String>>,
) -> bool {
    match condition {
        Condition::Done => is_target_done(target),
        Condition::Negated(inner) => !evaluate_condition_on_target(inner, target, sources),
        Condition::TodoState(kw) => target
            .entry()
            .and_then(|e| e.keyword.as_deref())
            .is_some_and(|actual| actual.eq_ignore_ascii_case(kw)),
        Condition::HasProperty(key, val) => target
            .doc
            .property(target.entry_idx, key)
            .is_some_and(|actual| actual == val),
        Condition::HasTags(required) => {
            let tags: Vec<&str> = if target.entry_idx == FILE_NODE_ENTRY_INDEX {
                target.doc.filetags.iter().map(String::as_str).collect()
            } else {
                target.doc.inherited_tags(target.entry_idx)
            };
            required
                .iter()
                .all(|req| tags.iter().any(|t| t.eq_ignore_ascii_case(req)))
        }
        Condition::ReSearch(pattern) => evaluate_re_search(pattern, target, sources),
        Condition::VariableSet(_, _) => true,
    }
}

/// Evaluates `re-search?("pattern")` on the target subtree text, returning `false` on bad regex.
fn evaluate_re_search(
    pattern: &str,
    target: &EvalTarget<'_>,
    sources: Option<&HashMap<PathBuf, String>>,
) -> bool {
    let Ok(regex) = Regex::new(pattern) else {
        return false;
    };
    let raw_file = sources
        .and_then(|m| m.get(&target.doc.file).cloned())
        .or_else(|| std::fs::read_to_string(&target.doc.file).ok());

    if let Some(content) = raw_file {
        if target.entry_idx == FILE_NODE_ENTRY_INDEX {
            return regex.is_match(&content);
        }
        if let Some(entry) = target.entry() {
            let lines: Vec<&str> = content.split('\n').collect();
            let start = (entry.heading_line - 1).min(lines.len());
            let end = (entry.subtree_end_line - 1).min(lines.len());
            let subtree = lines[start..end].join("\n");
            return regex.is_match(&subtree);
        }
    }

    if let Some(entry) = target.entry() {
        return regex.is_match(&entry.raw_heading);
    }
    false
}

/// Formats a human-readable requirement description for [`Condition`].
fn condition_description(condition: &Condition) -> String {
    match condition {
        Condition::Done => "must be done".to_string(),
        Condition::Negated(inner) => negated_condition_description(inner),
        Condition::TodoState(kw) => format!("must have state {kw}"),
        Condition::HasProperty(key, value) => format!("must have property {key}={value}"),
        Condition::ReSearch(re) => format!("must match regex {re}"),
        Condition::VariableSet(var, val) => format!("variable {var} must be {val}"),
        Condition::HasTags(tags) => format!("must have tags {}", tags.join(", ")),
    }
}

/// Formats a negated human-readable requirement description (`!cond?`).
fn negated_condition_description(inner: &Condition) -> String {
    match inner {
        Condition::Done => "must not be done".to_string(),
        Condition::TodoState(kw) => format!("must not have state {kw}"),
        Condition::HasProperty(k, v) => format!("must not have property {k}={v}"),
        Condition::ReSearch(re) => format!("must not match regex {re}"),
        Condition::VariableSet(k, v) => format!("variable {k} must not be {v}"),
        Condition::HasTags(tags) => format!("must not have tags {}", tags.join(", ")),
        Condition::Negated(nested) => condition_description(nested),
    }
}

/// Builds a [`BlockerDetail`] from an [`EvalTarget`] and unsatisfied condition description.
fn make_blocker_detail(target: &EvalTarget<'_>, condition_desc: &str) -> BlockerDetail {
    let locator = locator_for_entry(&target.doc, target.entry_idx).to_string();
    if let Some(entry) = target.entry() {
        BlockerDetail {
            title: entry.title.clone(),
            keyword: entry.keyword.clone(),
            file: target.doc.file.display().to_string(),
            line: entry.heading_line,
            locator,
            condition_desc: condition_desc.to_string(),
        }
    } else {
        BlockerDetail {
            title: target.doc.file_title_or_stem(),
            keyword: None,
            file: target.doc.file.display().to_string(),
            line: target.doc.file_properties_line.unwrap_or(1),
            locator,
            condition_desc: condition_desc.to_string(),
        }
    }
}

/// Evaluates a `:BLOCKER:` condition across `scope` and appends unsatisfied `details`.
fn evaluate_blocker_condition<'a>(
    condition: &Condition,
    ctx: &EdnaContext<'a>,
    scope: &mut BlockerScope<'a>,
    details: &mut Vec<BlockerDetail>,
) {
    scope.ensure_default_self(ctx);
    scope.checked_explicitly = true;
    scope.reset_on_next_finder = true;
    let desc = condition_description(condition);
    for target in &scope.targets {
        if !evaluate_condition_on_target(condition, target, None) {
            details.push(make_blocker_detail(target, &desc));
        }
    }
}

/// Evaluates `exprs` in `:BLOCKER:` mode, updating `scope` and appending unsatisfied `details`.
fn evaluate_blocker_exprs<'a>(
    exprs: &[EdnaExpr],
    ctx: &EdnaContext<'a>,
    scope: &mut BlockerScope<'a>,
    details: &mut Vec<BlockerDetail>,
) {
    for expr in exprs {
        match expr {
            EdnaExpr::Finder(finder) => {
                if scope.reset_on_next_finder {
                    scope.flush_implicit_done(details);
                    *scope = BlockerScope::new();
                }
                scope.has_finder = true;
                if let Finder::Ids(ids) = finder {
                    let (targets, missing) = resolve_ids_with_missing(ids, ctx);
                    for missing_id in missing {
                        details.push(BlockerDetail {
                            title: format!("Unresolved blocker target ID: {missing_id}"),
                            keyword: None,
                            file: String::new(),
                            line: 0,
                            locator: format!("id:{missing_id}"),
                            condition_desc: "referenced blocker ID not found in documents"
                                .to_string(),
                        });
                    }
                    scope.merge_targets(targets);
                } else {
                    scope.merge_targets(resolve_finder_eval(finder, ctx));
                }
            }
            EdnaExpr::Condition(condition) => {
                evaluate_blocker_condition(condition, ctx, scope, details);
            }
            EdnaExpr::If { cond, then, else_ } => {
                scope.ensure_default_self(ctx);
                let cond_ok = match cond.as_ref() {
                    EdnaExpr::Condition(c) => scope
                        .targets
                        .iter()
                        .all(|t| evaluate_condition_on_target(c, t, None)),
                    _ => true,
                };
                evaluate_blocker_exprs(if cond_ok { then } else { else_ }, ctx, scope, details);
                scope.reset_on_next_finder = true;
            }
            EdnaExpr::Action(_) => {
                scope.reset_on_next_finder = true;
            }
        }
    }
}

/// Returns structured [`BlockerDetail`] items for every unsatisfied `:BLOCKER:` dependency.
///
/// Returns an empty vector when the entry has no `:BLOCKER:` property or all
/// dependencies are satisfied.
pub fn blocking_details(ctx: &EdnaContext<'_>) -> Vec<BlockerDetail> {
    let Some(blocker_value) = ctx.doc.property(ctx.entry_idx, "BLOCKER") else {
        return Vec::new();
    };
    let (exprs, _errors) = parse_edna(blocker_value);
    if exprs.is_empty() {
        return Vec::new();
    }

    let mut scope = BlockerScope::new();
    let mut details = Vec::new();
    evaluate_blocker_exprs(&exprs, ctx, &mut scope, &mut details);
    scope.flush_implicit_done(&mut details);
    details
}

/// Returns `true` when the entry in `ctx` is blocked by at least one `:BLOCKER:` dependency.
pub fn is_blocked(ctx: &EdnaContext<'_>) -> bool {
    !blocking_details(ctx).is_empty()
}

// endregion
