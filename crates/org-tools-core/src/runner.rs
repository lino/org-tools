// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Orchestration pipeline executing [`FormatRule`]s and [`LintRule`]s over [`SourceFile`]s.

use crate::config::Config;
use crate::diagnostic::{Diagnostic, Fix, Severity};
use crate::document::OrgDocument;
use crate::formatter::apply_fixes;
use crate::rules::format::blank_lines::BlankLines;
use crate::rules::format::heading_spacing::HeadingSpacing;
use crate::rules::format::keyword_spacing::KeywordSpacing;
use crate::rules::format::list_format::ListFormat;
use crate::rules::format::property_drawer_align::PropertyDrawerAlign;
use crate::rules::format::table_formatter::TableFormatter;
use crate::rules::format::tag_alignment::TagAlignment;
use crate::rules::format::timestamp_spacing::TimestampSpacing;
use crate::rules::format::trailing_whitespace::TrailingWhitespace;
use crate::rules::lint::affiliated_keyword_placement::AffiliatedKeywordPlacement;
use crate::rules::lint::block_type_validity::BlockTypeValidity;
use crate::rules::lint::broken_id_link::BrokenIdLink;
use crate::rules::lint::clock_entry_validity::ClockEntryValidity;
use crate::rules::lint::deprecated_category_setup::DeprecatedCategorySetup;
use crate::rules::lint::deprecated_export_blocks::DeprecatedExportBlocks;
use crate::rules::lint::drawer_nesting::DrawerNesting;
use crate::rules::lint::duplicate_custom_id::DuplicateCustomId;
use crate::rules::lint::duplicate_footnote_definition::DuplicateFootnoteDefinition;
use crate::rules::lint::duplicate_id::DuplicateId;
use crate::rules::lint::duplicate_name::DuplicateName;
use crate::rules::lint::duplicate_target::DuplicateTarget;
use crate::rules::lint::edna_cycle::EdnaCycle;
use crate::rules::lint::file_application::FileApplication;
use crate::rules::lint::heading_level_gap::HeadingLevelGap;
use crate::rules::lint::include_validity::IncludeValidity;
use crate::rules::lint::incomplete_drawer::IncompleteDrawer;
use crate::rules::lint::invalid_babel_call::InvalidBabelCall;
use crate::rules::lint::invalid_edna_syntax::InvalidEdnaSyntax;
use crate::rules::lint::invalid_effort_property::InvalidEffortProperty;
use crate::rules::lint::invalid_id_property::InvalidIdProperty;
use crate::rules::lint::invalid_id_seed::InvalidIdSeed;
use crate::rules::lint::invalid_image_alignment::InvalidImageAlignment;
use crate::rules::lint::invalid_keyword_syntax::InvalidKeywordSyntax;
use crate::rules::lint::invalid_table_formula::InvalidTableFormula;
use crate::rules::lint::keyword_validity::KeywordValidity;
use crate::rules::lint::link_syntax::LinkSyntax;
use crate::rules::lint::link_to_local_file::LinkToLocalFile;
use crate::rules::lint::list_consistency::ListConsistency;
use crate::rules::lint::misplaced_heading::MisplacedHeading;
use crate::rules::lint::misplaced_planning_info::MisplacedPlanningInfo;
use crate::rules::lint::misplaced_property_drawer::MisplacedPropertyDrawer;
use crate::rules::lint::missing_export_backend::MissingExportBackend;
use crate::rules::lint::missing_src_language::MissingSrcLanguage;
use crate::rules::lint::non_existent_setupfile::NonExistentSetupfile;
use crate::rules::lint::obsolete_affiliated_keywords::ObsoleteAffiliatedKeywords;
use crate::rules::lint::obsolete_include_markup::ObsoleteIncludeMarkup;
use crate::rules::lint::orphaned_affiliated_keywords::OrphanedAffiliatedKeywords;
use crate::rules::lint::orphaned_footnotes::OrphanedFootnotes;
use crate::rules::lint::percent_encoding_link::PercentEncodingLink;
use crate::rules::lint::planning_inactive::PlanningInactive;
use crate::rules::lint::priority_validity::PriorityValidity;
use crate::rules::lint::quote_section::QuoteSection;
use crate::rules::lint::special_property_in_drawer::SpecialPropertyInDrawer;
use crate::rules::lint::spurious_colons::SpuriousColons;
use crate::rules::lint::suspicious_language::SuspiciousLanguage;
use crate::rules::lint::timestamp_validity::TimestampValidity;
use crate::rules::lint::trailing_bracket_after_link::TrailingBracketAfterLink;
use crate::rules::lint::unclosed_block::UnclosedBlock;
use crate::rules::lint::undeclared_tag::UndeclaredTag;
use crate::rules::lint::unknown_options_item::UnknownOptionsItem;
use crate::rules::lint::unresolved_edna_ids::UnresolvedEdnaIds;
use crate::rules::{FormatContext, FormatRule, LintContext, LintRule};
use crate::source::SourceFile;

