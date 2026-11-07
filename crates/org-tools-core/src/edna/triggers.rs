// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=700 reason="Cohesive WorkspaceOverlay, workspace root discovery, and recursive trigger cascade engine"

//! Workspace overlay, root discovery, and recursive `:TRIGGER:` cascade engine.
//!
//! Spec: <https://www.nongnu.org/org-edna-el/>

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::document::OrgDocument;
use crate::files::{canonicalize_clean, collect_org_files};
use crate::locator::{locator_for_entry, resolve_in_document, OrgLocator, FILE_NODE_ENTRY_INDEX};
use crate::source::SourceFile;

use super::actions::{apply_action_to_entry, TriggerSourceInfo, TriggeredMutation};
use super::eval::{
    evaluate_condition_on_target, resolve_finder_eval, DocHandle, EdnaContext, EvalTarget,
};
use super::parser::{parse_edna, Action, Condition, EdnaExpr};

// region Constants & Types

/// Maximum recursion depth allowed for cascading `:TRIGGER:` executions.
pub const MAX_TRIGGER_DEPTH: usize = 16;

/// Ancestor marker filenames identifying a project or workspace root.
const WORKSPACE_ROOT_MARKERS: &[&str] = &[".org-tools.toml", ".fritz.toml"];

/// Identifies an entry whose `:TRIGGER:` property should be evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerSeed {
    /// Canonical or relative path to the file containing the entry.
    pub file: PathBuf,
    /// Stable locator (`id:...`, `::#...`, or `::*/...`) for the entry.
    pub locator: OrgLocator,
}

/// Summary returned after evaluating a `:TRIGGER:` cascade across a [`WorkspaceOverlay`].
#[derive(Debug, Clone, Default)]
pub struct TriggerCascadeResult {
    /// Ordered list of individual triggered mutations performed in memory.
    pub mutations: Vec<TriggeredMutation>,
    /// Count of distinct target entries mutated by triggered actions.
    pub entries_affected: usize,
}

/// Single file tracked inside [`WorkspaceOverlay`].
#[derive(Debug, Clone)]
struct OverlayEntry {
    /// Initial file content before any mutations in the current command.
    initial_content: String,
    /// Latest in-memory file content after primary and triggered mutations.
    current_content: String,
    /// Whether the file existed on disk prior to the command.
    existed_on_disk: bool,
}

/// In-memory transactional workspace buffer supporting dry-run `:TRIGGER:` cascades.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceOverlay {
    /// Deterministically ordered map from normalized file path to [`OverlayEntry`].
    files: BTreeMap<PathBuf, OverlayEntry>,
}

// endregion

// region Workspace Discovery & Overlay Methods

/// Discovers the workspace root for cross-file org-edna resolution.
///
/// Prefers `explicit` when supplied; otherwise walks ancestor directories of
/// `target_file` looking for `.org-tools.toml` or `.fritz.toml` (stopping before
/// the OS temporary root) and falls back to `target_file.parent()`.
pub fn discover_workspace_root(explicit: Option<&Path>, target_file: &Path) -> PathBuf {
    if let Some(dir) = explicit {
        return dir.to_path_buf();
    }
    let temp_root = std::env::temp_dir();
    let temp_canon = canonicalize_clean(&temp_root).ok();
    let start_dir = target_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    for ancestor in start_dir.ancestors() {
        if ancestor.as_os_str().is_empty() || ancestor == temp_root {
            break;
        }
        if let (Some(ref tc), Ok(ac)) = (&temp_canon, canonicalize_clean(ancestor)) {
            if ac == *tc {
                break;
            }
        }
        if WORKSPACE_ROOT_MARKERS
            .iter()
            .any(|marker| ancestor.join(marker).is_file())
        {
            return ancestor.to_path_buf();
        }
    }
    start_dir.to_path_buf()
}

