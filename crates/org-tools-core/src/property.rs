// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later
// fritz: allow-file-length=1500 reason="Cohesive property drawer, tag alignment, Stempelamt UUIDv5 engine, and unit tests"

//! Property drawer mutations, heading tag alignment, and Stempelamt UUIDv5 minting.
//!
//! Provides pure-Rust helpers for mutating `:PROPERTIES:` drawers with `F005`
//! alignment, updating heading tags aligned to column 77 (`F007`), minting
//! deterministic RFC 9562 UUIDv5 identities with `:ID_SEED:` provenance, and
//! post-processing newly created TODO entries so `org fmt check` reports zero
//! diagnostics.

use std::path::Path;

use unicode_width::UnicodeWidthStr;

use crate::document::{OrgDocument, OrgEntry};
use crate::rules::heading::{is_heading, parse_heading_with_keywords};
use crate::source::SourceFile;

// region Constants & Types

/// Default RFC 9562 URL namespace UUID used as the Stempelamt root namespace.
pub const DEFAULT_ROOT_NAMESPACE: &str = "6ba7b811-9dad-11d1-80b4-00c04fd430c8";

/// Default URN prefix for canonical Stempelamt identity seeds.
pub const DEFAULT_URN_PREFIX: &str = "urn:babelsberg";

/// Default scope token used when neither CLI nor file metadata provides one.
pub const DEFAULT_SCOPE: &str = "default";

/// Default kind token used when no explicit entry kind is provided.
pub const DEFAULT_KIND: &str = "entry";

/// Default target display column for Org-mode heading tag alignment (`F007`).
pub const DEFAULT_TAG_COLUMN: usize = 77;

/// Environment variable overriding the Stempelamt root namespace UUID.
pub const ENV_ID_NAMESPACE: &str = "BABELSBERG_ID_NAMESPACE";

/// Environment variable overriding the Stempelamt URN prefix.
pub const ENV_ID_PREFIX: &str = "BABELSBERG_ID_PREFIX";

/// Environment variable overriding the heading tag alignment column.
pub const ENV_TAG_COLUMN: &str = "ORG_TAG_COLUMN";

/// Initial 160-bit digest state defined by FIPS 180-4 SHA-1.
const SHA1_INIT_STATE: [u32; 5] = [
    0x6745_2301,
    0xEFCD_AB89,
    0x98BA_DCFE,
    0x1032_5476,
    0xC3D2_E1F0,
];

/// Block size in bytes for SHA-1 message compression.
const SHA1_BLOCK_BYTES: usize = 64;

/// Size in bytes of the big-endian bit-length trailer in SHA-1 padding.
const SHA1_LENGTH_BYTES: usize = 8;

/// Target congruence modulo 64 before appending the SHA-1 bit-length trailer.
const SHA1_PAD_MODULO: usize = SHA1_BLOCK_BYTES - SHA1_LENGTH_BYTES;

/// Number of 32-bit words in the expanded SHA-1 message schedule.
const SHA1_SCHEDULE_WORDS: usize = 80;

/// SHA-1 round constant for rounds 0 through 19.
const SHA1_K0: u32 = 0x5A82_7999;

/// SHA-1 round constant for rounds 20 through 39.
const SHA1_K1: u32 = 0x6ED9_EBA1;

/// SHA-1 round constant for rounds 40 through 59.
const SHA1_K2: u32 = 0x8F1B_BCDC;

/// SHA-1 round constant for rounds 60 through 79.
const SHA1_K3: u32 = 0xCA62_C1D6;

/// Mask clearing the upper nibble of UUID byte 6 before stamping version 5.
const UUID_VERSION_CLEAR_MASK: u8 = 0x0f;

/// Version 5 nibble (`0101_0000`) per RFC 9562 Section 5.5.
const UUID_VERSION_5_BITS: u8 = 0x50;

/// Mask clearing the upper two bits of UUID byte 8 before stamping RFC variant.
const UUID_VARIANT_CLEAR_MASK: u8 = 0x3f;

/// RFC 9562 variant bits (`1000_0000`) per RFC 9562 Section 4.1.
const UUID_RFC9562_VARIANT_BITS: u8 = 0x80;

/// Number of bytes copied from the 20-byte SHA-1 digest into a UUID.
const UUID_BYTE_LEN: usize = 16;

/// Leading marker byte appended before zero-padding in SHA-1.
const SHA1_ONE_BIT_MARKER: u8 = 0x80;

/// Resolved configuration for deterministic Stempelamt UUIDv5 minting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StempelamtConfig {
    /// Root namespace UUID hashed as the 16-byte prefix in RFC 9562 UUIDv5.
    pub namespace: uuid::Uuid,
    /// Canonical URN prefix (for example, `"urn:babelsberg"`).
    pub urn_prefix: String,
}

/// Structured input fields for constructing a canonical Stempelamt seed URN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedSpec<'a> {
    /// Project or bounded-context scope token.
    pub scope: &'a str,
    /// Artifact kind token (for example, `"wp"`, `"jira"`, `"finding"`).
    pub kind: &'a str,
    /// Raw artifact key or slug source.
    pub key: &'a str,
    /// Optional author handle prepended to the key when not already prefixed.
    pub author: Option<&'a str>,
}

/// Deterministic identity minted by the Stempelamt engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedIdentity {
    /// Lowercase hyphenated RFC 9562 UUIDv5 string.
    pub id: String,
    /// Canonical `:ID_SEED:` URN string hashed to produce [`Self::id`].
    pub seed: String,
}

/// Options controlling batch or single-entry deterministic ID minting.
#[derive(Debug, Clone)]
pub struct DeterministicIdOptions {
    /// Resolved Stempelamt namespace and URN prefix configuration.
    pub config: StempelamtConfig,
    /// Explicit seed URN overriding scope/kind/key derivation.
    pub explicit_seed: Option<String>,
    /// Optional explicit scope token override.
    pub scope: Option<String>,
    /// Optional explicit kind token override.
    pub kind: Option<String>,
    /// Optional author handle token.
    pub author: Option<String>,
}

