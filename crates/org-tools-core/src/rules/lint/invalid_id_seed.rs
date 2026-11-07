// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Validates deterministic Stempelamt `:ID_SEED:` -> RFC 9562 UUIDv5 `:ID:` integrity (`E010`).
//!
//! Spec:
//! - Org Mode Manual: <https://orgmode.org/manual/Property-Syntax.html>
//! - Org Syntax: <https://orgmode.org/worg/org-syntax.html#Property_Drawers>
//! - RFC 9562 Section 5.5 (UUIDv5): <https://www.rfc-editor.org/rfc/rfc9562.html#section-5.5>

use crate::diagnostic::{Diagnostic, Severity};
use crate::property::{uuid_v5, StempelamtConfig, DEFAULT_ROOT_NAMESPACE};
use crate::rules::format::regions::{is_protected, protected_regions};
use crate::rules::heading::parse_heading;
use crate::rules::{LintContext, LintRule};

/// Detects empty `:ID_SEED:` properties, missing `:ID:`s, or `:ID:` != UUIDv5(`:ID_SEED:`) mismatches (`E010`).
///
/// Citations:
/// - Org Mode Manual: [§7.1 Property Syntax](https://orgmode.org/manual/Property-Syntax.html)
/// - Org Syntax: [Property Drawers](https://orgmode.org/worg/org-syntax.html#Property_Drawers)
/// - Ecosystem Spec: [org-roam v2](https://www.orgroam.com/manual.html) &
///   [RFC 9562 §5.5](https://www.rfc-editor.org/rfc/rfc9562.html#section-5.5)
pub struct InvalidIdSeed;

/// Accumulated `:ID:` and `:ID_SEED:` state for a single `:PROPERTIES:` drawer.
#[derive(Default)]
struct DrawerSeedState {
    /// Trimmed `:ID:` value observed on this drawer, if any.
    id_val: Option<String>,
    /// Trimmed `:ID_SEED:` value together with its `(line, column)` location.
    seed_entry: Option<(String, usize, usize)>,
}

impl LintRule for InvalidIdSeed {
    /// Returns the canonical structural error rule identifier `"E010"`.
    fn id(&self) -> &'static str {
        "E010"
    }

    /// Returns the kebab-case rule slug `"invalid-id-seed"`.
    fn name(&self) -> &'static str {
        "invalid-id-seed"
    }

    /// Returns a one-line summary of the `:ID_SEED:` determinism check.
    fn description(&self) -> &'static str {
        "Detect empty :ID_SEED:, missing :ID:, or :ID_SEED: -> UUIDv5 mismatches"
    }

    /// Inspects level-0 and heading drawers carrying `:ID_SEED:` for RFC 9562 UUIDv5 parity.
    fn check(&self, ctx: &LintContext) -> Vec<Diagnostic> {
        let namespace = resolve_namespace();
        let protected = protected_regions(&ctx.source.content);
        let mut diagnostics = Vec::new();
        let mut active_drawer: Option<DrawerSeedState> = None;
        let mut offset = 0;

        for (line_idx, line) in ctx.source.content.split('\n').enumerate() {
            let raw = line.strip_suffix('\r').unwrap_or(line);
            if !is_protected(line_idx, &protected) {
                step_drawer_line(
                    ctx,
                    raw,
                    offset,
                    &namespace,
                    &mut active_drawer,
                    &mut diagnostics,
                );
            }
            offset += line.len() + 1;
        }

        diagnostics
    }
}

/// Resolves the Stempelamt root namespace UUID from environment or [`DEFAULT_ROOT_NAMESPACE`].
fn resolve_namespace() -> uuid::Uuid {
    StempelamtConfig::resolve(None, None)
        .map(|cfg| cfg.namespace)
        .unwrap_or_else(|_| {
            uuid::Uuid::parse_str(DEFAULT_ROOT_NAMESPACE).expect("valid default namespace UUID")
        })
}

/// Parses `(key, trimmed_value)` from a `:KEY: value` drawer line.
fn parse_drawer_kv(trimmed: &str) -> Option<(&str, &str)> {
    let rest = trimmed.strip_prefix(':')?;
    let colon_pos = rest.find(':')?;
    let key = &rest[..colon_pos];
    if key.is_empty() {
        return None;
    }
    Some((key, rest[colon_pos + 1..].trim()))
}

/// Updates active drawer state for a single unprotected line and emits `E010` on `:END:`.
fn step_drawer_line(
    ctx: &LintContext,
    raw: &str,
    offset: usize,
    namespace: &uuid::Uuid,
    active_drawer: &mut Option<DrawerSeedState>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let trimmed = raw.trim();
    if let Some(ref mut state) = active_drawer {
        if parse_heading(raw).is_some() {
            *active_drawer = None;
            return;
        }
        if trimmed.eq_ignore_ascii_case(":END:") {
            let finished = active_drawer.take().unwrap_or_default();
            if let Some(diag) = evaluate_drawer_seed(ctx, namespace, finished) {
                diagnostics.push(diag);
            }
            return;
        }
        record_drawer_property(ctx, raw, trimmed, offset, state);
    } else if trimmed.eq_ignore_ascii_case(":PROPERTIES:") {
        *active_drawer = Some(DrawerSeedState::default());
    }
}