/// Builds a [`TriggerSeed`] for `entry_idx` in `doc`.
pub fn seed_for_entry(doc: &OrgDocument, entry_idx: usize) -> TriggerSeed {
    TriggerSeed {
        file: doc.file.clone(),
        locator: locator_for_entry(doc, entry_idx),
    }
}

/// Runs a file's post-transition `:TRIGGER:` cascade and optionally commits modified files.
pub fn run_file_trigger_cascade(
    file: &Path,
    before_doc: &OrgDocument,
    primary: &crate::state::SetStateResult,
    explicit_workspace: Option<&Path>,
    add_closed: bool,
    dry_run: bool,
) -> Result<(Vec<PathBuf>, TriggerCascadeResult), String> {
    let ws_root = discover_workspace_root(explicit_workspace, file);
    let mut overlay = WorkspaceOverlay::load_workspace(&ws_root);
    overlay.ensure_loaded(file);
    overlay.set_content(file, primary.content.clone());

    let post_doc = OrgDocument::from_source(&SourceFile::new(file, primary.content.clone()));
    let seeds = primary
        .newly_done_indices
        .iter()
        .copied()
        .filter(|&idx| before_doc.property(idx, "TRIGGER").is_some())
        .map(|idx| seed_for_entry(&post_doc, idx))
        .collect();
    let cascade = execute_trigger_cascade(&mut overlay, seeds, add_closed);
    if !dry_run {
        overlay.commit_to_disk()?;
    }
    let modified = overlay
        .modified_files()
        .into_iter()
        .map(|(p, _)| canonicalize_clean(p).unwrap_or_else(|_| p.to_path_buf()))
        .collect();
    Ok((modified, cascade))
}

impl WorkspaceOverlay {
    /// Scans all `.org` files under `workspace_root` into an in-memory overlay.
    pub fn load_workspace(workspace_root: &Path) -> Self {
        let mut overlay = Self::default();
        for path in collect_org_files(&[workspace_root.to_path_buf()]) {
            overlay.ensure_loaded(&path);
        }
        overlay
    }

    /// Ensures `path` is loaded from disk if not already present in the overlay.
    pub fn ensure_loaded(&mut self, path: &Path) {
        let key = self
            .matching_key(path)
            .unwrap_or_else(|| path.to_path_buf());
        if self.files.contains_key(&key) {
            return;
        }
        if let Ok(content) = std::fs::read_to_string(&key) {
            self.files.insert(
                key,
                OverlayEntry {
                    initial_content: content.clone(),
                    current_content: content,
                    existed_on_disk: true,
                },
            );
        }
    }

    /// Updates the in-memory content for `path`, preserving its `initial_content` baseline.
    pub fn set_content(&mut self, path: &Path, content: String) {
        let key = self
            .matching_key(path)
            .unwrap_or_else(|| path.to_path_buf());
        if let Some(entry) = self.files.get_mut(&key) {
            entry.current_content = content;
        } else {
            let initial = std::fs::read_to_string(&key).ok();
            let existed = initial.is_some();
            self.files.insert(
                key,
                OverlayEntry {
                    initial_content: initial.unwrap_or_default(),
                    current_content: content,
                    existed_on_disk: existed,
                },
            );
        }
    }

    /// Returns the current in-memory text for `path`, if loaded.
    pub fn content(&self, path: &Path) -> Option<&str> {
        let key = self.matching_key(path)?;
        self.files.get(&key).map(|e| e.current_content.as_str())
    }

    /// Parses all currently loaded overlay files into [`OrgDocument`]s.
    pub fn parse_documents(&self) -> Vec<OrgDocument> {
        self.files
            .iter()
            .map(|(path, entry)| {
                let src = SourceFile::new(path.clone(), entry.current_content.clone());
                OrgDocument::from_source(&src)
            })
            .collect()
    }

    /// Builds a map of current file contents for in-memory `re-search?` checks.
    fn source_map(&self) -> HashMap<PathBuf, String> {
        self.files
            .iter()
            .map(|(k, v)| (k.clone(), v.current_content.clone()))
            .collect()
    }

