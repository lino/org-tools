// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scope filtering, predicate matching, and multi-dimensional query evaluation.

use super::atom::{glob_matches, parse_scope, ScopeAtom};
use super::registry::ScopeRegistry;
use crate::document::OrgDocument;
use crate::locator::FILE_NODE_ENTRY_INDEX;

// region Node Scope Resolution

/// Finds a value in a key-value map using ASCII case-insensitive key lookup.
fn find_ci<'a>(map: &'a std::collections::HashMap<String, String>, key: &str) -> Option<&'a str> {
    map.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.trim())
        .filter(|s| !s.is_empty())
}

/// Finds a value in a vulpea-meta map using ASCII case-insensitive key lookup.
fn find_vulpea_ci<'a>(
    map: &'a std::collections::HashMap<String, Vec<String>>,
    key: &str,
) -> Option<&'a str> {
    map.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, vs)| vs.first().map(|s| s.trim()))
        .filter(|s| !s.is_empty())
}

/// Resolves the raw scope string from an [`OrgDocument`] and entry index if declared.
pub fn resolve_node_scope(doc: &OrgDocument, entry_idx: usize) -> Option<String> {
    if entry_idx == FILE_NODE_ENTRY_INDEX {
        if let Some(s) = find_ci(&doc.file_properties, "SCOPE") {
            return Some(s.to_string());
        }
        if let Some(s) = find_vulpea_ci(&doc.vulpea_meta, "scope") {
            return Some(s.to_string());
        }
        return None;
    }
    if let Some(entry) = doc.entries.get(entry_idx) {
        if let Some(s) = find_ci(&entry.properties, "SCOPE") {
            return Some(s.to_string());
        }
        if let Some(s) = find_vulpea_ci(&entry.vulpea_meta, "scope") {
            return Some(s.to_string());
        }
    }
    if let Some(s) = find_ci(&doc.file_properties, "SCOPE") {
        return Some(s.to_string());
    }
    find_vulpea_ci(&doc.vulpea_meta, "scope").map(str::to_string)
}

/// Resolves the effective scope string from an [`OrgDocument`] and entry index, defaulting to `"global"`.
pub fn resolve_effective_scope(doc: &OrgDocument, entry_idx: usize) -> String {
    resolve_node_scope(doc, entry_idx).unwrap_or_else(|| "global".to_string())
}

// endregion

// region Scope Query

/// Scope filter criteria matching nodes against projects, landscapes, and forges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeQuery {
    /// Explicit scope expression constraint (e.g. `global` or `landscape:platform`).
    pub scope_expr: Option<String>,
    /// Target project slug to filter against.
    pub for_project: Option<String>,
    /// Target landscape slug to filter against.
    pub for_landscape: Option<String>,
    /// Target repository forge path (e.g. `github.com/acme/repo`).
    pub forge_path: Option<String>,
    /// When true, excludes `global` nodes unless explicitly matching the scope target.
    pub strict_scope: bool,
}

impl ScopeQuery {
    /// Returns true when no filtering constraints or scope expressions are active.
    pub fn is_empty(&self) -> bool {
        self.scope_expr.is_none()
            && self.for_project.is_none()
            && self.for_landscape.is_none()
            && self.forge_path.is_none()
            && !self.strict_scope
    }

    /// Evaluates whether a raw scope string and node tags satisfy this query.
    pub fn matches(&self, raw: &str, effective_tags: &[String], reg: &ScopeRegistry) -> bool {
        let atoms = parse_scope(raw);
        self.matches_node(&atoms, effective_tags, reg)
    }

    /// Evaluates whether parsed scope atoms and node tags satisfy this query.
    pub fn matches_node(&self, atoms: &[ScopeAtom], tags: &[String], reg: &ScopeRegistry) -> bool {
        if self.is_empty() {
            return true;
        }
        let mut positive = Vec::new();
        let mut exclusions = Vec::new();
        for a in atoms {
            match a {
                ScopeAtom::Exclude(inner) => exclusions.push(inner.as_ref()),
                pos => positive.push(pos),
            }
        }
        if positive.is_empty() {
            positive.push(&ScopeAtom::Global);
        }
        for exc in exclusions {
            if exclusion_matches_query(exc, self, tags, reg) {
                return false;
            }
        }
        if let Some(ref expr) = self.scope_expr {
            let q_atoms = parse_scope(expr);
            let pos_atoms: Vec<ScopeAtom> = positive.iter().map(|&a| a.clone()).collect();
            if !atoms_intersect(&pos_atoms, &q_atoms, reg) {
                return false;
            }
        }
        let has_ctx = self.for_project.is_some()
            || self.for_landscape.is_some()
            || self.forge_path.is_some()
            || self.strict_scope;
        if !has_ctx {
            return true;
        }
        positive
            .iter()
            .any(|a| atom_matches_context(a, self, tags, reg))
    }
}

// endregion

// region Internal Evaluation Helpers