/// Records `:ID:` or `:ID_SEED:` key-value pairs into `state`.
fn record_drawer_property(
    ctx: &LintContext,
    raw: &str,
    trimmed: &str,
    offset: usize,
    state: &mut DrawerSeedState,
) {
    let Some((key, val)) = parse_drawer_kv(trimmed) else {
        return;
    };
    if key.eq_ignore_ascii_case("ID") {
        state.id_val = Some(val.to_string());
    } else if key.eq_ignore_ascii_case("ID_SEED") {
        let lead = raw.len() - raw.trim_start().len();
        let (line_num, col) = ctx.source.line_col(offset + lead);
        state.seed_entry = Some((val.to_string(), line_num, col));
    }
}

/// Validates a closed drawer's `:ID_SEED:` against its `:ID:` and returns an `E010` [`Diagnostic`] on mismatch.
fn evaluate_drawer_seed(
    ctx: &LintContext,
    namespace: &uuid::Uuid,
    state: DrawerSeedState,
) -> Option<Diagnostic> {
    let (seed, line, column) = state.seed_entry?;
    let message = if seed.is_empty() {
        ":ID_SEED: must not be empty".to_string()
    } else {
        let actual_id = state.id_val.as_deref().unwrap_or("").trim();
        if actual_id.is_empty() {
            format!(":ID_SEED: \"{seed}\" requires an :ID: property on the same drawer")
        } else {
            let expected = uuid_v5(namespace, &seed).to_string();
            if actual_id.eq_ignore_ascii_case(&expected) {
                return None;
            }
            format!(":ID: \"{actual_id}\" does not match UUIDv5(\"{seed}\") = {expected}")
        }
    };

    Some(Diagnostic {
        file: ctx.source.path.clone(),
        line,
        column,
        severity: Severity::Error,
        rule_id: "E010",
        rule: "invalid-id-seed",
        message,
        fix: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::source::SourceFile;

    /// Runs `InvalidIdSeed` against an in-memory Org buffer.
    fn check_single(content: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new("seed.org", content.to_string());
        let config = Config::default();
        let ctx = LintContext::new(&source, &config);
        InvalidIdSeed.check(&ctx)
    }

    /// Verifies Stempelamt golden vectors and unseeded legacy UUIDs pass with zero errors.
    #[test]
    fn valid_golden_vectors_and_unseeded_drawers_pass() {
        let content = concat!(
            ":PROPERTIES:\n",
            ":ID:      cbba66ec-baff-5eb9-b0d3-4e38f56ebcd9\n",
            ":ID_SEED: urn:babelsberg:fritz:wp:fritz-wp-01\n",
            ":END:\n",
            "#+TITLE: Golden\n",
            "* Unseeded Legacy\n:PROPERTIES:\n:ID: 40000000-0000-4000-8000-000000000004\n:END:\n",
            "* Seeded Jira\n:PROPERTIES:\n",
            ":ID:      9472f8df-1ad0-53bb-baac-dce22a6b1821\n",
            ":ID_SEED: urn:babelsberg:orgfmt:jira:orgfmt-101\n",
            ":END:\n",
        );
        assert!(check_single(content).is_empty());
    }

    /// Verifies empty `:ID_SEED:`, missing `:ID:`, and mismatched `:ID:` all report `E010`.
    #[test]
    fn detects_empty_seed_missing_id_and_uuid_mismatch() {
        let content = concat!(
            ":PROPERTIES:\n",
            ":ID:      00000000-0000-5000-8000-000000000000\n",
            ":ID_SEED: urn:babelsberg:fritz:wp:fritz-wp-01\n",
            ":END:\n",
            "* Empty Seed\n:PROPERTIES:\n:ID: cbba66ec-baff-5eb9-b0d3-4e38f56ebcd9\n:ID_SEED:\n:END:\n",
            "* Missing ID\n:PROPERTIES:\n:id_seed: urn:babelsberg:orgfmt:jira:orgfmt-101\n:END:\n",
        );
        let diags = check_single(content);
        assert_eq!(diags.len(), 3);
        assert!(diags
            .iter()
            .all(|d| d.rule_id == "E010" && d.severity == Severity::Error));
        assert_eq!(diags[0].line, 3);
        assert_eq!(diags[1].line, 8);
        assert_eq!(diags[2].line, 12);
    }

    /// Verifies `:ID_SEED:` examples inside `#+begin_example` blocks are ignored.
    #[test]
    fn ignores_protected_example_blocks() {
        let content = concat!(
            "#+begin_example\n",
            ":PROPERTIES:\n:ID: wrong\n:ID_SEED: urn:babelsberg:fritz:wp:fritz-wp-01\n:END:\n",
            "#+end_example\n",
        );
        assert!(check_single(content).is_empty());
    }
}
