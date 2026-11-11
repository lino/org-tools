// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! SQLite-backed and static domain candidate generators for `org complete`.

use std::collections::HashSet;
use std::path::Path;

use org_tools_core::cache::{CachedIdCandidate, WorkspaceCache};
use org_tools_core::config::Config;

use super::{complete_date_spec_domain, complete_rule_domain, CompletionItem};

/// Canonical Org, `org-roam`, and `org-edna` property names and descriptions.
const CANONICAL_PROPERTIES: &[(&str, &str)] = &[
    ("EFFORT", "Estimated task effort duration (H:MM)"),
    ("STYLE", "Entry style (e.g. habit)"),
    ("LOGGING", "Per-entry state logging configuration"),
    ("CATEGORY", "Agenda category name"),
    ("BLOCKER", "org-edna blocking dependency expression"),
    ("TRIGGER", "org-edna state trigger expression"),
    ("ROAM_ALIASES", "org-roam node title aliases"),
    ("ROAM_REFS", "org-roam external URI or citation references"),
    ("ROAM_EXCLUDE", "Exclude node from org-roam indexing"),
    ("ID_SEED", "Deterministic RFC 9562 UUIDv5 seed URN"),
    ("CUSTOM_ID", "Human-readable custom heading identifier"),
    ("COLUMNS", "Column view format specification"),
    ("ARCHIVE", "Archive destination target"),
];

/// Supported `org query search` predicate prefixes and their descriptions.
const QUERY_PREDICATE_PREFIXES: &[(&str, &str)] = &[
    ("todo:", "Filter by active TODO keyword"),
    ("done:", "Filter by completion status (true|false)"),
    ("tags:", "Filter by inherited or heading tag"),
    ("heading:", "Filter by heading title substring"),
    ("body:", "Full-text BM25 phrase search in entry bodies"),
    ("property:", "Filter by property drawer key or KEY=VALUE"),
    ("priority:", "Filter by priority cookie (A|B|C)"),
    ("level:", "Filter by heading star level"),
    (
        "scheduled:",
        "Filter by SCHEDULED date (today|past|future|none)",
    ),
    (
        "deadline:",
        "Filter by DEADLINE date (today|past|future|none)",
    ),
    ("closed:", "Filter by CLOSED timestamp (today|past|none)"),
    ("clocked:", "Filter by clocked time presence (true|false)"),
    ("blocked:", "Filter by org-edna blocker status (true|false)"),
    ("actionable:", "Filter by unblocked actionable TODO status"),
    ("waiting:", "Filter by WAITING state or tag"),
];

/// Dispatches `kind` to its domain candidate generator, returning `None` on unknown `kind`.
pub(crate) fn collect_domain_candidates(
    kind: &str,
    prefix: &str,
    ws_root: &Path,
) -> Option<Vec<CompletionItem>> {
    match kind {
        "id" => Some(complete_id_domain(prefix, ws_root)),
        "locator" => Some(complete_locator_domain(prefix, ws_root)),
        "roam-node" => Some(complete_roam_node_domain(prefix, ws_root)),
        "todo-state" => Some(complete_todo_state_domain(prefix, ws_root)),
        "tag" => Some(complete_tag_domain(prefix, ws_root, false)),
        "context" => Some(complete_tag_domain(prefix, ws_root, true)),
        "property" => Some(complete_property_domain(prefix, ws_root)),
        "date-spec" => Some(complete_date_spec_domain(prefix)),
        "query" => Some(complete_query_domain(prefix, ws_root)),
        "rule" => Some(complete_rule_domain(prefix)),
        _ => None,
    }
}

/// Formats `<TODO> <Title> (<rel_path>:<line>)` for a [`CachedIdCandidate`].
fn format_id_desc(cand: &CachedIdCandidate) -> String {
    match cand.todo_keyword.as_deref() {
        Some(kw) => format!("{kw} {} ({}:{})", cand.title, cand.rel_path, cand.line),
        None => format!("{} ({}:{})", cand.title, cand.rel_path, cand.line),
    }
}

