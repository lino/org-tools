// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rich query and show helpers: SHA-256 digest, outbound link extraction, clean body, and FTS5 ranking.

#[path = "rich/digest.rs"]
pub mod digest;

#[path = "rich/search.rs"]
pub mod search;

pub use digest::{compute_canonical_digest, extract_clean_body, extract_links};
pub use search::run_search_command;
