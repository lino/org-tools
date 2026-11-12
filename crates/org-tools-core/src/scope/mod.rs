// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Composable project, project-landscape, forge, and tag `:SCOPE:` validity engine.

pub mod atom;
pub mod eval;
pub mod registry;

pub use atom::{extract_scope_drawer_or_meta, glob_matches, parse_scope, ScopeAtom};
pub use eval::{resolve_effective_scope, resolve_node_scope, ScopeQuery};
pub use registry::{ExpandedLandscape, LandscapeDef, ProjectDef, ScopeRegistry};

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Verifies parsing of all scope atom variants, prefix forms, and pipe delimiters.
    #[test]
    fn parse_all_scope_atom_forms_and_delimiters() {
        let raw = "global project:payments-api projects:auth,billing landscape:core landscapes:infra,cloud forge:github.com/acme/* tag:security | bare-slug";
        let atoms = parse_scope(raw);
        for expected in [
            ScopeAtom::Global,
            ScopeAtom::Project("payments-api".into()),
            ScopeAtom::Project("auth".into()),
            ScopeAtom::Project("billing".into()),
            ScopeAtom::Landscape("core".into()),
            ScopeAtom::Landscape("infra".into()),
            ScopeAtom::Landscape("cloud".into()),
            ScopeAtom::Forge("github.com/acme/*".into()),
            ScopeAtom::Tag("security".into()),
            ScopeAtom::Project("bare-slug".into()),
        ] {
            assert!(atoms.contains(&expected));
        }
        assert_eq!(parse_scope("   "), vec![ScopeAtom::Global]);
    }

    /// Verifies fallback resolution from drawer property to vulpea-meta to default global.
    #[test]
    fn drawer_and_vulpea_meta_fallback_resolution() {
        assert_eq!(
            extract_scope_drawer_or_meta(Some("project:alpha"), ""),
            "project:alpha"
        );
        let body = "Header line\n- scope :: landscape:platform\nBody text";
        assert_eq!(
            extract_scope_drawer_or_meta(None, body),
            "landscape:platform"
        );
        assert_eq!(
            extract_scope_drawer_or_meta(Some(""), "no scope here"),
            "global"
        );
    }

    /// Verifies glob matching with single and recursive wildcards.
    #[test]
    fn glob_matching_handles_single_and_double_star() {
        assert!(glob_matches(
            "github.com/acme/*",
            "github.com/acme/payments-api"
        ));
        assert!(!glob_matches(
            "github.com/acme/*",
            "github.com/acme/sub/nested"
        ));
        assert!(glob_matches(
            "gitlab.internal.corp/platform/**",
            "gitlab.internal.corp/platform/a/b/c"
        ));
        assert!(glob_matches(
            "gitlab.internal.corp/platform/**",
            "gitlab.internal.corp/platform/"
        ));
        assert!(glob_matches("**/service", "gitlab.com/group/service"));
    }

    /// Verifies TOML parsing, directory discovery, and cycle-safe landscape inclusion.
    #[test]
    fn toml_loading_and_cycle_safe_expansion() {
        let toml_data = concat!(
            "[landscapes.platform]\ndescription = \"Platform Core\"\nprojects = [\"payments-api\"]\n",
            "forges = [\"github.com/acme/*\"]\nincludes = [\"infra\"]\n",
            "[landscapes.infra]\nprojects = [\"terraform\"]\nincludes = [\"platform\"]\n",
            "[project_groups]\nlegacy = [\"auth-service\"]\n"
        );
        let reg = ScopeRegistry::from_toml_str(toml_data).expect("valid toml");
        assert_eq!(reg.landscapes.len(), 3);
        let exp = reg.expand_landscape("platform");
        assert!(exp.projects.contains("payments-api") && exp.projects.contains("terraform"));
        assert!(exp.included_landscapes.contains("infra"));
        assert_eq!(
            reg.landscapes_for_project("payments-api"),
            BTreeSet::from(["infra".into(), "platform".into()])
        );
        assert_eq!(
            reg.landscapes_for_forge("github.com/acme/payments-api"),
            BTreeSet::from(["infra".into(), "platform".into()])
        );

        let dir = tempfile::tempdir().expect("temp dir");
        let cfg = dir.path().join(".org-tools.toml");
        std::fs::write(&cfg, toml_data).expect("write toml");
        assert_eq!(
            ScopeRegistry::load_for_dir(dir.path(), None)
                .expect("load")
                .landscapes
                .len(),
            3
        );
        assert_eq!(
            ScopeRegistry::load_for_dir(dir.path(), Some(&cfg))
                .expect("load")
                .landscapes
                .len(),
            3
        );
    }

    /// Verifies scope query matching across strict mode, landscapes, forges, and expressions.
    #[test]
    fn scope_query_matching_with_strict_scope_and_filters() {
        let toml_data =
            "[landscapes.core]\nprojects = [\"api\"]\nforges = [\"github.com/acme/*\"]\n";
        let reg = ScopeRegistry::from_toml_str(toml_data).expect("valid toml");
        assert!(ScopeQuery::default().matches("global", &[], &reg));

        let strict_q = ScopeQuery {
            for_project: Some("api".into()),
            strict_scope: true,
            ..Default::default()
        };
        assert!(
            !strict_q.matches("global", &[], &reg) && strict_q.matches("project:api", &[], &reg)
        );

        let non_strict = ScopeQuery {
            for_project: Some("api".into()),
            strict_scope: false,
            ..Default::default()
        };
        assert!(non_strict.matches("global", &[], &reg));

        let l_q = ScopeQuery {
            for_landscape: Some("core".into()),
            ..Default::default()
        };
        assert!(l_q.matches("project:api", &[], &reg) && l_q.matches("landscape:core", &[], &reg));

        let f_q = ScopeQuery {
            forge_path: Some("github.com/acme/api".into()),
            ..Default::default()
        };
        assert!(
            f_q.matches("landscape:core", &[], &reg)
                && f_q.matches("forge:github.com/acme/*", &[], &reg)
        );

        let expr_q = ScopeQuery {
            scope_expr: Some("landscape:core".into()),
            ..Default::default()
        };
        assert!(
            expr_q.matches("landscape:core", &[], &reg)
                && !expr_q.matches("project:other", &[], &reg)
        );
    }

    /// Verifies negated exclusion atoms and forge prefixes alias resolution.
    #[test]
    fn negation_and_forge_prefixes_alias_resolution() {
        let toml_data = r#"
[landscapes.java-fleet]
description = "Java backend microservices"
projects = ["payment-service", "legacy-billing", "identity-service"]
forge_prefixes = ["github.com/acme/java/*"]
"#;
        let reg = ScopeRegistry::from_toml_str(toml_data).expect("valid toml");
        assert_eq!(
            reg.expand_landscape("java-fleet").forge_prefixes(),
            &["github.com/acme/java/*".to_string()]
        );

        let scope_str = "landscape:java-fleet !project:legacy-billing";
        let atoms = parse_scope(scope_str);
        assert_eq!(atoms.len(), 2);
        assert_eq!(atoms[0], ScopeAtom::Landscape("java-fleet".into()));
        assert_eq!(
            atoms[1],
            ScopeAtom::Exclude(Box::new(ScopeAtom::Project("legacy-billing".into())))
        );

        let for_legacy = ScopeQuery {
            for_project: Some("legacy-billing".into()),
            ..Default::default()
        };
        assert!(
            !for_legacy.matches(scope_str, &[], &reg),
            "legacy-billing must be excluded by !project:legacy-billing"
        );

        let for_payment = ScopeQuery {
            for_project: Some("payment-service".into()),
            ..Default::default()
        };
        assert!(
            for_payment.matches(scope_str, &[], &reg),
            "payment-service in java-fleet must match"
        );

        let strict_only = ScopeQuery {
            strict_scope: true,
            ..Default::default()
        };
        assert!(!strict_only.matches("global", &[], &reg));
        assert!(strict_only.matches("project:alpha", &[], &reg));
    }

    /// Verifies sibling and parent-child merging of `.org-tools.toml` and `kb.toml`.
    #[test]
    fn sibling_and_parent_fallback_merging() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).expect("create .git");
        std::fs::write(
            root.join(".org-tools.toml"),
            "[format]\ncolumn_width = 80\n",
        )
        .expect("write root tools");
        std::fs::write(
            root.join("kb.toml"),
            "[landscapes.billing-platform]\nprojects = [\"payments-api\"]\n",
        )
        .expect("write kb.toml");
        let sub = root.join("child");
        std::fs::create_dir_all(&sub).expect("create child dir");
        std::fs::write(
            sub.join(".org-tools.toml"),
            "[lint]\nid_scope = \"vault\"\n",
        )
        .expect("write sub tools");

        let loaded = ScopeRegistry::load_for_dir(&sub, None).expect("load from child");
        assert!(
            loaded.landscapes.contains_key("billing-platform"),
            "billing-platform from parent kb.toml must be loaded when child has format-only .org-tools.toml"
        );
        let exp = loaded.expand_landscape("billing-platform");
        assert!(exp.projects.contains("payments-api"));
    }
}
