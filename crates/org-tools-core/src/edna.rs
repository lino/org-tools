// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parser, multi-finder evaluator, deterministic `:TRIGGER:` cascade engine, and
//! `:BLOCKER:` dependency graph analyzer for the org-edna mini-language.
//!
//! org-edna uses `:BLOCKER:` and `:TRIGGER:` properties inside `:PROPERTIES:`
//! drawers to express task dependencies via **finders** (target selectors),
//! **actions** (mutations suffixed with `!`), and **conditions** (predicates
//! suffixed with `?`, optionally negated with `!`).
//!
//! Spec: <https://www.nongnu.org/org-edna-el/>

mod actions;
mod eval;
mod graph;
mod parser;
mod triggers;

#[cfg(test)]
mod tests;

pub use actions::TriggeredMutation;
pub use eval::{
    blocking_details, is_blocked, resolve_finder_targets, BlockerDetail, EdnaContext, EdnaTarget,
};
pub use graph::{
    collect_resolved_targets, find_entry_blocker_cycle, merge_blocker_id, merge_trigger_id,
    remove_blocker_id, remove_trigger_id, would_create_blocker_cycle, GraphNode,
};
pub use parser::{
    extract_dependency_ids, format_edna_exprs, parse_edna, Action, Condition, EdnaExpr,
    EdnaParseError, Finder,
};
pub use triggers::{
    discover_workspace_root, execute_trigger_cascade, run_file_trigger_cascade, seed_for_entry,
    TriggerCascadeResult, TriggerSeed, WorkspaceOverlay, MAX_TRIGGER_DEPTH,
};