/// Per-entry record emitted when deterministic `:ID:` and `:ID_SEED:` are added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeterministicIdEntry {
    /// Zero-based index of the entry in [`OrgDocument::entries`].
    pub entry_index: usize,
    /// One-based heading line number of the affected entry.
    pub line: usize,
    /// Minted UUIDv5 value written to `:ID:`.
    pub id: String,
    /// Canonical seed URN written to `:ID_SEED:`.
    pub seed: String,
}

/// Result of adding deterministic Stempelamt IDs to a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddDeterministicIdResult {
    /// Modified document text with aligned `:ID:` and `:ID_SEED:` properties.
    pub content: String,
    /// Details for each entry that received a minted `:ID:`.
    pub entries: Vec<DeterministicIdEntry>,
}

/// Result of mutating properties or tags on a single entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyMutationResult {
    /// Resulting document text after the operation.
    pub content: String,
    /// Whether the document text changed compared to the input source.
    pub changed: bool,
    /// One-based heading line number of the targeted entry.
    pub line: usize,
}

/// Post-processing options applied after `entry::add_entry` inserts a heading.
#[derive(Debug, Clone)]
pub struct FinalizeTodoOptions {
    /// Ordered `(KEY, VALUE)` pairs to write into `:PROPERTIES:`.
    pub properties: Vec<(String, String)>,
    /// Optional body text inserted immediately below the heading/drawer.
    pub body: Option<String>,
    /// Target column for heading tag alignment (`F007`).
    pub tag_column: usize,
}

/// Strategy for minting `:ID:` properties on newly created `add-todo` entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoIdentityMode {
    /// Do not automatically mint an `:ID:` property.
    None,
    /// Mint a random UUIDv4 `:ID:` property.
    Auto,
    /// Mint a deterministic RFC 9562 UUIDv5 `:ID:` and `:ID_SEED:` pair.
    Deterministic,
}

/// Inputs for assembling initial `:PROPERTIES:` pairs on a new TODO entry.
#[derive(Debug, Clone)]
pub struct NewTodoPropertySpec<'a> {
    /// Entry heading title used as fallback Stempelamt key.
    pub title: &'a str,
    /// Destination file path used as fallback Stempelamt scope.
    pub file_path: &'a Path,
    /// Selected `:ID:` minting strategy.
    pub id_mode: TodoIdentityMode,
    /// Optional Stempelamt root namespace UUID override.
    pub namespace: Option<&'a str>,
    /// Optional Stempelamt URN prefix override.
    pub urn_prefix: Option<&'a str>,
    /// Optional explicit Stempelamt `:ID_SEED:` URN override.
    pub seed: Option<&'a str>,
    /// Optional Stempelamt scope override.
    pub scope: Option<&'a str>,
    /// Optional Stempelamt kind override.
    pub kind: Option<&'a str>,
    /// Optional Stempelamt author handle override.
    pub author: Option<&'a str>,
    /// Raw `KEY=VALUE` strings supplied via repeatable `--property` flags.
    pub raw_properties: &'a [String],
}

impl Default for FinalizeTodoOptions {
    /// Returns default post-processing options aligned to [`DEFAULT_TAG_COLUMN`].
    fn default() -> Self {
        Self {
            properties: Vec::new(),
            body: None,
            tag_column: DEFAULT_TAG_COLUMN,
        }
    }
}

/// Result of finalizing a newly inserted TODO entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedTodoResult {
    /// Formatted document text passing `org fmt check`.
    pub content: String,
    /// One-based heading line of the newly inserted entry in [`Self::content`].
    pub heading_line: usize,
    /// Zero-based index of the newly inserted entry in the parsed document.
    pub entry_index: usize,
}

/// Internal location of an existing or missing `:PROPERTIES:` drawer.
#[derive(Debug)]
struct DrawerLocation {
    /// Zero-based line where an existing drawer starts or a new one belongs.
    start_line: usize,
    /// Inclusive zero-based `:END:` line when an existing drawer was found.
    end_line: Option<usize>,
    /// Leading whitespace preserved from `:PROPERTIES:` or heading.
    indent: String,
    /// Parsed `(key, value)` pairs inside an existing drawer.
    properties: Vec<(String, String)>,
}

// endregion

// region Stempelamt Minting

impl StempelamtConfig {
    /// Resolves namespace and URN prefix from CLI overrides, environment, or defaults.
    pub fn resolve(
        namespace_override: Option<&str>,
        prefix_override: Option<&str>,
    ) -> Result<Self, String> {
        let env_ns = std::env::var(ENV_ID_NAMESPACE).ok();
        let raw_ns = namespace_override
            .or(env_ns.as_deref())
            .unwrap_or(DEFAULT_ROOT_NAMESPACE)
            .trim();
        let namespace = uuid::Uuid::parse_str(raw_ns)
            .map_err(|e| format!("invalid Stempelamt namespace UUID '{raw_ns}': {e}"))?;

        let env_prefix = std::env::var(ENV_ID_PREFIX).ok();
        let raw_prefix = prefix_override
            .or(env_prefix.as_deref())
            .unwrap_or(DEFAULT_URN_PREFIX)
            .trim()
            .trim_end_matches(':');
        if raw_prefix.is_empty() {
            return Err("Stempelamt URN prefix must not be empty".to_string());
        }

        Ok(Self {
            namespace,
            urn_prefix: raw_prefix.to_string(),
        })
    }
}

/// Checks whether a lowercase character is permitted directly inside a Stempelamt token.
fn is_allowed_token_char(ch: char, allow_slash: bool) -> bool {
    ch.is_ascii_lowercase()
        || ch.is_ascii_digit()
        || matches!(ch, '.' | '_' | '@' | '#' | '-')
        || (allow_slash && ch == '/')
}

/// Normalizes a raw Stempelamt token into its canonical lowercase slug form.
pub fn normalize_token(raw: &str, allow_slash: bool) -> String {
    let preprocessed = raw.trim().to_lowercase().replace('\\', "/");
    let mut collapsed = String::with_capacity(preprocessed.len());

    for ch in preprocessed.chars() {
        if is_allowed_token_char(ch, allow_slash) {
            if ch != '-' || !collapsed.ends_with('-') {
                collapsed.push(ch);
            }
        } else if !collapsed.ends_with('-') {
            collapsed.push('-');
        }
    }

    collapsed.trim_matches(&['-', '/'][..]).to_string()
}