    /// Returns all `(path, content)` pairs whose content changed relative to baseline.
    pub fn modified_files(&self) -> Vec<(&Path, &str)> {
        self.files
            .iter()
            .filter(|(_, e)| !e.existed_on_disk || e.current_content != e.initial_content)
            .map(|(p, e)| (p.as_path(), e.current_content.as_str()))
            .collect()
    }

    /// Writes all modified overlay files atomically to disk, returning the number of files written.
    pub fn commit_to_disk(&self) -> Result<usize, String> {
        let modified = self.modified_files();
        for (path, content) in &modified {
            crate::files::write_file_atomic(path, content)
                .map_err(|e| format!("error writing {}: {e}", path.display()))?;
        }
        Ok(modified.len())
    }

    /// Finds an existing map key matching `path` directly or by canonical path.
    fn matching_key(&self, path: &Path) -> Option<PathBuf> {
        if self.files.contains_key(path) {
            return Some(path.to_path_buf());
        }
        let target_canon = canonicalize_clean(path).ok()?;
        self.files
            .keys()
            .find(|k| canonicalize_clean(k).ok().as_ref() == Some(&target_canon))
            .cloned()
    }
}

// endregion

// region Cascade Engine

/// Stable handle used to re-locate a target entry across sequential file edits.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetRef {
    /// Path to the Org file containing the target entry.
    file: PathBuf,
    /// Primary locator (`id:...`, `::#...`, or `::*/...`).
    primary: OrgLocator,
    /// Fallback outline path segments in case a property action alters `:ID:`.
    outline_fallback: Vec<String>,
}

/// Active finder scope while evaluating a `:TRIGGER:` expression list.
struct TriggerScope {
    /// Targets currently selected in the active scope.
    targets: Vec<TargetRef>,
    /// Whether at least one finder ran in the current scope group.
    has_finder: bool,
    /// Whether the next finder should clear `targets` and start a new scope.
    reset_on_next_finder: bool,
    /// Whether a top-level condition failed and gated subsequent actions in this scope.
    condition_failed: bool,
}

impl TriggerScope {
    /// Creates an empty trigger scope.
    fn new() -> Self {
        Self {
            targets: Vec::new(),
            has_finder: false,
            reset_on_next_finder: false,
            condition_failed: false,
        }
    }

