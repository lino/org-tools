// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=545 reason="Comprehensive Zensical documentation craft, drawer indentation, and LSP/completion contract tests"

//! Automated documentation craft, Zensical navigation, `arc42`, and readability verification suite (`ORGFMT-WP-30`).

use std::fs;
use std::path::{Path, PathBuf};

// region Constants & Canonical Lists

/// Maximum word count allowed in any single prose sentence across `docs/**/*.md`.
const MAX_SENTENCE_WORDS: usize = 25;
/// Maximum sentence count allowed in any single prose paragraph across `docs/**/*.md`.
const MAX_PARAGRAPH_SENTENCES: usize = 4;
/// Maximum participant count allowed in any Mermaid `sequenceDiagram`.
const MAX_SEQUENCE_PARTICIPANTS: usize = 6;

/// Required navigation targets in `zensical.toml` relative to `docs/`.
#[rustfmt::skip]
const REQUIRED_NAV_DOCS: &[&str] = &[
    "index.md", "user/quickstart.md", "user/formatting-and-linting.md",
    "user/querying-and-agenda.md", "user/updating-clocking-and-exporting.md",
    "user/power-user-and-scripting.md", "user/schemas.md", "user/reference.md",
    "cookbook/index.md", "cookbook/gtd.md", "cookbook/zettelkasten.md",
    "cookbook/mcp-server.md", "cookbook/lsp-editor-setup.md",
    "cookbook/automation-and-ci.md", "operations/index.md", "operations/installation.md",
    "operations/docker-and-mcp-runtime.md", "operations/release-and-tap.md",
    "operations/metrics-and-troubleshooting.md", "developer/index.md",
    "developer/architecture.md", "developer/contributing.md",
    "developer/decisions.md", "glossary.md",
];

/// All 16 task, feature, cookbook, operator, and contributor guides requiring six-part scaffolding.
#[rustfmt::skip]
const SCAFFOLDED_GUIDES: &[&str] = &[
    "docs/user/quickstart.md", "docs/user/formatting-and-linting.md",
    "docs/user/querying-and-agenda.md", "docs/user/updating-clocking-and-exporting.md",
    "docs/user/power-user-and-scripting.md", "docs/user/schemas.md",
    "docs/cookbook/gtd.md", "docs/cookbook/zettelkasten.md",
    "docs/cookbook/mcp-server.md", "docs/cookbook/lsp-editor-setup.md",
    "docs/cookbook/automation-and-ci.md",
    "docs/operations/installation.md", "docs/operations/docker-and-mcp-runtime.md",
    "docs/operations/release-and-tap.md", "docs/operations/metrics-and-troubleshooting.md",
    "docs/developer/contributing.md",
];

/// Ordered H2 section headings required on every scaffolded guide.
#[rustfmt::skip]
const REQUIRED_SCAFFOLD_SECTIONS: &[&str] = &[
    "## Overview", "## Prerequisites", "## Steps",
    "## Expected Outcome", "## Power-User & Scripting Options", "## Troubleshooting",
];

/// Canonical 12 `arc42` section titles required in both `architecture.md` and `architecture.org`.
#[rustfmt::skip]
const REQUIRED_ARC42_SECTIONS: &[&str] = &[
    "Introduction and Goals", "Constraints", "Context and Scope", "Solution Strategy",
    "Building Block View", "Runtime View", "Deployment View", "Cross-Cutting Concepts",
    "Architecture Decisions", "Quality Requirements", "Risks and Technical Debt", "Glossary",
];

/// Required Mermaid diagram header tokens across the 4 `arc42` views.
#[rustfmt::skip]
const REQUIRED_MERMAID_TYPES: &[&str] = &[
    "flowchart LR", "flowchart TD", "sequenceDiagram", "flowchart TB",
];

/// Forbidden conversational filler words and phrases in documentation prose.
#[rustfmt::skip]
const FORBIDDEN_FILLER_PHRASES: &[&str] = &[
    "basically", "simply", "in order to", "it should be noted that",
    "please note that", "as mentioned earlier", "it is important to remember",
];