/// Completes `id:<uuid>` and bare `<uuid>` candidates from `.cache/org-cache.db`.
fn complete_id_domain(prefix: &str, ws_root: &Path) -> Vec<CompletionItem> {
    let Ok((cache, _)) = WorkspaceCache::open_and_sync(ws_root) else {
        return Vec::new();
    };
    let Ok(rows) = cache.complete_ids(prefix) else {
        return Vec::new();
    };
    let only_prefixed = prefix.trim().starts_with("id:");
    let mut out = Vec::with_capacity(rows.len() * 2);
    for row in rows {
        let desc = format_id_desc(&row);
        out.push(CompletionItem {
            candidate: format!("id:{}", row.id),
            description: desc.clone(),
        });
        if !only_prefixed {
            out.push(CompletionItem {
                candidate: row.id,
                description: desc,
            });
        }
    }
    out
}

/// Completes `id:<uuid>`, `#<custom-id>`, `<file>::#<custom-id>`, and `<file>::*<Heading>` locators.
fn complete_locator_domain(prefix: &str, ws_root: &Path) -> Vec<CompletionItem> {
    let Ok((cache, _)) = WorkspaceCache::open_and_sync(ws_root) else {
        return Vec::new();
    };
    let needle = prefix.trim().to_ascii_lowercase();
    let mut out = Vec::new();
    if let Ok(ids) = cache.complete_ids(prefix) {
        for row in ids {
            out.push(CompletionItem {
                candidate: format!("id:{}", row.id),
                description: format_id_desc(&row),
            });
        }
    }
    if let Ok(headings) = cache.complete_headings(prefix) {
        for h in headings {
            let desc = format!("{} ({}:{})", h.title, h.rel_path, h.heading_line);
            if let Some(ref cid) = h.custom_id {
                out.push(CompletionItem {
                    candidate: format!("#{cid}"),
                    description: desc.clone(),
                });
                out.push(CompletionItem {
                    candidate: format!("{}::#{cid}", h.rel_path),
                    description: desc.clone(),
                });
            }
            out.push(CompletionItem {
                candidate: format!("{}::*{}", h.rel_path, h.title),
                description: desc,
            });
        }
    }
    if !needle.is_empty() {
        out.retain(|item| {
            item.candidate.to_ascii_lowercase().contains(&needle)
                || item.description.to_ascii_lowercase().contains(&needle)
        });
    }
    out
}

/// Completes `org-roam` v2 node titles, aliases, `id:<uuid>`, and bare `<uuid>` tokens.
fn complete_roam_node_domain(prefix: &str, ws_root: &Path) -> Vec<CompletionItem> {
    let Ok((cache, _)) = WorkspaceCache::open_and_sync(ws_root) else {
        return Vec::new();
    };
    let Ok(rows) = cache.complete_ids(prefix) else {
        return Vec::new();
    };
    let needle = prefix.trim().to_ascii_lowercase();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for row in rows.into_iter().filter(|r| !r.roam_excluded) {
        let desc = format!("{} ({})", row.title, row.rel_path);
        for cand in std::iter::once(row.title.clone())
            .chain(row.aliases)
            .chain([format!("id:{}", row.id), row.id])
        {
            if (needle.is_empty()
                || cand.to_ascii_lowercase().contains(&needle)
                || desc.to_ascii_lowercase().contains(&needle))
                && seen.insert(cand.clone())
            {
                out.push(CompletionItem {
                    candidate: cand,
                    description: desc.clone(),
                });
            }
        }
    }
    out
}

