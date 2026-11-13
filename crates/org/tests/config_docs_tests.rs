// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural contract tests for configuration documentation, opinionated directory structure
//! documentation, `.org-tools.toml.example`, and `man/org-tools.toml.5` parity.

use std::fs;
use std::path::{Path, PathBuf};

use org_tools_core::capture::resolve_effective_templates;
use org_tools_core::config::Config;
use org_tools_core::gtd_roam::BASE_GTD_FILENAMES;
use org_tools_core::scope::ScopeRegistry;
use tempfile::TempDir;

/// Returns the workspace root directory derived from `CARGO_MANIFEST_DIR`.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/org must sit two levels below workspace root")
        .to_path_buf()
}

/// Reads a UTF-8 file relative to the workspace root.
fn read_repo_file(rel_path: &str) -> String {
    let full = workspace_root().join(rel_path);
    fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("expected {} to be readable: {e}", full.display()))
}

/// Verifies `docs/user/configuration-reference.md` documents every TOML section, built-in template,
/// template variable, and runtime environment variable.
#[test]
fn configuration_reference_covers_all_toml_tables_templates_and_env_vars() {
    let reference = read_repo_file("docs/user/configuration-reference.md");
    for section in [
        "[format]",
        "[lint]",
        "[lint.zettel]",
        "[lint.authoring]",
        "[todo]",
        "[agenda]",
        "[roam]",
        "[roam.directories]",
        "[capture.templates.<name>]",
        "[landscapes.<slug>]",
        "[projects.<slug>]",
        "project_groups",
    ] {
        assert!(
            reference.contains(section),
            "configuration-reference.md missing section `{section}`"
        );
    }

    for key in [
        "trailing_whitespace",
        "blank_lines",
        "max_consecutive_blank_lines",
        "heading_blank_lines",
        "heading_blank_lines_before",
        "table_format",
        "property_drawer_align",
        "disabled_rules",
        "enable",
        "id_scope",
        "forbid_tasks",
        "forbid_ephemeral_headers",
        "max_table_cell_chars",
        "sequences",
        "deadline_warning_days",
        "default_directory",
        "gtd_subdir",
        "zettel_subdir",
        "fleeting_subdir",
        "daily_subdir",
        "editor",
        "log_to_daily",
        "target_heading",
        "roam_type",
        "body_file",
    ] {
        assert!(
            reference.contains(key),
            "configuration-reference.md missing key `{key}`"
        );
    }

    for tpl in [
        "zettel",
        "concept",
        "technical",
        "howto",
        "person",
        "place",
        "project",
        "fleeting",
        "daily",
        "inbox",
    ] {
        assert!(
            reference.contains(tpl),
            "configuration-reference.md missing built-in template `{tpl}`"
        );
    }

    for var in [
        "${id}",
        "${title}",
        "${slug}",
        "${date}",
        "${timestamp}",
        "${timestamp_inactive}",
        "${timestamp_active}",
        "${roam_dir}",
        "${body}",
        "${medium}",
        "${location}",
        "${parent}",
    ] {
        assert!(
            reference.contains(var),
            "configuration-reference.md missing template placeholder `{var}`"
        );
    }

    for env_var in [
        "ORG_TOOLS_CONFIG",
        "ORG_TOOLS_WORKSPACE",
        "ORG_ROAM_DIRECTORIES",
        "ORG_TOOLS_CACHE_PATH",
        "ORG_ID_NAMESPACE",
        "ORG_ID_PREFIX",
        "ORG_TAG_COLUMN",
        "ORG_TOOLS_NOW",
        "ORG_LSP_TELEMETRY_DIR",
        "VISUAL",
        "EDITOR",
        "NO_COLOR",
        "XDG_CONFIG_HOME",
    ] {
        assert!(
            reference.contains(env_var),
            "configuration-reference.md missing environment variable `{env_var}`"
        );
    }
}

/// Verifies `docs/user/configuration.md` explains hierarchical layering and links to the reference.
#[test]
fn configuration_guide_explains_hierarchy_and_links_to_reference() {
    let guide = read_repo_file("docs/user/configuration.md");
    for token in [
        "org-tools/config.toml",
        ".org-tools.toml",
        "[roam.directories]",
        "configuration-reference.md",
        "directory-structure.md",
        "flowchart",
    ] {
        assert!(
            guide.contains(token),
            "configuration.md missing expected token `{token}`"
        );
    }
}

/// Verifies `docs/user/directory-structure.md` covers all 5 base GTD files, 4 core subdirectories,
/// companion template folders, and dynamic `:agenda:` filetag pruning.
#[test]
fn directory_structure_guide_covers_opinionated_vault_layout_and_base_gtd_files() {
    let guide = read_repo_file("docs/user/directory-structure.md");
    for base_file in BASE_GTD_FILENAMES {
        assert!(
            guide.contains(base_file),
            "directory-structure.md missing base GTD file `{base_file}`"
        );
    }
    for subdir in [
        "gtd/",
        "fleeting/",
        "zettel/",
        "daily/",
        "tech/",
        "howto/",
        "people/",
        "places/",
        ".cache/org-cache.db",
        "sync-agenda-tags",
        "flowchart",
    ] {
        assert!(
            guide.contains(subdir),
            "directory-structure.md missing `{subdir}`"
        );
    }
}

/// Verifies `.org-tools.toml.example` and `man/org-tools.toml.5` match live `Config` deserializers
/// without stale or ignored keys.
#[test]
fn org_tools_toml_example_and_man_5_match_live_config_deserializers() {
    let example = read_repo_file(".org-tools.toml.example");
    for stale in ["target_file", "dailies_directory", "\nactive ="] {
        assert!(
            !example.contains(stale),
            ".org-tools.toml.example contains stale key `{stale}`"
        );
    }

    let tmp = TempDir::new().expect("tempdir");
    let cfg_path = tmp.path().join(".org-tools.toml");
    fs::write(&cfg_path, &example).expect("write .org-tools.toml");

    let cfg = Config::load_for_dir_with_global(tmp.path(), None)
        .expect(".org-tools.toml.example must load cleanly via Config");
    let kw = cfg
        .todo
        .to_todo_keywords()
        .expect(".org-tools.toml.example [todo] must produce TodoKeywords");
    assert!(kw.todo.contains(&"NEXT".to_string()));
    assert!(kw.done.contains(&"DONE".to_string()));

    let templates = resolve_effective_templates(&cfg);
    let inbox = templates.get("inbox").expect("inbox template");
    assert_eq!(inbox.target, "gtd/inbox.org");

    let scope = ScopeRegistry::from_toml_str(&example)
        .expect(".org-tools.toml.example must parse cleanly via ScopeRegistry");
    assert!(!scope.landscapes.is_empty());

    let man5 = read_repo_file("man/org-tools.toml.5");
    for stale in ["target_file", "dailies_directory"] {
        assert!(
            !man5.contains(stale),
            "man/org-tools.toml.5 contains stale key `{stale}`"
        );
    }
    for required in [
        "[lint.zettel]",
        "[lint.authoring]",
        "[roam.directories]",
        "[projects.<slug>]",
        "ORG_ROAM_DIRECTORIES",
    ] {
        assert!(
            man5.contains(required),
            "man/org-tools.toml.5 missing `{required}`"
        );
    }
}