/// Forbidden planning-framework lingo tokens that must never appear in documentation.
#[rustfmt::skip]
const FORBIDDEN_FRAMEWORK_LINGO: &[&str] = &[
    "mabuse", "stempelamt", "uhrenmaschine", "herzmaschine", "freigabeamt", "schaltwerk",
    "moloch", "paternoster", "befundakte", "eichamt", "gesetzbuch", "babelsberg",
];

// endregion

// region Filesystem & TOML Navigation Helpers

/// Returns the workspace root directory derived from the `org` crate manifest path.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/org must reside two levels beneath the workspace root")
        .to_path_buf()
}

/// Reads a UTF-8 file relative to the workspace root.
fn read_repo_file(rel_path: &str) -> String {
    let full = workspace_root().join(rel_path);
    fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("expected {} to be readable: {e}", full.display()))
}

/// Recursively collects all files with extension `ext` under `dir` into `out`.
fn collect_files_with_ext(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() {
            collect_files_with_ext(&path, ext, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some(ext) {
            out.push(path);
        }
    }
}

/// Recursively extracts Markdown file paths from the `nav` structure in `zensical.toml`.
fn collect_nav_paths(value: &toml::Value, out: &mut Vec<String>) {
    if let Some(s) = value.as_str() {
        out.push(s.to_string());
    } else if let Some(arr) = value.as_array() {
        arr.iter().for_each(|item| collect_nav_paths(item, out));
    } else if let Some(tbl) = value.as_table() {
        tbl.values().for_each(|val| collect_nav_paths(val, out));
    }
}

// endregion

// region Markdown Scaffolding & Readability Helpers

/// Asserts that a scaffolded guide contains `# <H1>` and all six required H2 sections in order.
fn assert_guide_scaffolding(rel_path: &str, content: &str) {
    assert!(
        content.lines().any(|l| l.starts_with("# ") && l.len() > 2),
        "{rel_path} must contain a top-level `# <Title>` heading"
    );
    let mut positions = Vec::with_capacity(REQUIRED_SCAFFOLD_SECTIONS.len());
    for &sec in REQUIRED_SCAFFOLD_SECTIONS {
        let pos = content
            .find(sec)
            .unwrap_or_else(|| panic!("{rel_path} missing section `{sec}`"));
        if let Some(&(prev_sec, prev_pos)) = positions.last() {
            assert!(
                pos > prev_pos,
                "{rel_path} `{sec}` must appear after `{prev_sec}`"
            );
        }
        positions.push((sec, pos));
    }
    assert!(
        content[positions[0].1..positions[1].1].contains("**What you will learn:**"),
        "{rel_path} `## Overview` must contain `**What you will learn:**`"
    );
    assert!(
        content[positions[2].1..positions[3].1]
            .lines()
            .any(|l| l.trim_start().starts_with("1. ")),
        "{rel_path} `## Steps` must contain numbered `1. ` steps"
    );
    let has_table = content[positions[5].1..].lines().any(|line| {
        let t = line.trim();
        t.starts_with('|') && t.contains("Symptom") && t.contains("Cause") && t.contains("Fix")
    });
    assert!(
        has_table,
        "{rel_path} `## Troubleshooting` must contain a `| Symptom | Cause | Fix |` table"
    );
}

/// Updates the active fenced-block backtick count when entering or leaving a code fence.
fn update_fence_state(trimmed: &str, active_fence: &mut Option<usize>) -> bool {
    let ticks = trimmed.chars().take_while(|&c| c == '`').count();
    if ticks < 3 {
        return active_fence.is_some();
    }
    if let Some(open_ticks) = *active_fence {
        if ticks >= open_ticks && trimmed.chars().all(|c| c == '`') {
            *active_fence = None;
        }
    } else {
        *active_fence = Some(ticks);
    }
    true
}

/// Returns `true` when a trimmed Markdown line is non-prose (heading, list, table, comment, admonition).
fn is_non_prose_line(t: &str) -> bool {
    #[rustfmt::skip]
    const PREFIXES: &[&str] = &[
        "#", "|", "- ", "* ", "\\- ", "<!--", "-->", "???", "!!!", "**What you will learn:**",
    ];
    t.is_empty()
        || PREFIXES.iter().any(|p| t.starts_with(p))
        || t.chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() && t.contains(". "))
}

