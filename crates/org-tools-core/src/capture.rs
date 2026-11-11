// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Capture template resolution, variable interpolation, and `org-roam` v2 note rendering.
//!
//! Spec:
//! - Org Mode Manual §10.1.3 (*Capture templates*): <https://orgmode.org/manual/Capture-templates.html>
//! - org-roam v2 Manual (*The Templating System*): <https://www.orgroam.com/manual.html>

mod builtin;
mod render;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cache::RoamWorkspace;
use crate::config::Config;
use crate::logbook::{current_local_date, parse_env_now};
use crate::planning::day_of_week_abbrev;

pub use self::builtin::{builtin_capture_templates, find_template, resolve_effective_templates};
pub use self::render::{
    append_daily_capture_log, expand_template_vars, format_aligned_properties_drawer,
    format_roam_token_list, mint_capture_id, render_capture_entry, render_capture_file,
    slugify_title, with_temp_capture_file,
};

/// Resolves `(vault_name, vault_path, vault_config)` for `workspace` and `roam_dir_arg`.
pub fn resolve_target_vault(
    workspace: Option<&Path>,
    roam_dir_arg: Option<&str>,
) -> Result<(String, PathBuf, Config), String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let ws_root = workspace.unwrap_or(&cwd);
    let ws_cfg = Config::load_for_dir(ws_root).map_err(|e| e.to_string())?;
    let (vault_name, vault_path) = if let Some(sel) = roam_dir_arg {
        let Some((opt_name, path)) = ws_cfg.resolve_roam_dir(sel) else {
            return Err(format!("unknown roam directory: {sel}"));
        };
        (opt_name.unwrap_or_else(|| sel.to_string()), path)
    } else if let Some(def) = ws_cfg.default_roam_dir() {
        def
    } else {
        ("default".to_string(), ws_root.to_path_buf())
    };
    let vault_cfg = Config::load_for_roam_dir(ws_root, &vault_path).map_err(|e| e.to_string())?;
    Ok((vault_name, vault_path, vault_cfg))
}

/// Resolves a node query against `vault_path` into an `[[id:<uuid>][<Title>]]` link when matched.
pub fn resolve_roam_link_or_raw(
    vault_name: &str,
    vault_path: &Path,
    query: &str,
    no_cache: bool,
) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.starts_with("[[") {
        return trimmed.to_string();
    }
    if vault_path.is_dir() {
        let pair = [(vault_name.to_string(), vault_path.to_path_buf())];
        if let Ok(ws) = RoamWorkspace::open_named(&pair, no_cache) {
            if let Ok(nodes) = ws.resolve(trimmed) {
                if let Some(first) = nodes.first() {
                    return format!("[[id:{}][{}]]", first.id, first.title);
                }
            }
        }
    }
    trimmed.to_string()
}

/// Returns current local `(year, month, day, hour, minute, second)`, honoring `ORG_TOOLS_NOW`.
pub fn current_capture_datetime() -> (i32, u32, u32, u32, u32, u32) {
    if let Some((y, m, d, hh, mm)) = parse_env_now() {
        return (y as i32, m as u32, d as u32, hh, mm, 0);
    }
    let (y, m, d) = current_local_date();
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let rem = (secs % 86_400) as u32;
    (
        y as i32,
        m as u32,
        d as u32,
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60,
    )
}

/// Target granularity of a capture template (`file` node vs. heading `entry`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureTargetKind {
    /// Creates or opens a level-0 `org-roam` v2 file node.
    #[default]
    File,
    /// Appends a heading entry with its own minted `:ID:` into an existing or new `.org` file.
    Entry,
}

/// Configuration hierarchy layer from which a capture template originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureTemplateLayer {
    /// Built-in default template shipped with `org-tools`.
    #[default]
    Builtin,
    /// Global user configuration (`$ORG_TOOLS_CONFIG` or `$XDG_CONFIG_HOME/org-tools/config.toml`).
    Global,
    /// Workspace-level `.org-tools.toml`.
    Workspace,
    /// Per-directory `<roam_dir>/.org-tools.toml` inside a specific vault.
    Vault,
}

/// Raw `[capture.templates.<name>]` table parsed from `.org-tools.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureTemplateConfig {
    /// Human-readable description shown by `org roam capture --list-templates`.
    pub description: Option<String>,
    /// Optional short key selector usable with `--template|-t <key>`.
    pub keys: Option<String>,
    /// Relative path template inside the selected `roam_dir`.
    pub target: Option<String>,
    /// Target kind (`"file"` or `"entry"`).
    pub kind: Option<CaptureTargetKind>,
    /// Optional target heading when `kind = "entry"` (e.g., `"* Inbox"`).
    pub target_heading: Option<String>,
    /// Optional `:TYPE:` property value in `:PROPERTIES:`.
    pub roam_type: Option<String>,
    /// Additional `:PROPERTIES:` key-value pairs supporting `${...}` expansion.
    pub properties: BTreeMap<String, String>,
    /// Filetags (when `kind = "file"`) or heading tags (when `kind = "entry"`).
    pub filetags: Vec<String>,
    /// Optional `#+TODO:` header line emitted into newly created files.
    pub todo_header: Option<String>,
    /// Optional extra preamble lines emitted into newly created files.
    pub head: Option<String>,
    /// Inline Org body template supporting `${...}` variable interpolation.
    pub body: Option<String>,
    /// Path (relative to the declaring config directory or `~/`) to an Org body scaffold file.
    pub body_file: Option<String>,
    /// Optional default controlling whether `$VISUAL` / `$EDITOR` is spawned.
    pub edit: Option<bool>,
    /// Configuration layer that last defined or overrode this template.
    #[serde(skip)]
    pub source_layer: CaptureTemplateLayer,
    /// Directory containing the `.org-tools.toml` file that defined `body_file`.
    #[serde(skip)]
    pub base_dir: Option<PathBuf>,
}

