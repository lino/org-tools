// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hidden `org complete` dynamic shell completion subcommand and script emitters.

pub(crate) mod candidates;
pub(crate) mod scripts;

use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::{Args, ValueHint};
use clap_complete::Shell;

use org_tools_core::export_md::{normalize_workspace_path, WORKSPACE_ENV_VAR};
use org_tools_core::rules::format::{
    blank_lines::BlankLines, heading_spacing::HeadingSpacing, keyword_spacing::KeywordSpacing,
    list_format::ListFormat, property_drawer_align::PropertyDrawerAlign,
    table_formatter::TableFormatter, tag_alignment::TagAlignment,
    timestamp_spacing::TimestampSpacing, trailing_whitespace::TrailingWhitespace,
};
use org_tools_core::rules::lint::{
    affiliated_keyword_placement::AffiliatedKeywordPlacement,
    block_type_validity::BlockTypeValidity, broken_id_link::BrokenIdLink,
    clock_entry_validity::ClockEntryValidity, deprecated_category_setup::DeprecatedCategorySetup,
    deprecated_export_blocks::DeprecatedExportBlocks, drawer_nesting::DrawerNesting,
    duplicate_custom_id::DuplicateCustomId,
    duplicate_footnote_definition::DuplicateFootnoteDefinition, duplicate_id::DuplicateId,
    duplicate_name::DuplicateName, duplicate_target::DuplicateTarget, edna_cycle::EdnaCycle,
    file_application::FileApplication, heading_level_gap::HeadingLevelGap,
    include_validity::IncludeValidity, incomplete_drawer::IncompleteDrawer,
    invalid_babel_call::InvalidBabelCall, invalid_edna_syntax::InvalidEdnaSyntax,
    invalid_effort_property::InvalidEffortProperty, invalid_id_property::InvalidIdProperty,
    invalid_id_seed::InvalidIdSeed, invalid_image_alignment::InvalidImageAlignment,
    invalid_keyword_syntax::InvalidKeywordSyntax, invalid_table_formula::InvalidTableFormula,
    keyword_validity::KeywordValidity, link_syntax::LinkSyntax,
    link_to_local_file::LinkToLocalFile, list_consistency::ListConsistency,
    misplaced_heading::MisplacedHeading, misplaced_planning_info::MisplacedPlanningInfo,
    misplaced_property_drawer::MisplacedPropertyDrawer,
    missing_export_backend::MissingExportBackend, missing_src_language::MissingSrcLanguage,
    non_existent_setupfile::NonExistentSetupfile,
    obsolete_affiliated_keywords::ObsoleteAffiliatedKeywords,
    obsolete_include_markup::ObsoleteIncludeMarkup,
    orphaned_affiliated_keywords::OrphanedAffiliatedKeywords,
    orphaned_footnotes::OrphanedFootnotes, percent_encoding_link::PercentEncodingLink,
    planning_inactive::PlanningInactive, priority_validity::PriorityValidity,
    quote_section::QuoteSection, special_property_in_drawer::SpecialPropertyInDrawer,
    spurious_colons::SpuriousColons, suspicious_language::SuspiciousLanguage,
    timestamp_validity::TimestampValidity, trailing_bracket_after_link::TrailingBracketAfterLink,
    unclosed_block::UnclosedBlock, undeclared_tag::UndeclaredTag,
    unknown_options_item::UnknownOptionsItem, unresolved_edna_ids::UnresolvedEdnaIds,
};
use org_tools_core::rules::{FormatRule, LintRule};
use org_tools_core::state::{civil_from_days, days_from_civil};

use self::candidates::collect_domain_candidates;
pub(crate) use self::scripts::emit_completions;
use crate::{EXIT_ERROR, EXIT_OK};

/// Ancestor markers inspected when discovering a default workspace root from `cwd`.
const WORKSPACE_MARKERS: &[&str] = &[".git", "kb.toml", ".org-tools.toml", ".fritz.toml"];