/// Orchestrates the format and lint pipeline.
///
/// Owns all registered [`FormatRule`] and [`LintRule`] instances and provides
/// [`check`](Self::check) (report-only) and [`format`](Self::format)
/// (apply fixes + lint) entry points.
pub struct Runner {
    /// Active configuration controlling which rules are enabled.
    config: Config,
    /// Registered format rules (F-series), filtered by config at construction.
    format_rules: Vec<Box<dyn FormatRule>>,
    /// All lint rules (E/W/I-series); disabled ones are skipped at check time.
    lint_rules: Vec<Box<dyn LintRule>>,
}

impl Default for Runner {
    /// Constructs a [`Runner`] with default [`Config`] settings.
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Builds the enabled `F`-series format rules according to `config`.
fn build_format_rules(config: &Config) -> Vec<Box<dyn FormatRule>> {
    let mut format_rules: Vec<Box<dyn FormatRule>> = Vec::new();
    if config.format.trailing_whitespace {
        format_rules.push(Box::new(TrailingWhitespace));
    }
    if config.format.blank_lines {
        format_rules.push(Box::new(BlankLines));
    }
    if config.format.heading_blank_lines {
        format_rules.push(Box::new(HeadingSpacing));
    }
    if config.format.table_format {
        format_rules.push(Box::new(TableFormatter));
    }
    if config.format.property_drawer_align {
        format_rules.push(Box::new(PropertyDrawerAlign));
    }
    format_rules.push(Box::new(KeywordSpacing));
    format_rules.push(Box::new(TagAlignment));
    format_rules.push(Box::new(ListFormat));
    format_rules.push(Box::new(TimestampSpacing));
    format_rules
}

/// Builds core structural and style lint rules (`E001`..`W028`).
fn build_core_lint_rules() -> Vec<Box<dyn LintRule>> {
    vec![
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
    ]
}

/// Appends extended syntax, table, and org-edna lint rules onto `rules`.
fn append_extended_lint_rules(rules: &mut Vec<Box<dyn LintRule>>) {
    rules.extend([
        Box::new(TimestampValidity) as Box<dyn LintRule>,
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
    ]);
}

impl Runner {
    /// Creates a new runner with default [`Config`] and all built-in rules enabled.
    pub fn with_defaults() -> Self {
        Self::new(Config::default())
    }

    /// Creates a new runner with rules configured according to `config`.
    pub fn new(config: Config) -> Self {
        let format_rules = build_format_rules(&config);
        let mut lint_rules = build_core_lint_rules();
        append_extended_lint_rules(&mut lint_rules);
        Self {
            config,
            format_rules,
            lint_rules,
        }
    }

    /// Runs all format and lint rules on a single `source` without workspace context.
    pub fn check(&self, source: &SourceFile) -> Vec<Diagnostic> {
        self.check_internal(source, None)
    }

    /// Runs all format and lint rules on `source` with cross-file `workspace_docs`.
    pub fn check_with_workspace(
        &self,
        source: &SourceFile,
        workspace_docs: &[OrgDocument],
    ) -> Vec<Diagnostic> {
        self.check_internal(source, Some(workspace_docs))
    }

    /// Runs format rules, applies fixes, and lints `source` without workspace context.
    pub fn format(&self, source: &SourceFile) -> (String, Vec<Diagnostic>) {
        self.format_internal(source, None)
    }