/// Top-level `[capture]` configuration table in `.org-tools.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    /// Named `[capture.templates.<name>]` entries.
    pub templates: BTreeMap<String, CaptureTemplateConfig>,
}

/// Resolved, effective capture template ready for listing or instantiation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveCaptureTemplate {
    /// Canonical template identifier.
    pub name: String,
    /// Human-readable summary description.
    pub description: String,
    /// Optional short key selector.
    pub keys: Option<String>,
    /// Target granularity (`file` or `entry`).
    pub kind: CaptureTargetKind,
    /// Relative destination path template inside `roam_dir`.
    pub target: String,
    /// Optional parent heading for `kind = "entry"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_heading: Option<String>,
    /// Optional `:TYPE:` drawer property value.
    pub roam_type: Option<String>,
    /// Additional `:PROPERTIES:` pairs.
    pub properties: BTreeMap<String, String>,
    /// Filetags or heading tags.
    pub filetags: Vec<String>,
    /// Optional `#+TODO:` header string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo_header: Option<String>,
    /// Optional extra preamble lines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Inline body template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Optional external body template path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_file: Option<String>,
    /// Optional default editor launch flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<bool>,
    /// Source layer (`builtin`, `global`, `workspace`, or `vault`).
    pub source_layer: CaptureTemplateLayer,
    /// Base directory for resolving `body_file`.
    #[serde(skip)]
    pub base_dir: Option<PathBuf>,
}

/// Variable bindings expanded into `${...}` placeholders during template rendering.
#[derive(Debug, Clone, Default)]
pub struct TemplateVars {
    /// Minted `:ID:` UUID string.
    pub id: String,
    /// Note or entry title.
    pub title: String,
    /// Lowercase hyphenated slug derived from `title` or `--slug`.
    pub slug: String,
    /// ISO date string (`YYYY-MM-DD`).
    pub date: String,
    /// Four-digit year string (`YYYY`).
    pub year: String,
    /// Two-digit month string (`MM`).
    pub month: String,
    /// Compact 14-digit timestamp (`YYYYMMDDHHMMSS`).
    pub timestamp: String,
    /// Inactive Org timestamp (`[YYYY-MM-DD Day HH:MM]`).
    pub timestamp_inactive: String,
    /// Active Org timestamp (`<YYYY-MM-DD Day HH:MM>`).
    pub timestamp_active: String,
    /// Selected vault name or directory string.
    pub roam_dir: String,
    /// User-supplied body text.
    pub body: String,
    /// Optional resolved source medium string or `[[id:...]]` link.
    pub medium: String,
    /// Optional source location string (page, chapter, URL anchor).
    pub location: String,
    /// Optional resolved parent Zettel `[[id:...]]` link.
    pub parent: String,
    /// Additional `--var KEY=VALUE` bindings.
    pub custom: BTreeMap<String, String>,
}

impl TemplateVars {
    /// Constructs [`TemplateVars`] from identity, title, vault name, and `(Y, M, D, h, m, s)`.
    pub fn new(
        id: &str,
        title: &str,
        slug: &str,
        roam_dir: &str,
        dt: (i32, u32, u32, u32, u32, u32),
    ) -> Self {
        let (y, m, d, hh, mm, ss) = dt;
        let dow = day_of_week_abbrev(y as u16, m as u8, d as u8);
        let date = format!("{y:04}-{m:02}-{d:02}");
        Self {
            id: id.to_string(),
            title: title.to_string(),
            slug: slug.to_string(),
            year: format!("{y:04}"),
            month: format!("{m:02}"),
            timestamp: format!("{y:04}{m:02}{d:02}{hh:02}{mm:02}{ss:02}"),
            timestamp_inactive: format!("[{date} {dow} {hh:02}:{mm:02}]"),
            timestamp_active: format!("<{date} {dow} {hh:02}:{mm:02}>"),
            date,
            roam_dir: roam_dir.to_string(),
            ..Self::default()
        }
    }
}

/// Additional CLI options applied when rendering a capture template into Org text.
#[derive(Debug, Clone, Default)]
pub struct CaptureRenderInput {
    /// Extra tags merged with the template's `filetags`.
    pub extra_tags: Vec<String>,
    /// `:ROAM_ALIASES:` tokens.
    pub aliases: Vec<String>,
    /// `:ROAM_REFS:` tokens.
    pub refs: Vec<String>,
    /// Resolved `[[id:<uuid>][<Title>]]` links for `--parent` and `--link`.
    pub related_links: Vec<String>,
}