/// Builds a canonical Stempelamt seed URN from normalized components.
pub fn build_seed_urn(urn_prefix: &str, spec: &SeedSpec<'_>) -> Result<String, String> {
    let prefix = urn_prefix.trim().trim_end_matches(':');
    let norm_scope = normalize_token(spec.scope, false);
    let norm_kind = normalize_token(spec.kind, false);
    let mut norm_key = normalize_token(spec.key, true);

    if prefix.is_empty() || norm_scope.is_empty() || norm_kind.is_empty() || norm_key.is_empty() {
        return Err(
            "Stempelamt seed components (prefix, scope, kind, key) must be non-empty".to_string(),
        );
    }

    if let Some(raw_author) = spec.author {
        let norm_author = normalize_token(raw_author, false);
        if !norm_author.is_empty() {
            let author_prefix = format!("{norm_author}/");
            if !norm_key.starts_with(&author_prefix) {
                norm_key = format!("{author_prefix}{norm_key}");
            }
        }
    }

    Ok(format!("{prefix}:{norm_scope}:{norm_kind}:{norm_key}"))
}

/// Pads an arbitrary byte slice according to FIPS 180-4 Section 5.1.1.
fn sha1_pad_message(data: &[u8]) -> Vec<u8> {
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + SHA1_BLOCK_BYTES);
    padded.extend_from_slice(data);
    padded.push(SHA1_ONE_BIT_MARKER);
    while padded.len() % SHA1_BLOCK_BYTES != SHA1_PAD_MODULO {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    padded
}

/// Computes the FIPS 180-4 boolean function and round constant for `round`.
fn sha1_round_fn(round: usize, b: u32, c: u32, d: u32) -> (u32, u32) {
    match round {
        0..=19 => ((b & c) | ((!b) & d), SHA1_K0),
        20..=39 => (b ^ c ^ d, SHA1_K1),
        40..=59 => ((b & c) | (b & d) | (c & d), SHA1_K2),
        _ => (b ^ c ^ d, SHA1_K3),
    }
}

/// Compresses a single 64-byte message block into the running SHA-1 state.
fn sha1_compress_block(state: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; SHA1_SCHEDULE_WORDS];
    for (idx, word) in w.iter_mut().take(16).enumerate() {
        let base = idx * 4;
        *word = u32::from_be_bytes([
            block[base],
            block[base + 1],
            block[base + 2],
            block[base + 3],
        ]);
    }
    for idx in 16..SHA1_SCHEDULE_WORDS {
        w[idx] = (w[idx - 3] ^ w[idx - 8] ^ w[idx - 14] ^ w[idx - 16]).rotate_left(1);
    }

    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (round, &word) in w.iter().enumerate() {
        let (f, k) = sha1_round_fn(round, b, c, d);
        let temp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = temp;
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

/// Computes the 20-byte SHA-1 digest of `data` in pure safe Rust.
fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let padded = sha1_pad_message(data);
    let mut state = SHA1_INIT_STATE;
    let mut offset = 0;
    while offset + SHA1_BLOCK_BYTES <= padded.len() {
        sha1_compress_block(&mut state, &padded[offset..offset + SHA1_BLOCK_BYTES]);
        offset += SHA1_BLOCK_BYTES;
    }

    let mut out = [0u8; 20];
    for (idx, word) in state.iter().enumerate() {
        out[idx * 4..(idx + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Computes an RFC 9562 version 5 UUID using pure-Rust SHA-1 hashing.
pub fn uuid_v5(namespace: &uuid::Uuid, name: &str) -> uuid::Uuid {
    let trimmed = name.trim();
    let mut hash_input = Vec::with_capacity(UUID_BYTE_LEN + trimmed.len());
    hash_input.extend_from_slice(namespace.as_bytes());
    hash_input.extend_from_slice(trimmed.as_bytes());

    let digest = sha1_digest(&hash_input);
    let mut bytes = [0u8; UUID_BYTE_LEN];
    bytes.copy_from_slice(&digest[..UUID_BYTE_LEN]);
    bytes[6] = (bytes[6] & UUID_VERSION_CLEAR_MASK) | UUID_VERSION_5_BITS;
    bytes[8] = (bytes[8] & UUID_VARIANT_CLEAR_MASK) | UUID_RFC9562_VARIANT_BITS;
    uuid::Uuid::from_bytes(bytes)
}

/// Mints a deterministic UUIDv5 and canonical `:ID_SEED:` from a [`SeedSpec`].
pub fn mint_identity(
    config: &StempelamtConfig,
    spec: &SeedSpec<'_>,
) -> Result<MintedIdentity, String> {
    let seed = build_seed_urn(&config.urn_prefix, spec)?;
    mint_from_seed(&config.namespace, &seed)
}

/// Mints a deterministic UUIDv5 directly from an explicit seed URN.
pub fn mint_from_seed(namespace: &uuid::Uuid, seed_urn: &str) -> Result<MintedIdentity, String> {
    let trimmed = seed_urn.trim();
    if trimmed.is_empty() {
        return Err("Stempelamt seed URN must not be empty".to_string());
    }
    let id = uuid_v5(namespace, trimmed).to_string();
    Ok(MintedIdentity {
        id,
        seed: trimmed.to_string(),
    })
}

/// Derives the canonical Stempelamt identity for a specific [`OrgEntry`].
fn derive_entry_identity(
    doc: &OrgDocument,
    entry: &OrgEntry,
    opts: &DeterministicIdOptions,
) -> Result<MintedIdentity, String> {
    if let Some(ref explicit) = opts.explicit_seed {
        return mint_from_seed(&opts.config.namespace, explicit);
    }

    let has_overrides = opts.scope.is_some() || opts.kind.is_some() || opts.author.is_some();
    if !has_overrides {
        if let Some(existing_seed) = entry.properties.get("ID_SEED").filter(|s| !s.is_empty()) {
            return mint_from_seed(&opts.config.namespace, existing_seed);
        }
    }

    let fallback_scope = doc
        .file
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_SCOPE);
    let raw_scope = opts.scope.as_deref().unwrap_or(fallback_scope);
    let raw_kind = opts.kind.as_deref().unwrap_or(DEFAULT_KIND);
    let raw_key = entry
        .properties
        .get("CUSTOM_ID")
        .map(|s| s.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&entry.title);

    mint_identity(
        &opts.config,
        &SeedSpec {
            scope: raw_scope,
            kind: raw_kind,
            key: raw_key,
            author: opts.author.as_deref(),
        },
    )
}

/// Adds deterministic `:ID:` and `:ID_SEED:` properties to entries lacking `:ID:`.
pub fn add_deterministic_ids(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_indices: Option<&[usize]>,
    opts: &DeterministicIdOptions,
) -> Result<Option<AddDeterministicIdResult>, String> {
    let mut target_indices: Vec<usize> = match entry_indices {
        Some(indices) => indices.to_vec(),
        None => (0..doc.entries.len()).collect(),
    };
    target_indices.sort_unstable();
    target_indices.dedup();

    let missing: Vec<usize> = target_indices
        .into_iter()
        .filter(|&idx| idx < doc.entries.len() && !doc.entries[idx].properties.contains_key("ID"))
        .collect();
    if missing.is_empty() {
        return Ok(None);
    }
    if opts.explicit_seed.is_some() && missing.len() > 1 {
        return Err(
            "--seed cannot be applied to multiple entries lacking :ID:; target one locator"
                .to_string(),
        );
    }

    let mut current_source = SourceFile::new(source.path.clone(), source.content.clone());
    let mut minted_by_idx: Vec<(usize, MintedIdentity)> = Vec::with_capacity(missing.len());
    for &idx in missing.iter().rev() {
        let cur_doc = OrgDocument::from_source(&current_source);
        let minted = derive_entry_identity(&cur_doc, &cur_doc.entries[idx], opts)?;
        let pairs = vec![
            ("ID".to_string(), minted.id.clone()),
            ("ID_SEED".to_string(), minted.seed.clone()),
        ];
        let updated = prepend_or_set_properties(&current_source, &cur_doc, idx, &pairs)?;
        current_source = SourceFile::new(source.path.clone(), updated.content);
        minted_by_idx.push((idx, minted));
    }

    minted_by_idx.reverse();
    let final_doc = OrgDocument::from_source(&current_source);
    let entries = minted_by_idx
        .into_iter()
        .map(|(idx, minted)| DeterministicIdEntry {
            entry_index: idx,
            line: final_doc.entries[idx].heading_line,
            id: minted.id,
            seed: minted.seed,
        })
        .collect();

    Ok(Some(AddDeterministicIdResult {
        content: current_source.content,
        entries,
    }))
}

// endregion

// region Property Drawer Operations

/// Returns `true` when `line` is an Org planning line (`SCHEDULED:`, `DEADLINE:`, `CLOSED:`).
fn is_planning_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

/// Parses a single `:KEY: value` property line inside a `:PROPERTIES:` drawer.
fn parse_drawer_property_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix(':')?;
    let colon_pos = rest.find(':')?;
    let key = &rest[..colon_pos];
    if key.is_empty() || key.chars().any(char::is_whitespace) {
        return None;
    }
    let value = rest[colon_pos + 1..].trim().to_string();
    Some((key.to_string(), value))
}

/// Validates an Org property drawer key and returns its trimmed form.
pub fn validate_property_key(key: &str) -> Result<String, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("property key must not be empty".to_string());
    }
    if trimmed.contains(':') || trimmed.chars().any(char::is_whitespace) {
        return Err(format!(
            "invalid property key '{trimmed}': keys cannot contain whitespace or ':'"
        ));
    }
    Ok(trimmed.to_string())
}