/// Relative planning date tokens (`token`, `day_delta`, `label`).
const RELATIVE_DATE_SPECS: &[(&str, i64, &str)] = &[
    ("today", 0, "Today"),
    (".", 0, "Today (shorthand)"),
    ("+1d", 1, "Tomorrow (+1 day)"),
    ("+3d", 3, "In 3 days"),
    ("+1w", 7, "In 1 week (+7 days)"),
    ("+2w", 14, "In 2 weeks (+14 days)"),
    ("+1m", 30, "In 1 month (+30 days)"),
    ("-1d", -1, "Yesterday (-1 day)"),
];

/// CLI arguments for the hidden `org complete` subcommand.
#[derive(Debug, Clone, Args)]
pub(crate) struct CompleteArgs {
    /// Dynamic completion domain (`id`, `locator`, `roam-node`, `todo-state`, `tag`, `context`, `property`, `date-spec`, `query`, `rule`).
    pub(crate) kind: String,

    /// Current word prefix being completed on the command line.
    #[arg(default_value = "", allow_hyphen_values = true)]
    pub(crate) prefix: String,

    /// Optional workspace root directory override.
    #[arg(long, value_hint = ValueHint::DirPath)]
    pub(crate) workspace: Option<PathBuf>,

    /// Target shell formatting (`bash`, `zsh`, `fish`).
    #[arg(long, value_enum, default_value_t = Shell::Zsh)]
    pub(crate) shell: Shell,
}

/// A single completion candidate paired with its human-readable description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionItem {
    /// Token inserted on the command line when selected.
    pub(crate) candidate: String,
    /// Single-line description displayed by `zsh` (`_describe`) and `fish` (`complete -a`).
    pub(crate) description: String,
}