    /// Merges resolved targets into `self.targets` without duplicates.
    fn merge(&mut self, resolved: &[EvalTarget<'_>]) {
        for item in resolved {
            if item.entry_idx == FILE_NODE_ENTRY_INDEX {
                continue;
            }
            let target_ref = TargetRef {
                file: item.doc.file.clone(),
                primary: locator_for_entry(&item.doc, item.entry_idx),
                outline_fallback: item
                    .doc
                    .outline_path(item.entry_idx)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            };
            if !self.targets.contains(&target_ref) {
                self.targets.push(target_ref);
            }
        }
    }

    /// Populates `self` as fallback target when an action/condition has no preceding finder.
    fn ensure_default_self(&mut self, src_doc: &OrgDocument, src_idx: usize) {
        if !self.has_finder && self.targets.is_empty() && src_idx < src_doc.entries.len() {
            self.targets.push(TargetRef {
                file: src_doc.file.clone(),
                primary: locator_for_entry(src_doc, src_idx),
                outline_fallback: src_doc
                    .outline_path(src_idx)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            });
        }
    }
}

/// Executes `:TRIGGER:` cascades starting from `initial_seeds` up to [`MAX_TRIGGER_DEPTH`].
pub fn execute_trigger_cascade(
    overlay: &mut WorkspaceOverlay,
    initial_seeds: Vec<TriggerSeed>,
    add_closed: bool,
) -> TriggerCascadeResult {
    let mut queue: VecDeque<(TriggerSeed, usize)> =
        initial_seeds.into_iter().map(|s| (s, 0)).collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut mutations = Vec::new();
    let mut affected_keys: HashSet<String> = HashSet::new();

    while let Some((seed, depth)) = queue.pop_front() {
        if depth >= MAX_TRIGGER_DEPTH {
            continue;
        }
        let docs = overlay.parse_documents();
        let Some((doc_pos, entry_idx)) = resolve_seed_in_docs(&seed, &docs) else {
            continue;
        };
        let visit_key = canonical_visit_key(&docs[doc_pos], entry_idx);
        if !visited.insert(visit_key) {
            continue;
        }
        let Some(trigger_raw) = docs[doc_pos]
            .property(entry_idx, "TRIGGER")
            .map(str::to_string)
        else {
            continue;
        };
        let (exprs, _) = parse_edna(&trigger_raw);
        if exprs.is_empty() {
            continue;
        }

        let mut step_ctx = CascadeStepContext {
            overlay,
            seed: &seed,
            add_closed,
            depth,
            visited: &visited,
            queue: &mut queue,
            mutations: &mut mutations,
            affected_keys: &mut affected_keys,
        };
        let mut scope = TriggerScope::new();
        run_trigger_exprs(&exprs, &mut scope, &mut step_ctx);
    }

    TriggerCascadeResult {
        mutations,
        entries_affected: affected_keys.len(),
    }
}

/// Mutable state passed through a single entry's `:TRIGGER:` AST execution.
struct CascadeStepContext<'a> {
    /// Workspace overlay receiving all file modifications.
    overlay: &'a mut WorkspaceOverlay,
    /// Seed of the source entry whose `:TRIGGER:` is firing.
    seed: &'a TriggerSeed,
    /// Whether `CLOSED:` timestamps are managed on `todo!` transitions.
    add_closed: bool,
    /// Current recursion depth (`0` for primary transition).
    depth: usize,
    /// Visited source entry keys for cycle prevention.
    visited: &'a HashSet<String>,
    /// Queue of downstream trigger seeds.
    queue: &'a mut VecDeque<(TriggerSeed, usize)>,
    /// Accumulated mutation log.
    mutations: &'a mut Vec<TriggeredMutation>,
    /// Distinct target keys modified across the cascade.
    affected_keys: &'a mut HashSet<String>,
}

/// Evaluates `exprs` in `:TRIGGER:` mode against `step.overlay`.
fn run_trigger_exprs(
    exprs: &[EdnaExpr],
    scope: &mut TriggerScope,
    step: &mut CascadeStepContext<'_>,
) {
    for expr in exprs {
        match expr {
            EdnaExpr::Finder(finder) => handle_trigger_finder(finder, scope, step),
            EdnaExpr::Condition(cond) => {
                if !scope.condition_failed {
                    scope.condition_failed = !evaluate_trigger_condition(cond, scope, step);
                }
                scope.reset_on_next_finder = true;
            }
            EdnaExpr::If { cond, then, else_ } => {
                let cond_ok = match cond.as_ref() {
                    EdnaExpr::Condition(c) => evaluate_trigger_condition(c, scope, step),
                    _ => true,
                };
                run_trigger_exprs(if cond_ok { then } else { else_ }, scope, step);
                scope.reset_on_next_finder = true;
            }
            EdnaExpr::Action(action) => {
                if !scope.condition_failed {
                    execute_action_on_scope(action, scope, step);
                }
                scope.reset_on_next_finder = true;
            }
        }
    }
}

/// Resolves `finder` and unions its matched targets into `scope`.
fn handle_trigger_finder(
    finder: &super::parser::Finder,
    scope: &mut TriggerScope,
    step: &mut CascadeStepContext<'_>,
) {
    if scope.reset_on_next_finder {
        *scope = TriggerScope::new();
    }
    scope.has_finder = true;
    let docs = step.overlay.parse_documents();
    let Some((doc_pos, entry_idx)) = resolve_seed_in_docs(step.seed, &docs) else {
        return;
    };
    let doc_refs: Vec<&OrgDocument> = docs.iter().collect();
    let edna_ctx = EdnaContext {
        all_docs: &doc_refs,
        doc: &docs[doc_pos],
        entry_idx,
    };
    let resolved = resolve_finder_eval(finder, &edna_ctx);
    for item in &resolved {
        step.overlay.ensure_loaded(&item.doc.file);
    }
    scope.merge(&resolved);
}