/// Formats a slice of `(key, value)` pairs with `F005` drawer value alignment.
pub fn format_property_lines(props: &[(String, String)], indent: &str) -> Vec<String> {
    if props.is_empty() {
        return Vec::new();
    }
    let max_key_len = if props.len() > 1 {
        props.iter().map(|(k, _)| k.len()).max().unwrap_or(0)
    } else {
        0
    };

    props
        .iter()
        .map(|(key, value)| {
            let trimmed_val = value.trim();
            if trimmed_val.is_empty() {
                format!("{indent}:{key}:")
            } else {
                let pad_len = max_key_len.saturating_sub(key.len());
                let pad = " ".repeat(pad_len);
                format!("{indent}:{key}:{pad} {trimmed_val}")
            }
        })
        .collect()
}

/// Locates an entry's `:PROPERTIES:` drawer or computes where to insert a new one.
fn locate_entry_drawer(lines: &[String], entry: &OrgEntry) -> DrawerLocation {
    let heading_idx = entry.heading_line - 1;
    let end_idx = (entry.content_end_line - 1).min(lines.len());
    let mut cursor = heading_idx + 1;
    while cursor < end_idx && is_planning_line(&lines[cursor]) {
        cursor += 1;
    }

    if cursor < end_idx && lines[cursor].trim().eq_ignore_ascii_case(":PROPERTIES:") {
        let raw = &lines[cursor];
        let indent = raw[..raw.len() - raw.trim_start().len()].to_string();
        let mut properties = Vec::new();
        for (idx, line) in lines.iter().enumerate().take(end_idx).skip(cursor + 1) {
            if line.trim().eq_ignore_ascii_case(":END:") {
                return DrawerLocation {
                    start_line: cursor,
                    end_line: Some(idx),
                    indent,
                    properties,
                };
            }
            if let Some(pair) = parse_drawer_property_line(line) {
                properties.push(pair);
            }
        }
    }

    DrawerLocation {
        start_line: cursor,
        end_line: None,
        indent: String::new(),
        properties: Vec::new(),
    }
}

/// Applies updated `(key, value)` pairs to `lines` at `loc` while keeping `F003`/`F005` clean.
fn splice_drawer_lines(
    mut lines: Vec<String>,
    had_trailing_newline: bool,
    heading_idx: usize,
    loc: &DrawerLocation,
    props: &[(String, String)],
) -> String {
    let mut drawer_block = Vec::new();
    if !props.is_empty() {
        drawer_block.push(format!("{}:PROPERTIES:", loc.indent));
        drawer_block.extend(format_property_lines(props, &loc.indent));
        drawer_block.push(format!("{}:END:", loc.indent));
    }

    if let Some(end_line) = loc.end_line {
        lines.splice(loc.start_line..=end_line, drawer_block);
        if props.is_empty() && loc.start_line == heading_idx + 1 {
            collapse_blank_before_consecutive_heading(&mut lines, heading_idx + 1);
        }
    } else if !drawer_block.is_empty() {
        let next_is_heading = lines.get(loc.start_line).is_some_and(|l| is_heading(l));
        if next_is_heading {
            drawer_block.push(String::new());
        }
        let append_trailing_empty = !had_trailing_newline && loc.start_line == lines.len();
        lines.splice(loc.start_line..loc.start_line, drawer_block);
        if append_trailing_empty {
            lines.push(String::new());
        }
    }

    lines.join("\n")
}

