// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behavioural unit tests for `org-roam` v2 node extraction, link scanning, and graph rendering.

use crate::document::OrgDocument;
use crate::roam::{
    extract_roam_links, extract_roam_nodes, scan_id_links, split_roam_tokens, RoamIndex,
};
use crate::source::SourceFile;

/// Verifies shell-quoted multi-word aliases, escaped quotes, and bare tokens split accurately.
#[test]
fn split_roam_tokens_handles_quotes_escapes_and_bare_words() {
    let raw = r#""orgfmt Work Packages" bare-alias "Quote \"Inside\" Here" https://orgmode.org"#;
    let tokens = split_roam_tokens(raw);
    assert_eq!(
        tokens,
        vec![
            "orgfmt Work Packages",
            "bare-alias",
            r#"Quote "Inside" Here"#,
            "https://orgmode.org",
        ]
    );
}

/// Verifies level-0 file nodes and nested heading nodes extract metadata, tags, `olp`, aliases, and refs.
#[test]
fn extracts_level_zero_and_heading_nodes_with_inherited_tags_and_olp() {
    let content = concat!(
        ":PROPERTIES:\n",
        ":ID:           f0000000-0000-4000-8000-000000000001\n",
        ":ID_SEED:      urn:babelsberg:orgfmt:file:spec\n",
        ":ROAM_ALIASES: \"SPEC-ORGFMT-01\" spec-alias\n",
        ":ROAM_ALIASES+: \"Second Alias\"\n",
        ":ROAM_REFS:    https://orgmode.org \"cite:knuth1984\"\n",
        ":END:\n",
        "#+TITLE: Agent Engine Spec\n",
        "#+FILETAGS: :orgfmt:spec:\n\n",
        "* Parent Section :arch:\n",
        "** NEXT [#A] Child Node :roam:spec:\n",
        ":PROPERTIES:\n",
        ":ID:           20000000-0000-4000-8000-000000000010\n",
        ":ROAM_ALIASES: \"Child Alias\"\n",
        ":END:\n",
        "Body prose.\n",
    );
    let source = SourceFile::new("specs/agent.org", content.to_string());
    let doc = OrgDocument::from_source(&source);
    let nodes = extract_roam_nodes(&doc);

    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].id, "f0000000-0000-4000-8000-000000000001");
    assert_eq!(
        nodes[0].id_seed.as_deref(),
        Some("urn:babelsberg:orgfmt:file:spec")
    );
    assert_eq!(nodes[0].title, "Agent Engine Spec");
    assert_eq!(nodes[0].level, 0);
    assert_eq!(nodes[0].line, 1);
    assert_eq!(nodes[0].tags, vec!["orgfmt", "spec"]);
    assert!(nodes[0].olp.is_empty());
    assert_eq!(
        nodes[0].aliases,
        vec!["SPEC-ORGFMT-01", "spec-alias", "Second Alias"]
    );
    assert_eq!(nodes[0].refs, vec!["https://orgmode.org", "cite:knuth1984"]);

    assert_eq!(nodes[1].id, "20000000-0000-4000-8000-000000000010");
    assert_eq!(nodes[1].title, "Child Node");
    assert_eq!(nodes[1].level, 2);
    assert_eq!(nodes[1].todo_keyword.as_deref(), Some("NEXT"));
    assert_eq!(nodes[1].priority.as_deref(), Some("A"));
    assert_eq!(nodes[1].tags, vec!["orgfmt", "spec", "arch", "roam"]);
    assert_eq!(nodes[1].olp, vec!["Parent Section"]);
    assert_eq!(nodes[1].aliases, vec!["Child Alias"]);
}

/// Verifies `:ROAM_EXCLUDE: t` suppresses a file node while still indexing non-excluded child headings.
#[test]
fn roam_exclude_suppresses_excluded_drawer_only_and_falls_back_to_stem() {
    let content = concat!(
        ":PROPERTIES:\n",
        ":ID:           file-excluded\n",
        ":ROAM_EXCLUDE: t\n",
        ":END:\n",
        "* Included Heading\n",
        ":PROPERTIES:\n",
        ":ID:           heading-included\n",
        ":END:\n",
        "* Excluded Heading\n",
        ":PROPERTIES:\n",
        ":ID:           heading-excluded\n",
        ":roam_exclude: T\n",
        ":END:\n",
    );
    let source = SourceFile::new("notes/stem_fallback.org", content.to_string());
    let doc = OrgDocument::from_source(&source);
    let nodes = extract_roam_nodes(&doc);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].id, "heading-included");

    let file_only = SourceFile::new(
        "notes/my-note.org",
        ":PROPERTIES:\n:ID: stem-id\n:END:\nBody.\n".to_string(),
    );
    let file_nodes = extract_roam_nodes(&OrgDocument::from_source(&file_only));
    assert_eq!(file_nodes.len(), 1);
    assert_eq!(file_nodes[0].title, "my-note");
}

