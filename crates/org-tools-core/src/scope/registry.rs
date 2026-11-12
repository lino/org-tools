// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Project landscapes, forge prefixes, and scope hierarchy resolution.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::atom::glob_matches;

// region Definitions & Raw Config

/// Definition of a project landscape loaded from configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandscapeDef {
    /// Human-readable description of the landscape purpose.
    #[serde(default)]
    pub description: Option<String>,
    /// Member project slugs assigned directly to this landscape.
    #[serde(default)]
    pub projects: Vec<String>,
    /// Forge URL or path glob patterns belonging to this landscape.
    #[serde(default, alias = "forge_prefixes")]
    pub forges: Vec<String>,
    /// Slugs of child landscapes transitively included by this landscape.
    #[serde(default)]
    pub includes: Vec<String>,
}

/// Definition of a project configuration entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDef {
    /// Human-readable description of the project purpose.
    #[serde(default)]
    pub description: Option<String>,
    /// Configured repository forge path or prefix (e.g. `github.com/acme/project`).
    #[serde(default)]
    pub forge: Option<String>,
}

/// Raw representation used when deserializing `.org-tools.toml` or `kb.toml`.
#[derive(Deserialize)]
struct RawScopeConfig {
    #[serde(default)]
    landscapes: BTreeMap<String, LandscapeDef>,
    #[serde(default)]
    projects: BTreeMap<String, ProjectDef>,
    #[serde(default)]
    project_groups: BTreeMap<String, Vec<String>>,
}

/// Fully expanded landscape view with all transitive `includes` resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpandedLandscape {
    /// Canonical lowercase slug of the landscape.
    pub slug: String,
    /// Description inherited from the base landscape definition.
    pub description: Option<String>,
    /// Set of all member project slugs (including transitively included projects).
    pub projects: BTreeSet<String>,
    /// List of all forge glob patterns (including transitively included forges).
    pub forges: Vec<String>,
    /// Set of all transitively included landscape slugs.
    pub included_landscapes: BTreeSet<String>,
}

impl ExpandedLandscape {
    /// Reference to the forge glob patterns or prefixes for this landscape.
    pub fn forge_prefixes(&self) -> &[String] {
        &self.forges
    }
}

// endregion

// region Scope Registry

/// Registry of known project landscapes with recursive cycle-safe expansion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeRegistry {
    /// Registered landscapes indexed by canonical lowercase slug.
    pub landscapes: BTreeMap<String, LandscapeDef>,
    /// Registered projects indexed by canonical lowercase slug.
    pub projects: BTreeMap<String, ProjectDef>,
}

impl ScopeRegistry {
    /// Parses landscapes and legacy project groups from raw TOML text.
    pub fn from_toml_str(content: &str) -> Result<Self, String> {
        let raw: RawScopeConfig = toml::from_str(content).map_err(|e| e.to_string())?;
        let mut landscapes = BTreeMap::new();
        for (k, mut def) in raw.landscapes {
            def.projects = def
                .projects
                .into_iter()
                .map(|s| s.to_ascii_lowercase())
                .collect();
            def.includes = def
                .includes
                .into_iter()
                .map(|s| s.to_ascii_lowercase())
                .collect();
            landscapes.insert(k.to_ascii_lowercase(), def);
        }
        for (k, projs) in raw.project_groups {
            let key = k.to_ascii_lowercase();
            let entry = landscapes.entry(key).or_default();
            for p in projs {
                let p_lower = p.to_ascii_lowercase();
                if !entry.projects.contains(&p_lower) {
                    entry.projects.push(p_lower);
                }
            }
        }
        let mut projects = BTreeMap::new();
        for (k, def) in raw.projects {
            projects.insert(k.to_ascii_lowercase(), def);
        }
        Ok(Self {
            landscapes,
            projects,
        })
    }