/// Removes blank lines between two consecutive headings when an empty drawer is deleted.
fn collapse_blank_before_consecutive_heading(lines: &mut Vec<String>, start: usize) {
    let mut idx = start;
    while idx < lines.len() && lines[idx].trim().is_empty() {
        idx += 1;
    }
    if idx > start && idx < lines.len() && is_heading(&lines[idx]) {
        lines.drain(start..idx);
    }
}

/// Splits source text into owned `\n`-delimited lines with `\r` stripped.
fn split_clean_lines(content: &str) -> Vec<String> {
    content
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect()
}

/// Updates or inserts properties, placing new `:ID:`/`:ID_SEED:` keys at the front of the drawer.
fn prepend_or_set_properties(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    pairs: &[(String, String)],
) -> Result<PropertyMutationResult, String> {
    let entry = doc
        .entries
        .get(entry_idx)
        .ok_or_else(|| format!("entry index {entry_idx} out of bounds"))?;
    let lines = split_clean_lines(&source.content);
    let loc = locate_entry_drawer(&lines, entry);
    let mut merged: Vec<(String, String)> = Vec::new();

    for (raw_k, raw_v) in pairs {
        let k = validate_property_key(raw_k)?;
        merged.push((k, raw_v.trim().to_string()));
    }
    for (existing_k, existing_v) in &loc.properties {
        if !merged
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(existing_k))
        {
            merged.push((existing_k.clone(), existing_v.clone()));
        }
    }

    let content = splice_drawer_lines(
        lines,
        source.content.ends_with('\n'),
        entry.heading_line - 1,
        &loc,
        &merged,
    );
    let changed = content != source.content;
    Ok(PropertyMutationResult {
        content,
        changed,
        line: entry.heading_line,
    })
}

/// Sets or inserts a single property inside the target entry's `:PROPERTIES:` drawer.
pub fn set_property(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    key: &str,
    value: &str,
) -> Result<PropertyMutationResult, String> {
    let valid_key = validate_property_key(key)?;
    set_properties(source, doc, entry_idx, &[(valid_key, value.to_string())])
}

/// Sets or inserts multiple properties in order inside the target entry's drawer.
pub fn set_properties(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    pairs: &[(String, String)],
) -> Result<PropertyMutationResult, String> {
    let entry = doc
        .entries
        .get(entry_idx)
        .ok_or_else(|| format!("entry index {entry_idx} out of bounds"))?;
    let lines = split_clean_lines(&source.content);
    let loc = locate_entry_drawer(&lines, entry);
    let mut props = loc.properties.clone();

    for (raw_key, raw_val) in pairs {
        let key = validate_property_key(raw_key)?;
        let val = raw_val.trim().to_string();
        if let Some(existing) = props.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            *existing = (key, val);
        } else {
            props.push((key, val));
        }
    }

    let content = splice_drawer_lines(
        lines,
        source.content.ends_with('\n'),
        entry.heading_line - 1,
        &loc,
        &props,
    );
    let changed = content != source.content;
    Ok(PropertyMutationResult {
        content,
        changed,
        line: entry.heading_line,
    })
}

/// Deletes a property from the target entry's `:PROPERTIES:` drawer and realigns.
pub fn delete_property(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    key: &str,
) -> Result<PropertyMutationResult, String> {
    let valid_key = validate_property_key(key)?;
    let entry = doc
        .entries
        .get(entry_idx)
        .ok_or_else(|| format!("entry index {entry_idx} out of bounds"))?;
    let lines = split_clean_lines(&source.content);
    let loc = locate_entry_drawer(&lines, entry);
    if loc.end_line.is_none() {
        return Ok(PropertyMutationResult {
            content: source.content.clone(),
            changed: false,
            line: entry.heading_line,
        });
    }

    let before_len = loc.properties.len();
    let remaining: Vec<(String, String)> = loc
        .properties
        .iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case(&valid_key))
        .cloned()
        .collect();
    if remaining.len() == before_len {
        return Ok(PropertyMutationResult {
            content: source.content.clone(),
            changed: false,
            line: entry.heading_line,
        });
    }

    let content = splice_drawer_lines(
        lines,
        source.content.ends_with('\n'),
        entry.heading_line - 1,
        &loc,
        &remaining,
    );
    let changed = content != source.content;
    Ok(PropertyMutationResult {
        content,
        changed,
        line: entry.heading_line,
    })
}

// endregion

// region Tag Operations

/// Resolves the target heading tag alignment column from override, env, or default.
pub fn resolve_tag_column(override_col: Option<usize>) -> usize {
    if let Some(col) = override_col {
        return col;
    }
    std::env::var(ENV_TAG_COLUMN)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_TAG_COLUMN)
}

/// Validates that a single Org tag token contains only legal heading tag characters.
fn validate_single_tag(tag: &str) -> Result<(), String> {
    let valid = !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '@');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid tag '{tag}': tags must contain only alphanumeric, '_', or '@' characters"
        ))
    }
}

/// Parses tag input (`:a:b:`, `a,b`, `""`, or `--clear`) into validated tag tokens.
pub fn parse_tag_input(raw: Option<&str>, clear: bool) -> Result<Vec<String>, String> {
    if clear {
        return Ok(Vec::new());
    }
    let raw_str = raw.ok_or_else(|| "specify TAGS or pass --clear".to_string())?;
    if raw_str.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut tags: Vec<String> = Vec::new();
    for segment in raw_str.split([':', ',']) {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            continue;
        }
        validate_single_tag(trimmed)?;
        if !tags.iter().any(|existing| existing == trimmed) {
            tags.push(trimmed.to_string());
        }
    }
    Ok(tags)
}