/// Verifies link scanning skips protected blocks, comments, drawers, fixed-width lines, and inline verbatim/code spans.
#[test]
fn scan_and_attribute_links_skips_protected_and_inline_verbatim_spans() {
    let content = concat!(
        ":PROPERTIES:\n",
        ":ID:   file-node\n",
        ":NOTE: [[id:inside-drawer]]\n",
        ":END:\n",
        "#+TITLE: Link Scanner\n",
        "# [[id:comment-link]]\n",
        ": [[id:fixed-width-link]]\n",
        "Preamble prose with =[[id:verbatim-fake]]= and ~[[id:code-fake]]~ and `[[id:backtick-fake]]`.\n",
        "3. *Standard =[[id:<uuid>][Description]]= Links*: see [[id:target-a::*Sub][Target =A=]].\n",
        "#+begin_src org\n",
        "[[id:inside-src-block]]\n",
        "#+end_src\n",
        "* Section Node\n",
        ":PROPERTIES:\n",
        ":ID: sec-node\n",
        ":END:\n",
        "** Unindexed Subheading\n",
        "Nested body linking [[id:target-b]] inside sec-node subtree.\n",
    );
    let source = SourceFile::new("links.org", content.to_string());
    let doc = OrgDocument::from_source(&source);

    let raw_links = scan_id_links(&source);
    assert_eq!(raw_links.len(), 2);
    assert_eq!(raw_links[0].target_id, "target-a");
    assert_eq!(raw_links[0].description.as_deref(), Some("Target =A="));
    assert_eq!(raw_links[1].target_id, "target-b");
    assert_eq!(raw_links[1].description, None);

    let roam_links = extract_roam_links(&source, &doc);
    assert_eq!(roam_links.len(), 2);
    assert_eq!(roam_links[0].source_id.as_deref(), Some("file-node"));
    assert_eq!(roam_links[0].target_id, "target-a");
    assert_eq!(roam_links[1].source_id.as_deref(), Some("sec-node"));
    assert_eq!(roam_links[1].target_id, "target-b");
}

/// Verifies `RoamIndex` resolves queries, computes backlinks/links, and renders safe Mermaid and DOT graphs.
#[test]
fn roam_index_resolves_backlinks_outgoing_and_renders_mermaid_dot() {
    let s1 = SourceFile::new(
        "a.org",
        concat!(
            ":PROPERTIES:\n:ID: node-a\n:ROAM_ALIASES: \"Alpha <1>\"\n:END:\n",
            "#+TITLE: Alpha \"Main\" <Tag>\n",
            "See [[id:node-b][Beta Desc]] and [[id:missing-id]].\n",
        )
        .to_string(),
    );
    let s2 = SourceFile::new(
        "b.org",
        ":PROPERTIES:\n:ID: node-b\n:END:\n#+TITLE: Beta Node\n".to_string(),
    );
    let index = RoamIndex::from_sources(&[s2, s1]);

    assert_eq!(index.resolve_nodes("alpha <1>", false).len(), 1);
    let target_b = index.resolve_nodes("id:node-b", true);
    assert_eq!(target_b.len(), 1);

    let backlinks = index.backlinks_for(&target_b);
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[0].source_id.as_deref(), Some("node-a"));
    assert_eq!(
        backlinks[0].source_title.as_deref(),
        Some("Alpha \"Main\" <Tag>")
    );

    let source_a = index.resolve_nodes("node-a", true);
    let outgoing = index.outgoing_links_for(&source_a);
    assert_eq!(outgoing.len(), 2);
    assert!(outgoing[0].resolved);
    assert!(!outgoing[1].resolved);

    let mermaid = index.render_mermaid();
    assert!(mermaid.starts_with("flowchart LR\n"));
    assert!(mermaid.contains("N0[\""));
    assert!(mermaid.contains("N0 --> N1"));
    assert!(!mermaid.contains("<Tag>"));

    let dot = index.render_dot();
    assert!(dot.starts_with("digraph org_roam {\n"));
    assert!(dot.contains("N0 -> N1;"));
}

/// Verifies an unclosed `[[` on a line does not swallow a subsequent valid `[[id:...]]` link.
#[test]
fn unclosed_double_bracket_does_not_swallow_subsequent_id_link() {
    let content = concat!(
        ":PROPERTIES:\n:ID: file-node\n:END:\n",
        "See [[draft and [[id:target-a][Real]] plus [[other [[id:target-b]].\n",
    );
    let source = SourceFile::new("unclosed.org", content.to_string());
    let doc = OrgDocument::from_source(&source);

    let raw_links = scan_id_links(&source);
    assert_eq!(raw_links.len(), 2);
    assert_eq!(raw_links[0].target_id, "target-a");
    assert_eq!(raw_links[0].description.as_deref(), Some("Real"));
    assert_eq!(raw_links[0].column, 17);
    assert_eq!(raw_links[1].target_id, "target-b");
    assert_eq!(raw_links[1].description, None);

    let roam_links = extract_roam_links(&source, &doc);
    assert_eq!(roam_links.len(), 2);
    assert_eq!(roam_links[0].target_id, "target-a");
    assert_eq!(roam_links[1].target_id, "target-b");
}

/// Verifies Mermaid and DOT graph renderers safely escape quotes, parens, brackets, and trailing `\`.
#[test]
fn mermaid_and_dot_escape_quotes_brackets_parens_and_trailing_backslash() {
    let s1 = SourceFile::new(
        "win.org",
        concat!(
            ":PROPERTIES:\n:ID: node-win\n:END:\n",
            "#+TITLE: Path \"v2\" (Core) [Draft] C:\\\n",
        )
        .to_string(),
    );
    let index = RoamIndex::from_sources(&[s1]);

    let mermaid = index.render_mermaid();
    assert!(
        mermaid.contains("N0[\"Path #quot;v2#quot; (Core) [Draft] C:#92;\"]"),
        "unexpected mermaid output: {mermaid}"
    );
    assert!(!mermaid.contains("\\\"]"));

    let dot = index.render_dot();
    assert!(
        dot.contains("N0 [label=\"Path \\\"v2\\\" (Core) [Draft] C:\\\\\"];"),
        "unexpected dot output: {dot}"
    );
}