/// Completes active and done TODO keywords merged from `.org-tools.toml`, SQLite, and defaults.
fn complete_todo_state_domain(prefix: &str, ws_root: &Path) -> Vec<CompletionItem> {
    let pfx = prefix.trim().to_ascii_lowercase();
    let mut active = Vec::new();
    let mut done = Vec::new();
    if let Ok(cfg) = Config::load_for_dir(ws_root) {
        if let Some(tk) = cfg.todo.to_todo_keywords() {
            active.extend(tk.todo);
            done.extend(tk.done);
        }
    }
    if let Ok((cache, _)) = WorkspaceCache::open_and_sync(ws_root) {
        if let Ok((c_act, c_done)) = cache.complete_todo_keywords() {
            active.extend(c_act);
            done.extend(c_done);
        }
    }
    for d in ["TODO", "NEXT", "WAIT", "WAITING", "HOLD", "BLOCKED"] {
        active.push(d.to_string());
    }
    for d in ["DONE", "CANCELED", "CANCELLED"] {
        done.push(d.to_string());
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (kw, label) in active
        .into_iter()
        .map(|k| (k, "active state"))
        .chain(done.into_iter().map(|k| (k, "done state")))
    {
        if (pfx.is_empty() || kw.to_ascii_lowercase().starts_with(&pfx)) && seen.insert(kw.clone())
        {
            out.push(CompletionItem {
                candidate: kw,
                description: label.to_string(),
            });
        }
    }
    out
}

/// Completes workspace `:tags:` or `@context` tags from SQLite.
fn complete_tag_domain(prefix: &str, ws_root: &Path, context_only: bool) -> Vec<CompletionItem> {
    let Ok((cache, _)) = WorkspaceCache::open_and_sync(ws_root) else {
        return Vec::new();
    };
    let query_pfx = if context_only { "" } else { prefix };
    let Ok(tags) = cache.complete_tags(query_pfx) else {
        return Vec::new();
    };
    let needle = prefix.trim().trim_start_matches('@').to_ascii_lowercase();
    let desc = if context_only {
        "GTD context tag"
    } else {
        "workspace tag"
    };
    tags.into_iter()
        .filter(|t| !context_only || t.starts_with('@'))
        .filter(|t| {
            needle.is_empty()
                || t.trim_start_matches('@')
                    .to_ascii_lowercase()
                    .starts_with(&needle)
        })
        .map(|t| CompletionItem {
            candidate: t,
            description: desc.to_string(),
        })
        .collect()
}

/// Completes canonical Org/roam/edna properties plus SQLite-cached workspace property keys.
fn complete_property_domain(prefix: &str, ws_root: &Path) -> Vec<CompletionItem> {
    let pfx = prefix.trim().to_ascii_lowercase();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for &(name, desc) in CANONICAL_PROPERTIES {
        if (pfx.is_empty() || name.to_ascii_lowercase().starts_with(&pfx))
            && seen.insert(name.to_string())
        {
            out.push(CompletionItem {
                candidate: name.to_string(),
                description: desc.to_string(),
            });
        }
    }
    if let Ok((cache, _)) = WorkspaceCache::open_and_sync(ws_root) {
        if let Ok(keys) = cache.complete_property_keys(prefix) {
            for key in keys {
                if seen.insert(key.clone()) {
                    out.push(CompletionItem {
                        candidate: key,
                        description: "workspace property key".to_string(),
                    });
                }
            }
        }
    }
    out
}

/// Completes `org query search` predicate prefixes and SQLite-backed values after `:`.
fn complete_query_domain(prefix: &str, ws_root: &Path) -> Vec<CompletionItem> {
    let (lead, last_tok) = match prefix.rfind(char::is_whitespace) {
        Some(idx) => (&prefix[..=idx], &prefix[idx + 1..]),
        None => ("", prefix),
    };
    if let Some((pred, val_pfx)) = last_tok.split_once(':') {
        let sub_items = match pred {
            "todo" => complete_todo_state_domain(val_pfx, ws_root),
            "tags" => complete_tag_domain(val_pfx, ws_root, false),
            "property" => complete_property_domain(val_pfx, ws_root),
            "priority" => ["A", "B", "C"]
                .into_iter()
                .map(|p| CompletionItem {
                    candidate: p.to_string(),
                    description: format!("Priority [#{p}]"),
                })
                .collect(),
            "scheduled" | "deadline" | "closed" => ["today", "past", "future", "none", "any"]
                .into_iter()
                .map(|w| CompletionItem {
                    candidate: w.to_string(),
                    description: format!("{pred} window ({w})"),
                })
                .collect(),
            "done" | "blocked" | "actionable" | "waiting" | "clocked" => ["true", "false"]
                .into_iter()
                .map(|b| CompletionItem {
                    candidate: b.to_string(),
                    description: format!("Boolean filter ({b})"),
                })
                .collect(),
            _ => Vec::new(),
        };
        let vp = val_pfx.to_ascii_lowercase();
        return sub_items
            .into_iter()
            .filter(|i| vp.is_empty() || i.candidate.to_ascii_lowercase().starts_with(&vp))
            .map(|i| CompletionItem {
                candidate: format!("{lead}{pred}:{}", i.candidate),
                description: i.description,
            })
            .collect();
    }
    let needle = last_tok.to_ascii_lowercase();
    QUERY_PREDICATE_PREFIXES
        .iter()
        .filter(|(p, _)| needle.is_empty() || p.starts_with(&needle))
        .map(|&(p, desc)| CompletionItem {
            candidate: format!("{lead}{p}"),
            description: desc.to_string(),
        })
        .collect()
}
