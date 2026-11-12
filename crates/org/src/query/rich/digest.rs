// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! SHA-256 digest computation, clean body extraction, and link extraction.

use crate::query::output::QueryLinkJson;

// region Constants

/// Sixty-four 32-bit round constants for SHA-256 defined by NIST FIPS 180-4.
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Initial hash state for SHA-256 defined by NIST FIPS 180-4.
const H_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

// endregion

// region Pure-Rust SHA-256

/// Compresses a 64-byte block into the SHA-256 state vector.
fn compress_block(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (i, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
        w[i] = u32::from_be_bytes(*chunk);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let mut vars = *state;
    for (i, &wi) in w.iter().enumerate() {
        let s1 = vars[4].rotate_right(6) ^ vars[4].rotate_right(11) ^ vars[4].rotate_right(25);
        let ch = (vars[4] & vars[5]) ^ (!vars[4] & vars[6]);
        let t1 = vars[7]
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(wi);
        let s0 = vars[0].rotate_right(2) ^ vars[0].rotate_right(13) ^ vars[0].rotate_right(22);
        let maj = (vars[0] & vars[1]) ^ (vars[0] & vars[2]) ^ (vars[1] & vars[2]);
        let t2 = s0.wrapping_add(maj);
        vars[7] = vars[6];
        vars[6] = vars[5];
        vars[5] = vars[4];
        vars[4] = vars[3].wrapping_add(t1);
        vars[3] = vars[2];
        vars[2] = vars[1];
        vars[1] = vars[0];
        vars[0] = t1.wrapping_add(t2);
    }
    for (s, v) in state.iter_mut().zip(vars.iter()) {
        *s = s.wrapping_add(*v);
    }
}

/// Computes a lowercase hexadecimal SHA-256 digest of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut state = H_INIT;
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut buf = bytes.to_vec();
    buf.push(0x80);
    while !(buf.len() + 8).is_multiple_of(64) {
        buf.push(0x00);
    }
    buf.extend_from_slice(&bit_len.to_be_bytes());
    for block in buf.as_chunks::<64>().0 {
        compress_block(&mut state, block);
    }
    state.iter().map(|w| format!("{w:08x}")).collect()
}

// endregion

// region Digest & Link Extraction

/// Strips excluded properties from drawer lines before computing canonical SHA-256.
pub fn compute_canonical_digest(raw: &str, exclude_properties: &[String]) -> String {
    let mut lines_out = Vec::new();
    let mut in_drawer = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(":PROPERTIES:") {
            in_drawer = true;
            lines_out.push(line);
            continue;
        }
        if in_drawer {
            if trimmed.eq_ignore_ascii_case(":END:") {
                in_drawer = false;
                lines_out.push(line);
                continue;
            }
            if let Some(stripped) = trimmed.strip_prefix(':') {
                if let Some((k, _)) = stripped.split_once(':') {
                    let key_upper = k.trim().to_ascii_uppercase();
                    if exclude_properties
                        .iter()
                        .any(|ex| ex.eq_ignore_ascii_case(&key_upper))
                    {
                        continue;
                    }
                }
            }
        }
        lines_out.push(line);
    }

    let normalized = lines_out.join("\n");
    let hex = sha256_hex(normalized.as_bytes());
    format!("sha256:{hex}")
}

/// Identifies the URI scheme or link category from `target`.
fn classify_link_scheme(target: &str) -> &'static str {
    if target.starts_with("id:") {
        "id"
    } else if target.starts_with("file:") || target.ends_with(".org") {
        "file"
    } else if target.starts_with("https://") {
        "https"
    } else if target.starts_with("http://") {
        "http"
    } else if target.starts_with('#') {
        "custom_id"
    } else {
        "other"
    }
}

/// Scans a single line for Org links (`[[target][desc]]` or `[[target]]`).
fn scan_line_links(line: &str, line_no: usize, out: &mut Vec<QueryLinkJson>) {
    let mut cur = line;
    while let Some(start) = cur.find("[[") {
        let after_open = &cur[start + 2..];
        let Some(end) = after_open.find("]]") else {
            break;
        };
        let link_content = &after_open[..end];
        let (raw_target, description) = if let Some((t, d)) = link_content.split_once("][") {
            (t, Some(d.to_string()))
        } else {
            (link_content, None)
        };
        let (kind, target) = if let Some(uuid) = raw_target.strip_prefix("id:") {
            ("id", uuid)
        } else if let Some(path) = raw_target.strip_prefix("file:") {
            ("file", path)
        } else {
            (classify_link_scheme(raw_target), raw_target)
        };
        out.push(QueryLinkJson {
            kind: kind.to_string(),
            target: target.to_string(),
            description,
            line: line_no,
        });
        cur = &after_open[end + 2..];
    }
}

/// Extracts outbound links from `text` with 1-based source line numbers.
pub fn extract_links(text: &str, start_line: usize) -> Vec<QueryLinkJson> {
    let mut links = Vec::new();
    for (i, line) in text.lines().enumerate() {
        scan_line_links(line, start_line + i, &mut links);
    }
    links
}

/// Checks if line is an ignored drawer or planning line in body extraction.
fn is_body_skip_line(trimmed: &str, in_drawer: &mut bool) -> bool {
    if trimmed.starts_with(":PROPERTIES:") || trimmed.starts_with(":LOGBOOK:") {
        *in_drawer = true;
        return true;
    }
    if *in_drawer {
        if trimmed.eq_ignore_ascii_case(":END:") {
            *in_drawer = false;
        }
        return true;
    }
    trimmed.starts_with("SCHEDULED:")
        || trimmed.starts_with("DEADLINE:")
        || trimmed.starts_with("CLOSED:")
}

/// Extracts clean body text from `text`, stripping property drawers and planning lines.
pub fn extract_clean_body(text: &str, is_file_node: bool) -> String {
    let mut kept = Vec::new();
    let mut in_drawer = false;
    let mut skipped_heading = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if !is_file_node && !skipped_heading && trimmed.starts_with('*') {
            skipped_heading = true;
            continue;
        }
        if is_file_node && trimmed.starts_with("#+") {
            continue;
        }
        if is_body_skip_line(trimmed, &mut in_drawer) {
            continue;
        }
        kept.push(line);
    }
    kept.join("\n").trim().to_string()
}

// endregion