/// Checks whether any node atom matches any query atom in the given registry.
fn atoms_intersect(
    node_atoms: &[ScopeAtom],
    query_atoms: &[ScopeAtom],
    reg: &ScopeRegistry,
) -> bool {
    node_atoms.iter().any(|na| {
        query_atoms.iter().any(|qa| match (na, qa) {
            (ScopeAtom::Global, ScopeAtom::Global) => true,
            (ScopeAtom::Project(p1), ScopeAtom::Project(p2)) => p1.eq_ignore_ascii_case(p2),
            (ScopeAtom::Landscape(l1), ScopeAtom::Landscape(l2)) => {
                l1.eq_ignore_ascii_case(l2)
                    || reg
                        .expand_landscape(l2)
                        .included_landscapes
                        .contains(&l1.to_ascii_lowercase())
            }
            (ScopeAtom::Forge(f1), ScopeAtom::Forge(f2)) => {
                f1.eq_ignore_ascii_case(f2) || glob_matches(f2, f1)
            }
            (ScopeAtom::Tag(t1), ScopeAtom::Tag(t2)) => t1.eq_ignore_ascii_case(t2),
            _ => false,
        })
    })
}

/// Matches forge atom against target context (forge_path, landscape, or project forge).
fn match_forge_context(glob: &str, q: &ScopeQuery, reg: &ScopeRegistry) -> bool {
    if q.forge_path
        .as_deref()
        .is_some_and(|f| glob_matches(glob, f))
    {
        return true;
    }
    if let Some(fl) = q.for_landscape.as_deref() {
        if reg
            .expand_landscape(fl)
            .forges
            .iter()
            .any(|f| f.eq_ignore_ascii_case(glob) || glob_matches(glob, f) || glob_matches(f, glob))
        {
            return true;
        }
    }
    if let Some(fp) = q.for_project.as_deref() {
        if let Some(pdef) = reg.projects.get(&fp.to_ascii_lowercase()) {
            if let Some(ref pf) = pdef.forge {
                if glob_matches(glob, pf) {
                    return true;
                }
            }
        }
    }
    false
}

/// Evaluates a single atom against the contextual filters of `query`.
fn atom_matches_context(
    atom: &ScopeAtom,
    q: &ScopeQuery,
    tags: &[String],
    reg: &ScopeRegistry,
) -> bool {
    let has_target_context =
        q.for_project.is_some() || q.for_landscape.is_some() || q.forge_path.is_some();
    if !has_target_context {
        return match atom {
            ScopeAtom::Global => !q.strict_scope,
            ScopeAtom::Exclude(inner) => !atom_matches_context(inner, q, tags, reg),
            _ => true,
        };
    }
    match atom {
        ScopeAtom::Global => !q.strict_scope,
        ScopeAtom::Project(p) => {
            if q.for_project
                .as_deref()
                .is_some_and(|fp| p.eq_ignore_ascii_case(fp))
            {
                return true;
            }
            q.for_landscape.as_deref().is_some_and(|fl| {
                reg.expand_landscape(fl)
                    .projects
                    .contains(&p.to_ascii_lowercase())
            })
        }
        ScopeAtom::Landscape(l) => match_landscape_context(l, q, reg),
        ScopeAtom::Forge(glob) => match_forge_context(glob, q, reg),
        ScopeAtom::Tag(t) => tags.iter().any(|tag| tag.eq_ignore_ascii_case(t)),
        ScopeAtom::Exclude(inner) => !atom_matches_context(inner, q, tags, reg),
    }
}

/// Evaluates whether an exclusion constraint specifically targets the active query parameters.
fn exclusion_matches_query(
    atom: &ScopeAtom,
    q: &ScopeQuery,
    tags: &[String],
    reg: &ScopeRegistry,
) -> bool {
    match atom {
        ScopeAtom::Global => q.strict_scope,
        ScopeAtom::Project(p) => q
            .for_project
            .as_deref()
            .is_some_and(|fp| p.eq_ignore_ascii_case(fp)),
        ScopeAtom::Landscape(l) => q.for_landscape.as_deref().is_some_and(|fl| {
            l.eq_ignore_ascii_case(fl)
                || reg
                    .expand_landscape(fl)
                    .included_landscapes
                    .contains(&l.to_ascii_lowercase())
        }),
        ScopeAtom::Forge(glob) => q
            .forge_path
            .as_deref()
            .is_some_and(|f| glob_matches(glob, f)),
        ScopeAtom::Tag(t) => tags.iter().any(|tag| tag.eq_ignore_ascii_case(t)),
        ScopeAtom::Exclude(_) => false,
    }
}

/// Evaluates landscape atom membership against query target contexts.
fn match_landscape_context(l: &str, q: &ScopeQuery, reg: &ScopeRegistry) -> bool {
    if q.for_landscape.is_none() && q.for_project.is_none() && q.forge_path.is_none() {
        return true;
    }
    if let Some(fl) = q.for_landscape.as_deref() {
        let (fl_low, l_low) = (fl.to_ascii_lowercase(), l.to_ascii_lowercase());
        if l_low == fl_low
            || reg
                .expand_landscape(fl)
                .included_landscapes
                .contains(&l_low)
        {
            return true;
        }
    }
    let exp_l = reg.expand_landscape(l);
    if let Some(fp) = q.for_project.as_deref() {
        let fp_low = fp.to_ascii_lowercase();
        if exp_l.projects.contains(&fp_low) {
            return true;
        }
        if let Some(pdef) = reg.projects.get(&fp_low) {
            if let Some(ref pf) = pdef.forge {
                if exp_l.forges.iter().any(|f| glob_matches(f, pf)) {
                    return true;
                }
            }
        }
    }
    q.forge_path
        .as_deref()
        .is_some_and(|forge| exp_l.forges.iter().any(|f| glob_matches(f, forge)))
}

// endregion