    /// Merges fallback landscape definition into an existing landscape definition.
    fn merge_landscape_def(existing: &mut LandscapeDef, fallback: LandscapeDef) {
        if existing.description.is_none() {
            existing.description = fallback.description;
        }
        for p in fallback.projects {
            if !existing.projects.contains(&p) {
                existing.projects.push(p);
            }
        }
        for f in fallback.forges {
            if !existing.forges.contains(&f) {
                existing.forges.push(f);
            }
        }
        for inc in fallback.includes {
            if !existing.includes.contains(&inc) {
                existing.includes.push(inc);
            }
        }
    }

    /// Merges fallback registry into `self`, preserving closer entries while adding missing items.
    pub fn merge_fallback(&mut self, parent_or_sibling: ScopeRegistry) {
        for (name, fallback_def) in parent_or_sibling.landscapes {
            match self.landscapes.entry(name) {
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(fallback_def);
                }
                std::collections::btree_map::Entry::Occupied(mut e) => {
                    Self::merge_landscape_def(e.get_mut(), fallback_def);
                }
            }
        }
        for (name, fallback_def) in parent_or_sibling.projects {
            self.projects.entry(name).or_insert(fallback_def);
        }
    }

    /// Discovers and loads scope configuration from `explicit_config` or ancestor directories.
    pub fn load_for_dir(dir: &Path, explicit_config: Option<&Path>) -> Result<Self, String> {
        if let Some(explicit) = explicit_config {
            let content = std::fs::read_to_string(explicit)
                .map_err(|e| format!("scope config not found: {}: {e}", explicit.display()))?;
            return Self::from_toml_str(&content);
        }
        let start = if dir.is_file() {
            dir.parent().unwrap_or(dir)
        } else {
            dir
        };
        let mut combined = Self::default();
        for d in start.ancestors() {
            for name in [".org-tools.toml", "kb.toml"] {
                let candidate = d.join(name);
                if candidate.is_file() {
                    let content = std::fs::read_to_string(&candidate).map_err(|e| e.to_string())?;
                    let reg = Self::from_toml_str(&content)?;
                    combined.merge_fallback(reg);
                }
            }
            if d.join(".git").exists() {
                break;
            }
        }
        Ok(combined)
    }

    /// Recursively expands all transitive `includes` for `slug` with cycle protection.
    pub fn expand_landscape(&self, slug: &str) -> ExpandedLandscape {
        let key = slug.to_ascii_lowercase();
        let mut exp = ExpandedLandscape {
            slug: key.clone(),
            ..Default::default()
        };
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        visited.insert(key.clone());
        if let Some(def) = self.landscapes.get(&key) {
            exp.description = def.description.clone();
            exp.forges.extend(def.forges.clone());
            exp.projects.extend(def.projects.iter().cloned());
            queue.extend(def.includes.iter().cloned());
        }
        while let Some(inc) = queue.pop_front() {
            if !visited.insert(inc.clone()) {
                continue;
            }
            exp.included_landscapes.insert(inc.clone());
            if let Some(sub) = self.landscapes.get(&inc) {
                exp.projects.extend(sub.projects.iter().cloned());
                exp.forges.extend(sub.forges.iter().cloned());
                for nxt in &sub.includes {
                    if !visited.contains(nxt) {
                        queue.push_back(nxt.clone());
                    }
                }
            }
        }
        exp
    }

    /// Determines all landscapes whose expanded projects contain `project`.
    pub fn landscapes_for_project(&self, project: &str) -> BTreeSet<String> {
        let p_lower = project.to_ascii_lowercase();
        let mut out = BTreeSet::new();
        for slug in self.landscapes.keys() {
            if self.expand_landscape(slug).projects.contains(&p_lower) {
                out.insert(slug.clone());
            }
        }
        out
    }

    /// Determines all landscapes whose expanded forge patterns match `forge_path`.
    pub fn landscapes_for_forge(&self, forge_path: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for slug in self.landscapes.keys() {
            if self
                .expand_landscape(slug)
                .forges
                .iter()
                .any(|f| glob_matches(f, forge_path))
            {
                out.insert(slug.clone());
            }
        }
        out
    }
}

// endregion