/// Evaluates `condition` against all targets currently in `scope`.
fn evaluate_trigger_condition(
    condition: &Condition,
    scope: &mut TriggerScope,
    step: &CascadeStepContext<'_>,
) -> bool {
    let docs = step.overlay.parse_documents();
    let Some((src_pos, src_idx)) = resolve_seed_in_docs(step.seed, &docs) else {
        return false;
    };
    scope.ensure_default_self(&docs[src_pos], src_idx);
    let sources = step.overlay.source_map();
    scope.targets.iter().all(|tref| {
        let Some((doc_pos, idx)) = resolve_target_ref(tref, &docs) else {
            return false;
        };
        let eval_target = EvalTarget {
            doc: DocHandle::Borrowed(&docs[doc_pos]),
            entry_idx: idx,
        };
        evaluate_condition_on_target(condition, &eval_target, Some(&sources))
    })
}

/// Applies `action` sequentially to every target in `scope`.
fn execute_action_on_scope(
    action: &Action,
    scope: &mut TriggerScope,
    step: &mut CascadeStepContext<'_>,
) {
    let init_docs = step.overlay.parse_documents();
    if let Some((src_pos, src_idx)) = resolve_seed_in_docs(step.seed, &init_docs) {
        scope.ensure_default_self(&init_docs[src_pos], src_idx);
    }
    let targets_snapshot = scope.targets.clone();

    for tref in &targets_snapshot {
        step.overlay.ensure_loaded(&tref.file);
        let docs = step.overlay.parse_documents();
        let Some((src_pos, src_idx)) = resolve_seed_in_docs(step.seed, &docs) else {
            continue;
        };
        let Some((tgt_pos, tgt_idx)) = resolve_target_ref(tref, &docs) else {
            continue;
        };
        let tgt_path = docs[tgt_pos].file.clone();
        let Some(raw_tgt) = step.overlay.content(&tgt_path).map(str::to_string) else {
            continue;
        };
        let tgt_source = SourceFile::new(tgt_path.clone(), raw_tgt);
        let archive_existing = resolve_existing_archive(step.overlay, &docs[tgt_pos], tgt_idx);
        let src_info = TriggerSourceInfo {
            doc: &docs[src_pos],
            entry_idx: src_idx,
            add_closed: step.add_closed,
        };

        let outcome = apply_action_to_entry(
            action,
            &tgt_source,
            &docs[tgt_pos],
            tgt_idx,
            &src_info,
            archive_existing.as_deref(),
        );
        record_action_outcome(outcome, tref, &tgt_path, step);
    }
}

/// Looks up existing archive file content in `overlay` when preparing for `archive!`.
fn resolve_existing_archive(
    overlay: &WorkspaceOverlay,
    doc: &OrgDocument,
    entry_idx: usize,
) -> Option<String> {
    let raw = doc
        .property(entry_idx, "ARCHIVE")
        .or_else(|| doc.file_keywords.get("ARCHIVE").map(String::as_str))
        .unwrap_or("");
    let target = crate::archive::parse_archive_target(raw, &doc.file);
    let archive_path = target.file.unwrap_or_else(|| doc.file.clone());
    overlay.content(&archive_path).map(str::to_string)
}