/// Discovers the active workspace root directory from `--workspace`, `ORG_TOOLS_WORKSPACE`, or `cwd`.
pub(crate) fn resolve_completion_workspace(explicit: Option<&Path>) -> PathBuf {
    if let Some(ws) = explicit.filter(|p| p.is_dir()) {
        return ws.to_path_buf();
    }
    if let Some(env_ws) = std::env::var_os(WORKSPACE_ENV_VAR).filter(|v| !v.is_empty()) {
        let env_path = PathBuf::from(env_ws);
        if env_path.is_dir() {
            return env_path;
        }
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let norm = normalize_workspace_path(&cwd);
    let mut cur: Option<&Path> = Some(&norm);
    while let Some(dir) = cur {
        if WORKSPACE_MARKERS.iter().any(|m| dir.join(m).exists()) {
            return dir.to_path_buf();
        }
        cur = dir.parent();
    }
    cwd
}

/// Completes relative planning date specifications with resolved `YYYY-MM-DD` descriptions.
pub(crate) fn complete_date_spec_domain(prefix: &str) -> Vec<CompletionItem> {
    let pfx = prefix.trim().to_ascii_lowercase();
    let (y, m, d) = crate::date::current_date();
    let base_days = days_from_civil(y, m, d);
    RELATIVE_DATE_SPECS
        .iter()
        .filter(|(tok, _, _)| pfx.is_empty() || tok.to_ascii_lowercase().starts_with(&pfx))
        .map(|&(tok, delta, label)| {
            let (ry, rm, rd) = civil_from_days(base_days + delta);
            CompletionItem {
                candidate: tok.to_string(),
                description: format!("{label} ({ry:04}-{rm:02}-{rd:02})"),
            }
        })
        .collect()
}

/// Returns all built-in format and lint rule `(id, name, description)` triples.
fn all_rule_metadata() -> Vec<(&'static str, &'static str, &'static str)> {
    let fmt_rules: Vec<Box<dyn FormatRule>> = vec![
        Box::new(TrailingWhitespace),
        Box::new(BlankLines),
        Box::new(HeadingSpacing),
        Box::new(TableFormatter),
        Box::new(PropertyDrawerAlign),
        Box::new(KeywordSpacing),
        Box::new(TagAlignment),
        Box::new(ListFormat),
        Box::new(TimestampSpacing),
    ];
    let lint_rules: Vec<Box<dyn LintRule>> = vec![
        Box::new(UnclosedBlock),
        Box::new(DuplicateCustomId),
        Box::new(DuplicateName),
        Box::new(DuplicateTarget),
        Box::new(DuplicateFootnoteDefinition),
        Box::new(IncompleteDrawer),
        Box::new(EdnaCycle),
        Box::new(HeadingLevelGap),
        Box::new(MissingSrcLanguage),
        Box::new(MisplacedPropertyDrawer),
        Box::new(OrphanedFootnotes),
        Box::new(InvalidKeywordSyntax),
        Box::new(ObsoleteAffiliatedKeywords),
        Box::new(DeprecatedExportBlocks),
        Box::new(DeprecatedCategorySetup),
        Box::new(TrailingBracketAfterLink),
        Box::new(InvalidEffortProperty),
        Box::new(InvalidIdProperty),
        Box::new(SpuriousColons),
        Box::new(MissingExportBackend),
        Box::new(PlanningInactive),
        Box::new(MisplacedPlanningInfo),
        Box::new(FileApplication),
        Box::new(ObsoleteIncludeMarkup),
        Box::new(QuoteSection),
        Box::new(SpecialPropertyInDrawer),
        Box::new(MisplacedHeading),
        Box::new(SuspiciousLanguage),
        Box::new(OrphanedAffiliatedKeywords),
        Box::new(NonExistentSetupfile),
        Box::new(LinkToLocalFile),
        Box::new(InvalidBabelCall),
        Box::new(UnknownOptionsItem),
        Box::new(PercentEncodingLink),
        Box::new(InvalidImageAlignment),
        Box::new(TimestampValidity),
        Box::new(LinkSyntax),
        Box::new(ListConsistency),
        Box::new(PriorityValidity),
        Box::new(DrawerNesting),
        Box::new(ClockEntryValidity),
        Box::new(KeywordValidity),
        Box::new(BlockTypeValidity),
        Box::new(AffiliatedKeywordPlacement),
        Box::new(UndeclaredTag),
        Box::new(InvalidTableFormula),
        Box::new(InvalidEdnaSyntax),
        Box::new(UnresolvedEdnaIds),
        Box::new(DuplicateId),
        Box::new(InvalidIdSeed),
        Box::new(BrokenIdLink),
        Box::new(IncludeValidity),
    ];
    fmt_rules
        .iter()
        .map(|r| (r.id(), r.name(), r.description()))
        .chain(
            lint_rules
                .iter()
                .map(|r| (r.id(), r.name(), r.description())),
        )
        .collect()
}

/// Completes format (`F001..F009`) and lint (`E001..E010`, `W001..W037`, `I001..I004`) rule IDs and names.
pub(crate) fn complete_rule_domain(prefix: &str) -> Vec<CompletionItem> {
    let needle = prefix.trim().to_ascii_lowercase();
    let mut out = Vec::new();
    for (id, name, desc) in all_rule_metadata() {
        if needle.is_empty()
            || id.to_ascii_lowercase().starts_with(&needle)
            || name.to_ascii_lowercase().contains(&needle)
        {
            out.push(CompletionItem {
                candidate: id.to_string(),
                description: format!("{name} — {desc}"),
            });
            out.push(CompletionItem {
                candidate: name.to_string(),
                description: format!("{id} — {desc}"),
            });
        }
    }
    out
}

/// Sanitizes tabs and newlines out of a candidate description string.
fn sanitize_description(raw: &str) -> String {
    raw.replace(['\t', '\r', '\n'], " ").trim().to_string()
}

/// Executes `org complete <kind> [prefix] [--workspace <dir>] [--shell <shell>]`.
pub(crate) fn run_complete(args: CompleteArgs) -> i32 {
    let ws_root = resolve_completion_workspace(args.workspace.as_deref());
    let Some(items) = collect_domain_candidates(&args.kind, &args.prefix, &ws_root) else {
        eprintln!("org: unknown completion domain '{}'", args.kind);
        return EXIT_ERROR;
    };

    let mut out = std::io::stdout().lock();
    for item in items {
        let cand = item.candidate.replace(['\r', '\n'], "");
        if cand.is_empty() {
            continue;
        }
        match args.shell {
            Shell::Bash => {
                let _ = writeln!(out, "{cand}");
            }
            _ => {
                let desc = sanitize_description(&item.description);
                let _ = writeln!(out, "{cand}\t{desc}");
            }
        }
    }
    EXIT_OK
}