/// Rebuilds a single heading line with `tags` aligned to `tag_column` (`F007`).
pub fn format_heading_with_tags(
    heading_line: &str,
    keywords: &[&str],
    tags: &[String],
    tag_column: usize,
) -> Result<String, String> {
    let raw = heading_line.strip_suffix('\r').unwrap_or(heading_line);
    let parts = parse_heading_with_keywords(raw, keywords)
        .ok_or_else(|| format!("line is not a valid org heading: '{raw}'"))?;

    let mut prefix = format!("{} ", "*".repeat(parts.level));
    if let Some(kw) = parts.keyword {
        prefix.push_str(kw);
        prefix.push(' ');
    }
    if let Some(pri) = parts.priority {
        prefix.push_str(&format!("[#{pri}] "));
    }
    prefix.push_str(parts.title);
    let prefix_trimmed = prefix.trim_end();

    if tags.is_empty() {
        return Ok(prefix_trimmed.to_string());
    }

    for tag in tags {
        validate_single_tag(tag)?;
    }
    let tag_str = format!(":{}:", tags.join(":"));
    let prefix_width = UnicodeWidthStr::width(prefix_trimmed);
    let tag_width = UnicodeWidthStr::width(tag_str.as_str());
    let spaces = if prefix_width + 1 + tag_width <= tag_column {
        tag_column - tag_width - prefix_width
    } else {
        1
    };

    Ok(format!("{prefix_trimmed}{}{tag_str}", " ".repeat(spaces)))
}

/// Updates tags on a single heading line aligned to `tag_column` (`F007`).
pub fn set_tags(
    source: &SourceFile,
    doc: &OrgDocument,
    entry_idx: usize,
    tags: &[String],
    tag_column: usize,
) -> Result<PropertyMutationResult, String> {
    let entry = doc
        .entries
        .get(entry_idx)
        .ok_or_else(|| format!("entry index {entry_idx} out of bounds"))?;
    let mut lines = split_clean_lines(&source.content);
    let heading_idx = entry.heading_line - 1;
    let kw_refs = doc.todo_keywords.all();
    lines[heading_idx] = format_heading_with_tags(&lines[heading_idx], &kw_refs, tags, tag_column)?;

    let content = lines.join("\n");
    let changed = content != source.content;
    Ok(PropertyMutationResult {
        content,
        changed,
        line: entry.heading_line,
    })
}

// endregion

// region Entry Composition

/// Parses repeatable `KEY=VALUE` CLI property assignments into validated pairs.
pub fn parse_key_value_pairs(raw_pairs: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut pairs = Vec::with_capacity(raw_pairs.len());
    for raw in raw_pairs {
        let (key_part, val_part) = raw
            .split_once('=')
            .ok_or_else(|| format!("invalid --property '{raw}': expected KEY=VALUE"))?;
        let valid_key = validate_property_key(key_part)?;
        let val = val_part.trim().to_string();
        if let Some(existing) = pairs
            .iter_mut()
            .find(|(k, _): &&mut (String, String)| k.eq_ignore_ascii_case(&valid_key))
        {
            *existing = (valid_key, val);
        } else {
            pairs.push((valid_key, val));
        }
    }
    Ok(pairs)
}

/// Mints deterministic `:ID:` and `:ID_SEED:` properties for a new TODO entry.
fn mint_new_todo_identity(
    spec: &NewTodoPropertySpec<'_>,
    user_props: &[(String, String)],
) -> Result<MintedIdentity, String> {
    let cfg = StempelamtConfig::resolve(spec.namespace, spec.urn_prefix)?;
    if let Some(explicit) = spec.seed {
        return mint_from_seed(&cfg.namespace, explicit);
    }
    let fallback_scope = spec
        .file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_SCOPE);
    let raw_key = user_props
        .iter()
        .find(|(k, v)| k.eq_ignore_ascii_case("CUSTOM_ID") && !v.is_empty())
        .map_or(spec.title, |(_, v)| v.as_str());
    mint_identity(
        &cfg,
        &SeedSpec {
            scope: spec.scope.unwrap_or(fallback_scope),
            kind: spec.kind.unwrap_or(DEFAULT_KIND),
            key: raw_key,
            author: spec.author,
        },
    )
}

/// Assembles initial `:PROPERTIES:` pairs for a new TODO entry including `:ID:` minting.
pub fn build_todo_properties(
    spec: &NewTodoPropertySpec<'_>,
) -> Result<Vec<(String, String)>, String> {
    let user_props = parse_key_value_pairs(spec.raw_properties)?;
    let mut result = Vec::new();

    match spec.id_mode {
        TodoIdentityMode::None => {}
        TodoIdentityMode::Auto => {
            if !user_props.iter().any(|(k, _)| k.eq_ignore_ascii_case("ID")) {
                result.push(("ID".to_string(), uuid::Uuid::new_v4().to_string()));
            }
        }
        TodoIdentityMode::Deterministic => {
            let minted = mint_new_todo_identity(spec, &user_props)?;
            result.push(("ID".to_string(), minted.id));
            result.push(("ID_SEED".to_string(), minted.seed));
        }
    }

    for (key, val) in user_props {
        if !result.iter().any(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            result.push((key, val));
        }
    }
    Ok(result)
}

/// Normalizes blank lines directly above `heading_idx` to match `F003` (`HeadingSpacing`).
fn normalize_spacing_before_heading(lines: &mut Vec<String>, heading_idx: usize) {
    if heading_idx == 0 {
        return;
    }
    let mut blank_count = 0;
    let mut cursor = heading_idx;
    while cursor > 0 && lines[cursor - 1].trim().is_empty() {
        blank_count += 1;
        cursor -= 1;
    }

    if cursor == 0 {
        lines.drain(0..heading_idx);
        return;
    }

    let prev_is_heading = is_heading(&lines[cursor - 1]);
    let desired = usize::from(!prev_is_heading);
    if blank_count != desired {
        let replacement = vec![String::new(); desired];
        lines.splice(cursor..heading_idx, replacement);
    }
}

