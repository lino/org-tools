// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Query engine for org-mode entries.

/// Multi-day scheduled, deadline, warning, repeater, and body-timestamp agenda engine.
pub mod agenda;
/// Human, locator, Org-mode, GFM Markdown, and JSON renderers for agenda reports.
pub mod agenda_render;
/// Dependency graph edge extraction and diagram output for `org-edna` `BLOCKER` and `TRIGGER`.
pub mod deps;
/// Candidate ranking and human/JSON/`--pick` renderers for `org [query] next`.
pub mod next_render;
/// Search, GTD context, blocked, waiting, and stuck-project output formatters.
pub mod output;
/// Recursive-descent query expression parser (`todo:`, `tags:`, `scheduled:`, `body:`, etc.).
pub mod parser;
/// Predicate evaluation and Okapi BM25 body-ranking helpers.
pub mod predicate;