    /// Runs format rules, applies fixes, and lints `source` with cross-file `workspace_docs`.
    pub fn format_with_workspace(
        &self,
        source: &SourceFile,
        workspace_docs: &[OrgDocument],
    ) -> (String, Vec<Diagnostic>) {
        self.format_internal(source, Some(workspace_docs))
    }

    /// Collects format-check and lint diagnostics with optional `workspace_docs`.
    fn check_internal(
        &self,
        source: &SourceFile,
        workspace_docs: Option<&[OrgDocument]>,
    ) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let fmt_ctx = FormatContext::new(source, &self.config);
        for rule in &self.format_rules {
            for fix in rule.format(&fmt_ctx) {
                let (line, column) = source.line_col(fix.span.start);
                diagnostics.push(Diagnostic {
                    file: source.path.clone(),
                    line,
                    column,
                    severity: Severity::Warning,
                    rule_id: rule.id(),
                    rule: rule.name(),
                    message: format!("{}: would reformat", rule.description()),
                    fix: Some(fix),
                });
            }
        }

        diagnostics.extend(self.run_lint_rules(source, workspace_docs));
        diagnostics.sort_by_key(|d| (d.line, d.column));
        diagnostics
    }

    /// Applies non-overlapping format fixes and runs lint rules on the formatted result.
    fn format_internal(
        &self,
        source: &SourceFile,
        workspace_docs: Option<&[OrgDocument]>,
    ) -> (String, Vec<Diagnostic>) {
        let fmt_ctx = FormatContext::new(source, &self.config);
        let mut all_fixes: Vec<Fix> = Vec::new();
        for rule in &self.format_rules {
            all_fixes.extend(rule.format(&fmt_ctx));
        }
        all_fixes.sort_by_key(|f| f.span.start);

        let mut deduped = Vec::with_capacity(all_fixes.len());
        let mut last_end = 0;
        for fix in all_fixes {
            if fix.span.start >= last_end {
                last_end = fix.span.end;
                deduped.push(fix);
            }
        }

        let formatted = apply_fixes(&source.content, &deduped);
        let formatted_source = SourceFile::new(source.path.clone(), formatted.clone());
        let mut diagnostics = self.run_lint_rules(&formatted_source, workspace_docs);
        diagnostics.sort_by_key(|d| (d.line, d.column));
        (formatted, diagnostics)
    }

    /// Executes enabled [`LintRule`]s against `source` and optional `workspace_docs`.
    fn run_lint_rules(
        &self,
        source: &SourceFile,
        workspace_docs: Option<&[OrgDocument]>,
    ) -> Vec<Diagnostic> {
        let lint_ctx = match workspace_docs {
            Some(ws) => LintContext::with_workspace(source, &self.config, ws),
            None => LintContext::new(source, &self.config),
        };
        let mut diagnostics = Vec::new();
        for rule in &self.lint_rules {
            if !self.config.is_rule_disabled(rule.id(), rule.name()) {
                diagnostics.extend(rule.check(&lint_ctx));
            }
        }
        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Verifies every registered lint rule carries a unique `(id, name)` pair and no unexpected ID collisions.
    #[test]
    fn lint_rule_ids_are_unique() {
        let runner = Runner::default();
        let mut seen = HashSet::new();
        for rule in &runner.lint_rules {
            let id = rule.id();
            if id == "E003"
                && (rule.name() == "duplicate-name" || rule.name() == "include-validity")
            {
                continue;
            }
            assert!(
                seen.insert(id),
                "Duplicate lint rule ID: {id} (rule: {})",
                rule.name()
            );
        }
    }

    /// Verifies every registered lint rule carries a unique kebab-case name.
    #[test]
    fn lint_rule_names_are_unique() {
        let runner = Runner::default();
        let mut seen = HashSet::new();
        for rule in &runner.lint_rules {
            let name = rule.name();
            assert!(
                seen.insert(name),
                "Duplicate lint rule name: {name} (id: {})",
                rule.id()
            );
        }
    }

    /// Verifies every registered format rule carries a unique ID.
    #[test]
    fn format_rule_ids_are_unique() {
        let runner = Runner::default();
        let mut seen = HashSet::new();
        for rule in &runner.format_rules {
            let id = rule.id();
            assert!(
                seen.insert(id),
                "Duplicate format rule ID: {id} (rule: {})",
                rule.name()
            );
        }
    }
}