/// Inserts body text immediately after an entry's heading, planning line, and drawer.
fn insert_entry_body(content: &str, file_path: &Path, entry_idx: usize, body: &str) -> String {
    let source = SourceFile::new(file_path, content.to_string());
    let doc = OrgDocument::from_source(&source);
    let entry = &doc.entries[entry_idx];
    let mut lines = split_clean_lines(content);
    let loc = locate_entry_drawer(&lines, entry);
    let insert_at = loc.end_line.map_or(loc.start_line, |end| end + 1);

    let mut body_lines: Vec<String> = body
        .trim_end_matches('\n')
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).trim_end().to_string())
        .collect();

    let next_non_blank = lines.iter().skip(insert_at).find(|l| !l.trim().is_empty());
    let next_immediately_heading = lines.get(insert_at).is_some_and(|l| is_heading(l));
    if next_immediately_heading && next_non_blank.is_some() {
        body_lines.push(String::new());
    }

    lines.splice(insert_at..insert_at, body_lines);
    lines.join("\n")
}

/// Post-processes content produced by `entry::add_entry` with drawer, tags, and body.
pub fn finalize_added_todo(
    was_empty: bool,
    added_content: &str,
    file_path: &Path,
    parent_idx: Option<usize>,
    opts: &FinalizeTodoOptions,
) -> Result<FinalizedTodoResult, String> {
    let initial = if was_empty {
        added_content.trim_start_matches('\n').to_string()
    } else {
        added_content.to_string()
    };

    let s1 = SourceFile::new(file_path, initial);
    let d1 = OrgDocument::from_source(&s1);
    let entry_idx = match parent_idx {
        Some(pidx) => *d1
            .entries
            .get(pidx)
            .and_then(|e| e.children.last())
            .ok_or_else(|| "newly added child entry not found".to_string())?,
        None => d1
            .entries
            .len()
            .checked_sub(1)
            .ok_or_else(|| "newly added entry not found".to_string())?,
    };

    let mut lines = split_clean_lines(&s1.content);
    normalize_spacing_before_heading(&mut lines, d1.entries[entry_idx].heading_line - 1);
    let mut current = lines.join("\n");

    let s2 = SourceFile::new(file_path, current.clone());
    let d2 = OrgDocument::from_source(&s2);
    if !d2.entries[entry_idx].tags.is_empty() {
        let tags = d2.entries[entry_idx].tags.clone();
        current = set_tags(&s2, &d2, entry_idx, &tags, opts.tag_column)?.content;
    }

    if !opts.properties.is_empty() {
        let s3 = SourceFile::new(file_path, current);
        let d3 = OrgDocument::from_source(&s3);
        current = set_properties(&s3, &d3, entry_idx, &opts.properties)?.content;
    }

    if let Some(ref body) = opts.body {
        if !body.is_empty() {
            current = insert_entry_body(&current, file_path, entry_idx, body);
        }
    }

    let final_source = SourceFile::new(file_path, current.clone());
    let final_doc = OrgDocument::from_source(&final_source);
    Ok(FinalizedTodoResult {
        content: current,
        heading_line: final_doc.entries[entry_idx].heading_line,
        entry_index: entry_idx,
    })
}

// endregion