/// Commits an `ActionApplyOutcome` into `step.overlay` and enqueues downstream triggers.
fn record_action_outcome(
    outcome: super::actions::ActionApplyOutcome,
    tref: &TargetRef,
    tgt_path: &Path,
    step: &mut CascadeStepContext<'_>,
) {
    if let Some(updated) = outcome.updated_content {
        step.overlay.set_content(tgt_path, updated);
    }
    if let Some((arch_path, arch_content)) = outcome.archive_update {
        step.overlay.set_content(&arch_path, arch_content);
    }
    if let Some(m) = outcome.mutation {
        step.affected_keys
            .insert(format!("{}::{}", tgt_path.display(), tref.primary));
        step.mutations.push(m);
    }
    if outcome.transitioned_to_done && step.depth + 1 < MAX_TRIGGER_DEPTH {
        let next_docs = step.overlay.parse_documents();
        if let Some((doc_pos, idx)) = resolve_target_ref(tref, &next_docs) {
            let key = canonical_visit_key(&next_docs[doc_pos], idx);
            if !step.visited.contains(&key) {
                let next_seed = seed_for_entry(&next_docs[doc_pos], idx);
                step.queue.push_back((next_seed, step.depth + 1));
            }
        }
    }
}

// endregion

// region Target Resolution

/// Resolves a [`TriggerSeed`] to `(doc_index, entry_index)` across `docs`.
fn resolve_seed_in_docs(seed: &TriggerSeed, docs: &[OrgDocument]) -> Option<(usize, usize)> {
    if let OrgLocator::Id(ref id) = seed.locator {
        for (doc_pos, doc) in docs.iter().enumerate() {
            if let Some(idx) = doc.find_by_id(id) {
                return Some((doc_pos, idx));
            }
        }
    }
    let seed_canon = canonicalize_clean(&seed.file).ok();
    for (doc_pos, doc) in docs.iter().enumerate() {
        let same_file = doc.file == seed.file
            || (seed_canon.is_some() && canonicalize_clean(&doc.file).ok() == seed_canon);
        if same_file {
            if let Ok(res) = resolve_in_document(&seed.locator, doc) {
                if !res.is_file_node() {
                    return Some((doc_pos, res.entry_index));
                }
            }
        }
    }
    None
}

/// Resolves a [`TargetRef`] against `docs`, falling back to its outline path if needed.
fn resolve_target_ref(tref: &TargetRef, docs: &[OrgDocument]) -> Option<(usize, usize)> {
    if let OrgLocator::Id(ref id) = tref.primary {
        if let Some(found) = docs
            .iter()
            .enumerate()
            .find_map(|(pos, d)| d.find_by_id(id).map(|idx| (pos, idx)))
        {
            return Some(found);
        }
    }
    let file_canon = canonicalize_clean(&tref.file).ok();
    docs.iter().enumerate().find_map(|(pos, doc)| {
        resolve_target_ref_in_doc(tref, doc, file_canon.as_deref()).map(|idx| (pos, idx))
    })
}

/// Attempts to locate `tref` inside a single candidate `doc`.
fn resolve_target_ref_in_doc(
    tref: &TargetRef,
    doc: &OrgDocument,
    file_canon: Option<&Path>,
) -> Option<usize> {
    let same_file = doc.file == tref.file
        || (file_canon.is_some() && canonicalize_clean(&doc.file).ok().as_deref() == file_canon);
    if !same_file {
        return None;
    }
    let fallback_loc = OrgLocator::OutlinePath {
        file: doc.file.clone(),
        path: tref.outline_fallback.clone(),
    };
    for loc in [&tref.primary, &fallback_loc] {
        if let Ok(res) = resolve_in_document(loc, doc) {
            if !res.is_file_node() {
                return Some(res.entry_index);
            }
        }
    }
    None
}

/// Builds a canonical loop-guard key for `(doc, entry_idx)`.
fn canonical_visit_key(doc: &OrgDocument, entry_idx: usize) -> String {
    if let Some(id) = doc.property(entry_idx, "ID") {
        return format!("id:{id}");
    }
    let canon = canonicalize_clean(&doc.file).unwrap_or_else(|_| doc.file.clone());
    let olp = doc.outline_path(entry_idx).join("/");
    format!("{}::*/{olp}", canon.display())
}

// endregion