/// Extracts prose paragraphs from a Markdown document, skipping code fences, comments, lists, and tables.
fn extract_prose_paragraphs(content: &str) -> Vec<String> {
    let (mut paragraphs, mut current) = (Vec::new(), Vec::new());
    let (mut active_fence, mut in_comment) = (None, false);
    for trimmed in content.lines().map(str::trim) {
        if trimmed.starts_with("<!--") || in_comment {
            in_comment = !trimmed.contains("-->");
            continue;
        }
        if update_fence_state(trimmed, &mut active_fence) || is_non_prose_line(trimmed) {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(trimmed.to_string());
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs
}

/// Replaces inline code spans (`` `...` ``) with a single token so periods inside paths do not split sentences.
fn collapse_inline_code(text: &str) -> String {
    let (mut out, mut in_code) = (String::with_capacity(text.len()), false);
    for ch in text.chars() {
        if ch == '`' {
            if !in_code {
                out.push_str("CODE");
            }
            in_code = !in_code;
        } else if !in_code {
            out.push(ch);
        }
    }
    out
}

/// Splits a normalized paragraph into individual sentences terminated by `.`, `!`, or `?`.
fn split_sentences(paragraph: &str) -> Vec<String> {
    let cleaned = collapse_inline_code(paragraph);
    let (mut sentences, mut current) = (Vec::new(), String::new());
    let mut chars = cleaned.chars().peekable();
    while let Some(ch) = chars.next() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?') && chars.peek().is_none_or(|n| n.is_whitespace()) {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        sentences.push(current.trim().to_string());
    }
    sentences
}

/// Asserts heading hierarchy never skips levels (`#` -> `##` -> `###` -> `####`) outside code fences.
fn assert_no_skipped_headings(rel_path: &str, content: &str) {
    let (mut active_fence, mut prev_level) = (None, 0usize);
    for (idx, trimmed) in content.lines().map(str::trim).enumerate() {
        if update_fence_state(trimmed, &mut active_fence) {
            continue;
        }
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if hashes > 0 && trimmed.chars().nth(hashes) == Some(' ') {
            assert!(
                prev_level == 0 || hashes <= prev_level + 1,
                "{rel_path}:{} skipped heading H{prev_level} -> H{hashes}: `{trimmed}`",
                idx + 1
            );
            prev_level = hashes;
        }
    }
}

/// Asserts paragraph sentence density (`<= 4`), sentence word count (`<= 25`), and zero filler phrases.
fn assert_prose_readability_bounds(rel_path: &str, content: &str) {
    for para in extract_prose_paragraphs(content) {
        let lower = para.to_ascii_lowercase();
        for &filler in FORBIDDEN_FILLER_PHRASES {
            assert!(
                !lower.contains(filler),
                "{rel_path} contains filler `{filler}` in: {para}"
            );
        }
        let sentences = split_sentences(&para);
        assert!(
            sentences.len() <= MAX_PARAGRAPH_SENTENCES,
            "{rel_path} paragraph has {} sentences: {para}",
            sentences.len()
        );
        for sentence in sentences {
            let words = sentence.split_whitespace().count();
            assert!(
                words <= MAX_SENTENCE_WORDS,
                "{rel_path} sentence has {words} words: {sentence}"
            );
        }
    }
}

/// Extracts fenced Mermaid diagram bodies from Markdown or Org-mode documents.
fn extract_mermaid_blocks(content: &str) -> Vec<String> {
    let (mut blocks, mut current, mut in_mermaid) = (Vec::new(), Vec::new(), false);
    for trimmed in content.lines().map(str::trim) {
        let lower = trimmed.to_ascii_lowercase();
        if !in_mermaid
            && (lower.starts_with("```mermaid") || lower.starts_with("#+begin_src mermaid"))
        {
            in_mermaid = true;
            current.clear();
        } else if in_mermaid && (trimmed == "```" || lower.starts_with("#+end_src")) {
            in_mermaid = false;
            blocks.push(current.join("\n"));
        } else if in_mermaid {
            current.push(trimmed.to_string());
        }
    }
    blocks
}

/// Asserts every `???` or `!!!` drawer has a 4-space indented body and zero escaped `\-` bullets.
fn assert_admonition_drawers(rel_path: &str, content: &str) {
    let lines: Vec<&str> = content.lines().collect();
    let mut active_fence = None;
    for (idx, &raw) in lines.iter().enumerate() {
        let trimmed = raw.trim();
        let line_no = idx + 1;
        assert!(!trimmed.starts_with("````"), "{rel_path}:{line_no} fence");
        assert!(!trimmed.starts_with("\\- "), "{rel_path}:{line_no} escaped");
        if !update_fence_state(trimmed, &mut active_fence)
            && (raw.starts_with("??? ") || raw.starts_with("!!!"))
        {
            let body = lines[line_no..].iter().find(|l| !l.trim().is_empty());
            assert!(
                body.is_some_and(|l| l.starts_with("    ")),
                "{rel_path}:{line_no} drawer `{trimmed}` requires 4-space indent"
            );
        }
    }
}

// endregion

// region Behavioural Test Slices

/// Verifies `zensical.toml` parses cleanly, configures the Three-Manual + Glossary tree, and resolves all paths.
#[test]
fn zensical_toml_configures_three_manuals_cookbook_and_glossary() {
    let raw = read_repo_file("zensical.toml");
    assert!(raw.contains("SPDX-License-Identifier: GPL-3.0-or-later"));
    let parsed: toml::Table = raw.parse().expect("zensical.toml must be valid TOML");
    let nav = parsed["project"]
        .as_table()
        .and_then(|p| p.get("nav"))
        .expect("[project].nav required");
    let mut nav_paths = Vec::new();
    collect_nav_paths(nav, &mut nav_paths);
    for &required in REQUIRED_NAV_DOCS {
        assert!(
            nav_paths.iter().any(|p| p == required),
            "zensical.toml nav missing `{required}`"
        );
    }
    let docs_root = workspace_root().join("docs");
    for rel in &nav_paths {
        assert!(
            docs_root.join(rel).is_file(),
            "zensical.toml nav missing file docs/{rel}"
        );
    }
}

/// Verifies all 16 User, Cookbook, Operator, and Contributor guides follow the six-part scaffolding template.
#[test]
fn all_sixteen_guides_conform_to_six_part_topic_scaffolding() {
    for &guide in SCAFFOLDED_GUIDES {
        assert_guide_scaffolding(guide, &read_repo_file(guide));
    }
}

/// Verifies LSP editor cookbook coverage, metrics runbook entries, and glossary definitions (`ORGFMT-WP-35`).
#[test]
fn lsp_cookbook_metrics_runbook_and_glossary_meet_contract() {
    let cookbook = read_repo_file("docs/cookbook/lsp-editor-setup.md");
    for editor in [
        "nvim-lspconfig",
        "languages.toml",
        "VS Code",
        "Zed",
        "eglot",
    ] {
        assert!(cookbook.contains(editor), "LSP cookbook missing `{editor}`");
    }
    let metrics = read_repo_file("docs/operations/metrics-and-troubleshooting.md");
    #[rustfmt::skip]
    let metric_tokens = [
        "orgfmt_lsp_requests_total", "orgfmt_lsp_request_duration_seconds",
        "orgfmt_lsp_diagnostics_published_total", "ORG_LSP_TELEMETRY_DIR",
    ];
    for token in metric_tokens {
        assert!(metrics.contains(token), "metrics runbook missing `{token}`");
    }
    let glossary = read_repo_file("docs/glossary.md");
    for term in [
        "Language Server Protocol",
        "16-Bit Unicode Transformation Format",
    ] {
        assert!(glossary.contains(term), "glossary missing `{term}`");
    }
}

/// Verifies shell completion documentation across power-user, installation, reference, usage.org, and README.org (`ORGFMT-WP-37`).
#[test]
fn shell_completion_docs_cover_bash_zsh_fish_and_dynamic_domains() {
    let power_user = read_repo_file("docs/user/power-user-and-scripting.md");
    #[rustfmt::skip]
    let pu_tokens = [
        "eval \"$(org --completions zsh)\"", "org --completions fish | source",
        "eval \"$(org --completions bash)\"", "roam-node", "todo-state", "date-spec",
    ];
    for token in pu_tokens {
        assert!(power_user.contains(token), "missing `{token}`");
    }
    let install = read_repo_file("docs/operations/installation.md");
    #[rustfmt::skip]
    let inst_tokens = [
        "eval \"$(org --completions zsh)\"", "org --completions fish | source",
        "eval \"$(org --completions bash)\"",
    ];
    for token in inst_tokens {
        assert!(install.contains(token), "installation.md missing `{token}`");
    }
    assert!(read_repo_file("docs/user/reference.md").contains("org complete"));
    let usage_org = read_repo_file("docs/user/usage.org");
    assert!(usage_org.contains("eval \"$(org --completions zsh)\""));
    assert!(usage_org.contains("org --completions fish | source"));
    assert!(read_repo_file("README.org").contains("eval \"$(org --completions zsh)\""));
}

/// Verifies `docs/developer/architecture.md` and `docs/technical/architecture.org` contain 12 `arc42` sections and 4 Mermaid views.
#[test]
fn architecture_md_and_org_contain_twelve_arc42_sections_and_four_mermaid_diagrams() {
    for rel in [
        "docs/developer/architecture.md",
        "docs/technical/architecture.org",
    ] {
        let content = read_repo_file(rel);
        let lower = content.to_ascii_lowercase();
        let mut last_pos = 0usize;
        for &sec in REQUIRED_ARC42_SECTIONS {
            let needle = sec.to_ascii_lowercase();
            let pos = lower[last_pos..]
                .find(&needle)
                .unwrap_or_else(|| panic!("{rel} missing arc42 section `{sec}` in order"));
            last_pos += pos + needle.len();
        }
        let blocks = extract_mermaid_blocks(&content);
        assert!(blocks.len() >= 4, "{rel} needs >= 4 Mermaid diagrams");
        for &kind in REQUIRED_MERMAID_TYPES {
            assert!(
                blocks.iter().any(|b| b.contains(kind)),
                "{rel} missing `{kind}` diagram"
            );
        }
        for block in blocks.iter().filter(|b| b.starts_with("sequenceDiagram")) {
            let count = block
                .lines()
                .filter(|l| l.starts_with("participant "))
                .count();
            assert!(
                count <= MAX_SEQUENCE_PARTICIPANTS,
                "{rel} sequenceDiagram has {count} participants"
            );
        }
    }
}

/// Verifies SPDX headers, heading hierarchy, sentence/paragraph readability bounds, and zero framework lingo across docs.
#[test]
fn markdown_docs_obey_spdx_headings_sentence_and_paragraph_bounds_and_zero_lingo() {
    let root = workspace_root();
    let mut md_files = Vec::new();
    collect_files_with_ext(&root.join("docs"), "md", &mut md_files);
    for path in &md_files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let content = fs::read_to_string(path).expect("read markdown file");
        assert!(
            content.contains("SPDX-License-Identifier: GPL-3.0-or-later"),
            "{rel} missing SPDX header"
        );
        assert_no_skipped_headings(&rel, &content);
        assert_admonition_drawers(&rel, &content);
        assert_prose_readability_bounds(&rel, &content);
    }
    let index_md = read_repo_file("docs/index.md");
    assert!(index_md.contains("??? info \"How It Works\""));
    assert!(!index_md
        .to_ascii_lowercase()
        .contains("executable topology"));
    assert!(
        md_files.len() >= REQUIRED_NAV_DOCS.len(),
        "expected >= {} Markdown docs, found {}",
        REQUIRED_NAV_DOCS.len(),
        md_files.len()
    );
    let mut all_docs = md_files;
    collect_files_with_ext(&root.join("docs"), "org", &mut all_docs);
    all_docs.extend([
        root.join("README.org"),
        root.join("man/org.1"),
        root.join("man/org-tools.toml.5"),
    ]);
    for path in all_docs {
        let Ok(lower) = fs::read_to_string(&path).map(|s| s.to_ascii_lowercase()) else {
            continue;
        };
        for &term in FORBIDDEN_FRAMEWORK_LINGO {
            assert!(
                !lower.contains(term),
                "{} leaked framework lingo `{term}`",
                path.display()
            );
        }
    }
}

// endregion