// region Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{add_entry, NewEntryOpts};
    use crate::runner::Runner;

    /// Builds a `SourceFile` and parsed `OrgDocument` for in-memory testing.
    fn make_doc(content: &str) -> (SourceFile, OrgDocument) {
        let source = SourceFile::new("test.org", content.to_string());
        let doc = OrgDocument::from_source(&source);
        (source, doc)
    }

    /// Asserts that `Runner::default().check` reports zero format or lint issues.
    fn assert_fmt_clean(content: &str) {
        let source = SourceFile::new("test.org", content.to_string());
        let diags = Runner::default().check(&source);
        assert!(
            diags.is_empty(),
            "expected zero diagnostics, got: {diags:?}\ncontent:\n{content}"
        );
    }

    /// Verifies token normalization with and without slash preservation.
    #[test]
    fn normalize_token_matches_stempelamt_rules() {
        assert_eq!(normalize_token("  FRITZ-WP-01  ", false), "fritz-wp-01");
        assert_eq!(
            normalize_token("alice\\20261106--cache   race!!", true),
            "alice/20261106-cache-race"
        );
        assert_eq!(
            normalize_token("alice\\20261106--cache   race!!", false),
            "alice-20261106-cache-race"
        );
        assert_eq!(normalize_token("/--Foo..Bar_@#--/", true), "foo..bar_@#");
    }

    /// Verifies bit-for-bit parity across all four Stempelamt golden vectors.
    #[test]
    fn stempelamt_golden_vectors_match_bit_for_bit() {
        let cfg = StempelamtConfig::resolve(Some(DEFAULT_ROOT_NAMESPACE), Some(DEFAULT_URN_PREFIX))
            .unwrap();
        let vectors = [
            (
                "fritz",
                "wp",
                "FRITZ-WP-01",
                None,
                "urn:babelsberg:fritz:wp:fritz-wp-01",
                "cbba66ec-baff-5eb9-b0d3-4e38f56ebcd9",
            ),
            (
                "orgfmt",
                "jira",
                "ORGFMT-101",
                None,
                "urn:babelsberg:orgfmt:jira:orgfmt-101",
                "9472f8df-1ad0-53bb-baac-dce22a6b1821",
            ),
            (
                "fritz",
                "finding",
                "20261106-cache-race",
                Some("alice"),
                "urn:babelsberg:fritz:finding:alice/20261106-cache-race",
                "2d68a2b3-33ca-574f-a339-e55515473106",
            ),
            (
                "fritz",
                "finding",
                "20261106-cache-race",
                Some("bob"),
                "urn:babelsberg:fritz:finding:bob/20261106-cache-race",
                "cb9854df-43a7-566f-8476-67333b5deec2",
            ),
        ];

        for (scope, kind, key, author, expected_seed, expected_id) in vectors {
            let spec = SeedSpec {
                scope,
                kind,
                key,
                author,
            };
            let minted = mint_identity(&cfg, &spec).unwrap();
            assert_eq!(minted.seed, expected_seed);
            assert_eq!(minted.id, expected_id);
        }
    }

    /// Verifies that an already author-prefixed key is not double-prefixed.
    #[test]
    fn build_seed_urn_does_not_double_prefix_author() {
        let seed = build_seed_urn(
            DEFAULT_URN_PREFIX,
            &SeedSpec {
                scope: "fritz",
                kind: "finding",
                key: "alice/20261106-cache-race",
                author: Some("alice"),
            },
        )
        .unwrap();
        assert_eq!(
            seed,
            "urn:babelsberg:fritz:finding:alice/20261106-cache-race"
        );
    }

    /// Verifies `set_property` creates a drawer after planning and aligns keys (`F005`).
    #[test]
    fn set_property_creates_and_aligns_drawer_preserving_body() {
        let (s1, d1) = make_doc("* TODO Task\nSCHEDULED: <2026-04-01 Wed>\nBody line 1.\n");
        let r1 = set_property(&s1, &d1, 0, "ID", "abc").unwrap();
        assert!(r1.changed);
        assert_fmt_clean(&r1.content);

        let (s2, d2) = make_doc(&r1.content);
        let r2 = set_property(&s2, &d2, 0, "CUSTOM_ID", "my-task").unwrap();
        assert!(r2.changed);
        assert!(r2
            .content
            .contains(":ID:        abc\n:CUSTOM_ID: my-task\n"));
        assert!(r2.content.contains("Body line 1.\n"));
        assert_fmt_clean(&r2.content);
    }

    /// Verifies `set_property` on consecutive headings preserves `F003` heading spacing.
    #[test]
    fn set_property_between_consecutive_headings_stays_fmt_clean() {
        let (s1, d1) = make_doc("* First\n* Second\n");
        let r1 = set_property(&s1, &d1, 0, "EFFORT", "0:45").unwrap();
        assert!(r1.changed);
        assert_fmt_clean(&r1.content);

        let (s2, d2) = make_doc(&r1.content);
        let r2 = delete_property(&s2, &d2, 0, "EFFORT").unwrap();
        assert_eq!(r2.content, "* First\n* Second\n");
        assert_fmt_clean(&r2.content);
    }

    /// Verifies `delete_property` realigns remaining keys and removes empty drawers.
    #[test]
    fn delete_property_realigns_and_removes_empty_drawer() {
        let (s1, d1) =
            make_doc("* Heading\n:PROPERTIES:\n:ID:        abc\n:CUSTOM_ID: sec\n:END:\nBody.\n");
        let r1 = delete_property(&s1, &d1, 0, "CUSTOM_ID").unwrap();
        assert!(r1.changed);
        assert!(r1.content.contains(":PROPERTIES:\n:ID: abc\n:END:\n"));
        assert_fmt_clean(&r1.content);

        let (s2, d2) = make_doc(&r1.content);
        let r2 = delete_property(&s2, &d2, 0, "ID").unwrap();
        assert!(r2.changed);
        assert!(!r2.content.contains(":PROPERTIES:"));
        assert_eq!(r2.content, "* Heading\nBody.\n");
        assert_fmt_clean(&r2.content);
    }

    /// Verifies `set_tags` aligns to column 77 and supports clearing tags.
    #[test]
    fn set_tags_aligns_column_77_and_clears() {
        let (s1, d1) = make_doc("* TODO Write docs\n");
        let tags = parse_tag_input(Some("work,docs"), false).unwrap();
        let r1 = set_tags(&s1, &d1, 0, &tags, DEFAULT_TAG_COLUMN).unwrap();
        assert!(r1.changed);
        assert_eq!(r1.content.trim_end().len(), DEFAULT_TAG_COLUMN);
        assert!(r1.content.trim_end().ends_with(":work:docs:"));
        assert_fmt_clean(&r1.content);

        let (s2, d2) = make_doc(&r1.content);
        let cleared = parse_tag_input(None, true).unwrap();
        let r2 = set_tags(&s2, &d2, 0, &cleared, DEFAULT_TAG_COLUMN).unwrap();
        assert_eq!(r2.content, "* TODO Write docs\n");
        assert_fmt_clean(&r2.content);
    }

    /// Verifies `add_deterministic_ids` writes aligned `:ID:` and `:ID_SEED:` across entries.
    #[test]
    fn add_deterministic_ids_batches_entries_fmt_clean() {
        let (source, doc) = make_doc("* FRITZ-WP-01\n* Second Entry\n");
        let opts = DeterministicIdOptions {
            config: StempelamtConfig::resolve(
                Some(DEFAULT_ROOT_NAMESPACE),
                Some(DEFAULT_URN_PREFIX),
            )
            .unwrap(),
            explicit_seed: None,
            scope: Some("fritz".to_string()),
            kind: Some("wp".to_string()),
            author: None,
        };
        let res = add_deterministic_ids(&source, &doc, None, &opts)
            .unwrap()
            .unwrap();
        assert_eq!(res.entries.len(), 2);
        assert_eq!(res.entries[0].id, "cbba66ec-baff-5eb9-b0d3-4e38f56ebcd9");
        assert_eq!(res.entries[0].seed, "urn:babelsberg:fritz:wp:fritz-wp-01");
        assert_fmt_clean(&res.content);
    }

    /// Verifies `finalize_added_todo` composes cleanly with `entry::add_entry`.
    #[test]
    fn finalize_added_todo_passes_fmt_check() {
        let (source, doc) = make_doc("");
        let added = add_entry(
            &source,
            &doc,
            None,
            &NewEntryOpts {
                title: "Implement Stempelamt".to_string(),
                level: 1,
                keyword: Some("TODO".to_string()),
                priority: Some('A'),
                tags: vec!["fritz".to_string(), "rust".to_string()],
                scheduled: Some("2026-11-07".to_string()),
                deadline: None,
            },
        );
        let finalized = finalize_added_todo(
            true,
            &added.content,
            Path::new("test.org"),
            None,
            &FinalizeTodoOptions {
                properties: vec![
                    (
                        "ID".to_string(),
                        "cbba66ec-baff-5eb9-b0d3-4e38f56ebcd9".to_string(),
                    ),
                    (
                        "ID_SEED".to_string(),
                        "urn:babelsberg:fritz:wp:fritz-wp-01".to_string(),
                    ),
                    ("CUSTOM_ID".to_string(), "FRITZ-WP-01".to_string()),
                ],
                body: Some("Body paragraph for the new entry.".to_string()),
                tag_column: DEFAULT_TAG_COLUMN,
            },
        )
        .unwrap();
        assert_eq!(finalized.heading_line, 1);
        assert_fmt_clean(&finalized.content);
    }
}

// endregion
